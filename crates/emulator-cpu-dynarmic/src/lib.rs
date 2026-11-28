#![cfg(all(
    feature = "jit",
    any(target_arch = "aarch64", target_arch = "x86_64"),
    any(target_os = "linux", target_os = "macos")
))]

mod ffi;
mod loops;
mod rprofile;
mod snapshot;
mod status;

pub use rprofile::Config as RProfileConfig;

use fujimod_emulator_cpu::{memory::Backing, *};
use std::cell::{Cell, RefCell, UnsafeCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::Ordering;

#[derive(Clone, Copy)]
enum Mapping {
    Backing {
        bytes: usize,
        permissions: u32,
        ptr: NonNull<u8>,
        physical_base: u64,
    },
    Mmio {
        bytes: usize,
        callbacks: Mmio,
        project: Option<DeviceReadProjection>,
    },
}

impl Mapping {
    fn bytes(self) -> usize {
        match self {
            Self::Backing { bytes, .. } | Self::Mmio { bytes, .. } => bytes,
        }
    }
    fn permissions(self) -> u32 {
        match self {
            Self::Backing { permissions, .. } => permissions,
            Self::Mmio { .. } => 3,
        }
    }
}

fn ram_window_end(address: u64, physical: u64, bytes: usize) -> Result<u64, String> {
    let end = address
        .checked_add(bytes as u64)
        .ok_or("Dynarmic mapping overflows")?;
    if bytes == 0
        || end > 1 << 32
        || physical.checked_add(bytes as u64).is_none()
        || !address.is_multiple_of(4096)
        || !bytes.is_multiple_of(4096)
    {
        return Err("Dynarmic requires a nonempty page-aligned 32-bit mapping".into());
    }
    Ok(end)
}

extern "C" fn batch_instruction(word: u32) -> u32 {
    let body = word & 0x0fff_ffff;
    if matches!(word & !15, 0xf57ff040 | 0xf57ff050) {
        return 1;
    }
    if armv7a::data_cache_set_way(word) {
        return 4;
    }
    if word == 0xf10c_0080 {
        return 5;
    }
    if armv7a::mpidr_read(word) && word >> 28 != 15 {
        return 6;
    }
    if word >> 28 == 15
        || armv7a::framework_instruction(word)
        || armv7a::exclusive_instruction(word)
        || loops::is_self_branch(word)
    {
        return 0;
    }
    if body & 0x0f00_0e10 == 0x0e00_0a00 {
        return 8; // VFP data processing, gated by the translation context.
    }
    if body & 0x0fff_0fff == 0x010f_0000 && (body >> 12) & 15 != 15 {
        return 1;
    }
    if matches!(
        body & 0x0ff0_03f0,
        0x06e0_0070 | 0x06f0_0070 | 0x06a0_0070 | 0x06b0_0070
    ) || matches!(
        body & 0x0fe0_00f0,
        0x0000_0090 | 0x0020_0090 | 0x0080_0090 | 0x00a0_0090 | 0x00c0_0090 | 0x00e0_0090
    ) || body & 0x0ff0_00f0 == 0x0060_0090
        || matches!(body & 0x0fe0_0070, 0x07e0_0050 | 0x07a0_0050 | 0x07c0_0010)
    {
        return 1;
    }
    let rn = (word >> 16) & 15;
    let rd = (word >> 12) & 15;
    let pre = body & (1 << 24) != 0;
    let writeback = body & (1 << 21) != 0;
    let unconditional = word >> 28 == 14;
    match (body >> 25) & 7 {
        5 => {
            if word == 0x1aff_fffd {
                7 // A two-instruction BNE loop, subject to the runtime proof.
            } else if loops::is_shift_count_branch(word) {
                9 // A branch that may close a shift-counting loop.
            } else if loops::is_word_fill_branch(word) {
                10 // A branch that may close a counted RAM fill.
            } else if word >> 28 != 10 {
                1
            } else if body & 0x0180_0000 == 0x0080_0000 {
                7
            } else {
                0
            }
        }
        2 | 3 => {
            let register_shift = body & (1 << 25) != 0 && body & 16 != 0;
            if !writeback && !register_shift && (pre || (rn != 15 && rd != 15 && rn != rd)) {
                2
            } else if unconditional
                && pre
                && writeback
                && !register_shift
                && rn != 15
                && rd != 15
                && rn != rd
            {
                3
            } else {
                0
            }
        }
        4 => {
            let registers = body & 0xffff;
            if unconditional
                && body & (1 << 22) == 0
                && rn != 15
                && registers != 0
                && (!writeback || registers & (1 << rn) == 0)
            {
                3
            } else {
                0
            }
        }
        0 | 1 => {
            let extra_transfer = body & 0x0e00_0090 == 0x0000_0090 && body & 0x60 != 0;
            if extra_transfer && rn != 15 && rd < 14 && (pre || !writeback) {
                if unconditional {
                    3
                } else if pre && !writeback && body & 0x0e10_00d0 != 0x0000_00d0 {
                    // A predicated single transfer with no base update needs
                    // neither a preflight callback nor a writeback rollback.
                    2
                } else {
                    0
                }
            } else if body & 0x0fbf_0fff == 0x010f_0000
                || body & 0x0fb0_fff0 == 0x0120_f000
                || body & 0x0fb0_f000 == 0x0320_f000
                || body & 0x0c10_f000 == 0x0010_f000
                || body & 0x0e00_0090 == 0x0000_0090
            {
                0
            } else {
                1
            }
        }
        _ => 0,
    }
}

struct FaultHook {
    id: usize,
    callback: unsafe fn(Fault, u64, i32, i64, *mut c_void) -> bool,
    data: *mut c_void,
}

#[derive(Clone, Copy)]
enum GuardedTransfer {
    Start {
        word: u32,
        base: u32,
    },
    Span {
        next: u32,
        remaining: usize,
        pointer: *mut u8,
        physical: u64,
    },
}

#[derive(Clone, Copy)]
struct NativePage {
    generation: u64,
    page: u32,
    physical_page: u64,
    mapped_base: u64,
    mapping: Mapping,
    descriptors: [Option<(*const [u8; 4], u32)>; 2],
}

#[derive(Clone, Copy)]
struct PreparedMmio {
    callbacks: Mmio,
    offset: u64,
    project: Option<DeviceReadProjection>,
}

enum NativeAccess {
    Ram(*mut u8, u64),
    Mmio(PreparedMmio),
}

impl NativePage {
    fn prepare(self, offset: usize, len: usize, access: Access) -> Option<NativeAccess> {
        let physical = self.physical_page + offset as u64;
        let mapped_offset = usize::try_from(physical.checked_sub(self.mapped_base)?).ok()?;
        let end = mapped_offset.checked_add(len)?;
        match self.mapping {
            Mapping::Backing {
                bytes,
                permissions,
                ptr,
                physical_base,
            } if end <= bytes => {
                let required = match access {
                    Access::Read => 1,
                    Access::Write => 2,
                    Access::Fetch => 4,
                };
                (permissions & required != 0).then(|| {
                    NativeAccess::Ram(
                        unsafe { ptr.as_ptr().add(mapped_offset) },
                        physical_base + mapped_offset as u64,
                    )
                })
            }
            Mapping::Mmio {
                bytes,
                callbacks,
                project,
            } if access != Access::Fetch && end <= bytes => {
                Some(NativeAccess::Mmio(PreparedMmio {
                    callbacks,
                    offset: mapped_offset as u64,
                    project,
                }))
            }
            _ => None,
        }
    }
}

struct WatchedPage {
    seen: [u64; 64],
    contents: [u8; 4096],
}

impl WatchedPage {
    fn overlaps(&self, first: usize, last: usize) -> bool {
        let first_slot = first / 64;
        let last_slot = last / 64;
        let first_mask = u64::MAX << (first % 64);
        let last_mask = u64::MAX >> (63 - last % 64);
        if first_slot == last_slot {
            return self.seen[first_slot] & first_mask & last_mask != 0;
        }
        self.seen[first_slot] & first_mask != 0
            || self.seen[last_slot] & last_mask != 0
            || self.seen[first_slot + 1..last_slot]
                .iter()
                .any(|&bits| bits != 0)
    }
}

#[cfg(test)]
#[test]
fn watched_byte_ranges_include_only_requested_bits() {
    let mut watched = WatchedPage {
        seen: [0; 64],
        contents: [0; 4096],
    };
    watched.seen[0] = 1 << 62;
    watched.seen[1] = 1 << 1;
    assert!(!watched.overlaps(61, 61));
    assert!(watched.overlaps(61, 62));
    assert!(!watched.overlaps(63, 64));
    assert!(watched.overlaps(63, 65));
    assert!(watched.overlaps(63, 128));
    assert!(!watched.overlaps(66, 4095));
}

#[derive(Default)]
struct WatchedPages {
    pages: RefCell<HashMap<usize, Box<WatchedPage>>>,
    revision: Cell<u64>,
    watch_revision: Cell<u64>,
    changed: RefCell<VecDeque<usize>>,
}

#[derive(Clone, Default)]
pub struct CodeVisibility(Rc<WatchedPages>);

impl CodeVisibility {
    fn watched(&self, pointer: *const u8) -> bool {
        self.0
            .pages
            .borrow()
            .contains_key(&(pointer.addr() & !4095))
    }

    fn overlaps(&self, pointer: *const u8, count: usize) -> bool {
        if count == 0 {
            return false;
        }
        let start = pointer.addr();
        let pages = self.0.pages.borrow();
        let end = start + count;
        ((start & !4095)..end).step_by(4096).any(|page| {
            pages.get(&page).is_some_and(|watched| {
                watched.overlaps(start.saturating_sub(page), (end - page).min(4096) - 1)
            })
        })
    }

    fn note_write(&self, pointer: *const u8, count: usize) {
        if count == 0 {
            return;
        }
        let start = pointer.addr();
        let end = start + count;
        let mut pages = self.0.pages.borrow_mut();
        let mut latest = None;
        for page in ((start & !4095)..end).step_by(4096) {
            let Some(watched) = pages.get_mut(&page) else {
                continue;
            };
            for index in start.saturating_sub(page)..(end - page).min(4096) {
                if watched.seen[index / 64] & (1 << (index % 64)) == 0 {
                    continue;
                }
                let byte = unsafe { pointer.add(page + index - start).read() };
                if std::mem::replace(&mut watched.contents[index], byte) == byte {
                    continue;
                }
                let word = (page + index) & !3;
                if latest == Some(word) {
                    continue;
                }
                latest = Some(word);
                let mut changed = self.0.changed.borrow_mut();
                if changed.len() == 4096 {
                    changed.pop_front();
                }
                changed.push_back(word);
                self.0.revision.set(self.0.revision.get().wrapping_add(1));
            }
        }
    }

    fn changed_since(&self, from: u64, mut visit: impl FnMut(usize)) -> bool {
        let delta = self.0.revision.get().wrapping_sub(from);
        let changed = self.0.changed.borrow();
        if delta > changed.len() as u64 {
            return false;
        }
        changed
            .iter()
            .skip(changed.len() - delta as usize)
            .copied()
            .for_each(&mut visit);
        true
    }
}

#[derive(Clone)]
struct BoardExclusive {
    monitors: exclusive::Shared,
    owner: exclusive::Owner,
}

struct NativeReadWindow {
    address: u32,
    generation: u64,
    descriptors: [Option<(*const [u8; 4], u32)>; 2],
    data: *mut c_void,
    entry_limit: u64,
    value: u64,
    window: ProjectedDeviceWindow,
    additional_reads: u64,
}

/// Most translated backing words have a single guest address. Allocate a
/// list only when another mapping actually translates the same word.
enum CompiledAddresses {
    One(u32),
    Aliases(Vec<u32>),
}

impl CompiledAddresses {
    fn insert(&mut self, address: u32) {
        match self {
            Self::One(first) if *first != address => {
                *self = Self::Aliases(vec![*first, address]);
            }
            Self::Aliases(addresses) if !addresses.contains(&address) => {
                addresses.push(address);
            }
            _ => {}
        }
    }

    fn addresses(&self) -> &[u32] {
        match self {
            Self::One(address) => std::slice::from_ref(address),
            Self::Aliases(addresses) => addresses,
        }
    }
}

struct CoreShared {
    rprofile: Option<rprofile::Profile>,
    exclusive: RefCell<Option<BoardExclusive>>,
    observe_exclusive_stores: Cell<bool>,
    #[cfg(test)]
    inlined_exclusives: Cell<u64>,
    mappings: RefCell<BTreeMap<u64, Mapping>>,
    code_visibility: CodeVisibility,
    seen_revision: Cell<u64>,
    seen_watch_revision: Cell<u64>,
    memory_flushes: Cell<u64>,
    selective_pages: Cell<u64>,
    compiled_words: RefCell<HashMap<usize, CompiledAddresses>>,
    compiled_descriptors: RefCell<HashMap<usize, BTreeSet<u32>>>,
    control_flushes: Cell<u64>,
    mmu_context: Cell<Option<TranslationContext>>,
    contexts: RefCell<Vec<TranslationContext>>,
    fault_hooks: RefCell<Vec<FaultHook>>,
    next_hook_id: Cell<usize>,
    observers: RefCell<Vec<(usize, Hook)>>,
    stopped: Cell<bool>,
    device_accessed: Cell<bool>,
    batching: Cell<bool>,
    read_anchor: Cell<Option<DeviceReadAnchor>>,
    projected_reads: Cell<u64>,
    native_base: Cell<u64>,
    native_budget: Cell<u64>,
    direct_read: UnsafeCell<ffi::DynarmicDirectRead>,
    #[cfg(test)]
    inline_device_reads: Cell<u64>,
    native_read_window: RefCell<Option<NativeReadWindow>>,
    replay_memory: Cell<Option<u32>>,
    replay_mmio: Cell<Option<PreparedMmio>>,
    replay_base: Cell<Option<(u32, u32)>>,
    transfer: Cell<Option<GuardedTransfer>>,
    native_generation: Cell<u64>,
    direct_pages: Cell<*mut *mut u8>,
    touched_pages: RefCell<Vec<u32>>,
    observed_backings: RefCell<Vec<BackingObserver>>,

    native_pages: [Cell<Option<NativePage>>; 24],
    fault: RefCell<Option<ExecutionError>>,
    abort: Cell<Option<(u32, u32, Access)>>,
    inst: Cell<*mut ffi::DynarmicInstance>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TranslationContext {
    control: ffi::DynarmicMemoryControl,
    protection_regions: Option<rprofile::Regions>,
    vfp_enabled: bool,
}

fn contains(range: HookRange, address: u64) -> bool {
    match range {
        HookRange::All => true,
        HookRange::Inclusive { begin, end } => (begin..=end).contains(&address),
    }
}

impl CoreShared {
    fn clear_reservation(&self) -> Result<(), String> {
        if let Some(context) = self.exclusive.borrow().as_ref() {
            context
                .monitors
                .lock()
                .map_err(|_| "exclusive monitors poisoned")?
                .clear(context.owner);
        }
        Ok(())
    }

    fn mark_backing_write(&self, pointer: *mut u8, bytes: usize) {
        let start = pointer.addr();
        let end = start + bytes;
        for range in self.observed_backings.borrow().iter() {
            let base = range.backing.as_ptr().addr();
            let first = start.max(base);
            let limit = end.min(base + range.bytes);
            if first >= limit {
                continue;
            }
            let first_page = (first - base) >> range.page_shift;
            let last_page = (limit - 1 - base) >> range.page_shift;
            for page in first_page..=last_page {
                let word = page / 64;
                // SAFETY: observe_backings requires both bitmaps to cover the backing.
                unsafe {
                    (*range.bitmap.as_ptr().add(word))
                        .fetch_or(1 << (page % 64), Ordering::Relaxed);
                    if let Some(summary) = range.summary {
                        (*summary.as_ptr().add(word / 64))
                            .fetch_or(1 << (word % 64), Ordering::Relaxed);
                    }
                }
            }
        }
    }

    fn refresh_exclusive_stores(&self) -> Result<(), String> {
        let context = self.exclusive.borrow();
        let observing = match &*context {
            Some(context) => context
                .monitors
                .lock()
                .map_err(|_| "exclusive monitors poisoned")?
                .active(),
            None => false,
        };
        if self.observe_exclusive_stores.replace(observing) != observing {
            unsafe { ffi::dynarmic_observe_stores(self.inst.get(), observing) };
            if !cfg!(target_arch = "aarch64") {
                // Without a native store guard, revoke writable inline pages.
                self.clear_inline_pages();
            }
        }
        Ok(())
    }

    fn note_guest_store(&self, physical: u64, size: usize) {
        if !self.observe_exclusive_stores.get() {
            return;
        }
        let context = self.exclusive.borrow();
        let Some(context) = context.as_ref() else {
            return;
        };
        let result = context
            .monitors
            .lock()
            .map_err(|_| "exclusive monitors poisoned".to_string())
            .and_then(|mut monitors| monitors.written(Some((physical, size))));
        if let Err(error) = result {
            self.fail(Fault::WriteProtected, error);
        }
    }

    fn observe_memory(&self, access: Access, addr: u32, size: i32, value: i64) {
        let observers = self
            .observers
            .borrow()
            .iter()
            .filter_map(|&(_, hook)| {
                if let Hook::Memory {
                    callback,
                    data,
                    range,
                    read,
                    write,
                } = hook
                    && ((access == Access::Write && write) || (access == Access::Read && read))
                    && contains(range, addr.into())
                {
                    Some((callback, data))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        for (callback, data) in observers {
            unsafe { callback(access, addr.into(), size, value, data) };
        }
    }

    fn observe_instruction(&self, pc: u32) {
        let single = {
            let hooks = self.observers.borrow();
            match hooks.as_slice() {
                [] => return,
                [(_, hook)] => Some(*hook),
                _ => None,
            }
        };
        let invoke = |hook| {
            if let Hook::Instruction {
                callback,
                data,
                range,
            } = hook
                && contains(range, pc.into())
            {
                unsafe { callback(pc.into(), 4, data) };
            }
        };
        if let Some(hook) = single {
            invoke(hook);
        } else {
            let hooks = self.observers.borrow().clone();
            for (_, hook) in hooks {
                invoke(hook);
                if self.stopped.get() {
                    break;
                }
            }
        }
    }

    fn mapping(&self, address: u64, bytes: usize) -> Option<(Mapping, usize)> {
        if let Some(profile) = &self.rprofile
            && profile.overlaps_tcm(address, bytes)
        {
            return profile.tcm_region(address, bytes);
        }
        let mappings = self.mappings.borrow();
        let (&base, &mapping) = mappings.range(..=address).next_back()?;
        let size = mapping.bytes();
        let offset = usize::try_from(address - base).ok()?;
        (offset.checked_add(bytes)? <= size).then_some((mapping, offset))
    }

    fn has_mmio(&self, address: u64, bytes: usize) -> bool {
        self.mappings.borrow().iter().any(|(&base, mapping)| {
            matches!(mapping, Mapping::Mmio { bytes: length, .. }
                if base < address + bytes as u64 && address < base + *length as u64)
        })
    }

    fn translate(&self, address: u32, access: Access) -> Result<u64, String> {
        self.walk(address, 1, access)?.map_err(|status| {
            format!("ARMv7 memory fault at {address:#010x} (status {status:#04x})")
        })
    }

    fn walk(&self, address: u32, bytes: usize, access: Access) -> Result<Result<u64, u32>, String> {
        self.walk_descriptors(address, bytes, access, |_, _| {})
    }

    fn walk_native_page(
        &self,
        address: u32,
        access: Access,
        descriptor: impl FnMut(*const [u8; 4], u32),
    ) -> Result<Result<u64, u32>, String> {
        if self.rprofile.is_some() {
            let begin = address & !4095;
            // The MPU can change permissions inside a page. Cache a pointer
            // only after checking the entire page for the requested access.
            return self
                .walk_descriptors(begin, 4096, access, descriptor)
                .map(|result| result.map(|physical| physical + u64::from(address - begin)));
        }
        self.walk_descriptors(address, 1, access, descriptor)
    }

    fn walk_descriptors(
        &self,
        address: u32,
        bytes: usize,
        access: Access,
        mut descriptor: impl FnMut(*const [u8; 4], u32),
    ) -> Result<Result<u64, u32>, String> {
        let inst = self.inst.get();
        let mut control = ffi::DynarmicMemoryControl::default();
        unsafe { ffi::dynarmic_memory_control(inst, &mut control) };
        let ffi::DynarmicMemoryControl {
            sctlr,
            ttbr0,
            ttbr1,
            ttbcr,
            dacr,
            fcseidr,
            cpsr,
            ..
        } = control;
        if let Some(profile) = &self.rprofile {
            return Ok(profile
                .mpu
                .borrow()
                .check(sctlr, cpsr, address, bytes, access)
                .map(|()| u64::from(address)));
        }
        if sctlr & 1 == 0 {
            return Ok(Ok(address.into()));
        }
        let cp15 = |crn, crm, opc2| {
            Ok(match (crn, crm, opc2) {
                (1, 0, 0) => sctlr,
                (2, 0, 0) => ttbr0,
                (2, 0, 1) => ttbr1,
                (2, 0, 2) => ttbcr,
                (3, 0, 0) => dacr,
                (13, 0, 0) => fcseidr,
                _ => return Err("Dynarmic MMU control selection is unavailable".into()),
            })
        };
        let mmu = armv7a::mmu::Mmu::read(cp15, cpsr)?;
        mmu.translate(address, access, mmu.user, |physical| {
            let Some((Mapping::Backing { ptr, .. }, offset)) = self.mapping(physical.into(), 4)
            else {
                return Err(format!(
                    "Dynarmic page table outside RAM at {physical:#010x}"
                ));
            };
            let pointer = unsafe { ptr.as_ptr().add(offset).cast::<[u8; 4]>() };
            let value = u32::from_le_bytes(unsafe { pointer.read() });
            descriptor(pointer, value);
            Ok(value)
        })
    }

    fn fetch(&self, address: u32) -> Result<Result<u32, u32>, String> {
        self.fetch_code(address, false)
    }

    fn watch_code_backing(&self, pointer: *const u8) {
        let page = pointer.addr() & !4095;
        let mut pages = self.code_visibility.0.pages.borrow_mut();
        let mut new = false;
        let watched = pages.entry(page).or_insert_with(|| {
            new = true;
            Box::new(WatchedPage {
                seen: [0; 64],
                contents: [0; 4096],
            })
        });
        let byte = pointer.addr() - page;
        let raw = unsafe { pointer.cast::<[u8; 4]>().read() };
        for (index, value) in (byte..byte + 4).zip(raw) {
            watched.seen[index / 64] |= 1 << (index % 64);
            watched.contents[index] = value;
        }
        drop(pages);
        if !new {
            return;
        }
        let revision = self.code_visibility.0.watch_revision.get().wrapping_add(1);
        self.code_visibility.0.watch_revision.set(revision);
        self.seen_watch_revision.set(revision);
        self.revoke_inline_stores(|backing| backing == page);
    }

    fn before_run(&self, inline_allowed: bool) {
        let revision = self.code_visibility.0.watch_revision.get();
        let changed = self.seen_watch_revision.replace(revision) != revision;
        if !inline_allowed {
            self.clear_inline_pages();
        } else if changed {
            let watched = self.code_visibility.0.pages.borrow();
            self.revoke_inline_stores(|page| watched.contains_key(&page));
        }
    }

    fn mapping_changed(&self) {
        self.clear_inline_pages();
        self.compiled_words.borrow_mut().clear();
        self.compiled_descriptors.borrow_mut().clear();
        unsafe { ffi::dynarmic_clear_cache(self.inst.get()) };
    }

    fn flush_changed_code(&self, read_context: bool, fpexc: u32) {
        let revision = self.code_visibility.0.revision.get();
        let mut recycle_contexts = false;
        if read_context {
            let mut control = ffi::DynarmicMemoryControl::default();
            unsafe { ffi::dynarmic_memory_control(self.inst.get(), &mut control) };
            let vfp_enabled = fpexc != 0 && status::vfp_permitted(control.cpacr, control.cpsr);
            control.cpacr = 0;
            // Key the native translations by the inputs the short-descriptor
            // walker uses. Guest reads still see unmodified CP15 registers.
            control.cpsr = u32::from(control.cpsr & 31 == 0x10);
            let rprofile = self.rprofile.is_some();
            control.sctlr &= if rprofile {
                (1 << 17) | (1 << 13) | 1
            } else {
                (1 << 29) | (1 << 25) | (1 << 23) | 1
            };
            control.ttbcr &= (1 << 31) | (1 << 5) | (1 << 4) | 7;
            let split = control.ttbcr & 7;
            control.ttbr0 &= u32::MAX << (14 - split);
            control.ttbr1 &= 0xffff_c000;
            control.fcseidr &= 0xfe00_0000;
            if split == 0 {
                control.ttbr1 = 0;
                control.ttbcr &= !(1 << 5);
            }
            if control.sctlr & 1 == 0 || rprofile {
                control = ffi::DynarmicMemoryControl {
                    cpsr: control.cpsr,
                    sctlr: if rprofile { control.sctlr } else { 0 },
                    ..Default::default()
                };
            }
            let context = TranslationContext {
                control,
                protection_regions: self
                    .rprofile
                    .as_ref()
                    .filter(|_| control.sctlr & 1 != 0)
                    .map(|profile| profile.mpu.borrow().regions),
                vfp_enabled,
            };
            if self.mmu_context.replace(Some(context)) != Some(context) {
                self.clear_inline_pages();
                self.control_flushes.set(self.control_flushes.get() + 1);
                let position = if cfg!(target_arch = "aarch64") {
                    let mut contexts = self.contexts.borrow_mut();
                    let position =
                        contexts
                            .iter()
                            .position(|c| *c == context)
                            .unwrap_or_else(|| {
                                if contexts.len() == 32 {
                                    contexts.clear();
                                    recycle_contexts = true;
                                }
                                let id = contexts.len();
                                contexts.push(context);
                                id
                            });
                    position as u32
                } else {
                    recycle_contexts = true;
                    0
                };
                unsafe {
                    ffi::dynarmic_set_translation_context(self.inst.get(), position, vfp_enabled)
                };
            }
        }
        let previous = self.seen_revision.replace(revision);
        if previous != revision {
            self.memory_flushes.set(self.memory_flushes.get() + 1);
            self.clear_inline_pages();
            if !recycle_contexts {
                let mut words = BTreeSet::new();
                let mut pages = BTreeSet::new();
                let mut compiled = self.compiled_words.borrow_mut();
                let mut descriptors = self.compiled_descriptors.borrow_mut();
                let complete = self.code_visibility.changed_since(previous, |word| {
                    if let Some(targets) = compiled.remove(&word) {
                        words.extend(targets.addresses().iter().copied());
                    }
                    if let Some(targets) = descriptors.remove(&word) {
                        pages.extend(targets);
                    }
                });
                if complete {
                    self.selective_pages
                        .set(self.selective_pages.get() + pages.len() as u64);
                    for word in words {
                        if !pages.contains(&(word & !4095)) {
                            unsafe { ffi::dynarmic_invalidate_cache(self.inst.get(), word, 4) };
                        }
                    }
                    for page in pages {
                        unsafe { ffi::dynarmic_invalidate_cache(self.inst.get(), page, 4096) };
                    }
                    return;
                }
            }
        }
        if previous != revision || recycle_contexts {
            self.mapping_changed();
        }
    }

    fn watch_compiled_descriptor(&self, pointer: *const [u8; 4], address: u32) {
        self.watch_code_backing(pointer.cast());
        self.compiled_descriptors
            .borrow_mut()
            .entry(pointer.addr() & !3)
            .or_default()
            .insert(address & !4095);
    }

    fn watch_compiled_word(&self, pointer: *const u8, address: u32) {
        self.watch_code_backing(pointer);
        self.compiled_words
            .borrow_mut()
            .entry(pointer.addr() & !3)
            .and_modify(|compiled| compiled.insert(address))
            .or_insert(CompiledAddresses::One(address));
    }

    fn fetch_code(&self, address: u32, compiled: bool) -> Result<Result<u32, u32>, String> {
        if compiled
            && self.batching.get()
            && let Some((page, NativeAccess::Ram(pointer, _))) =
                self.native_page(address, 4, Access::Fetch)
        {
            for (descriptor, _) in page.descriptors.into_iter().flatten() {
                self.watch_compiled_descriptor(descriptor, address);
            }
            self.watch_compiled_word(pointer, address);
            return Ok(Ok(u32::from_le_bytes(unsafe {
                pointer.cast::<[u8; 4]>().read()
            })));
        }
        let physical = match self.walk_descriptors(address, 4, Access::Fetch, |p, _| {
            if compiled {
                self.watch_compiled_descriptor(p, address);
            }
        })? {
            Ok(physical) => physical,
            Err(status) => return Ok(Err(status)),
        };
        let Some((
            Mapping::Backing {
                ptr, permissions, ..
            },
            offset,
        )) = self.mapping(physical, 4)
        else {
            return Err(format!(
                "Dynarmic instruction fetch outside RAM at {address:#010x}"
            ));
        };
        if permissions & 4 == 0 {
            return Err(format!(
                "Dynarmic instruction fetch denied at {address:#010x}"
            ));
        }
        let pointer = unsafe { ptr.as_ptr().add(offset) };
        if compiled {
            self.watch_compiled_word(pointer, address);
        }
        let bytes = unsafe { pointer.cast::<[u8; 4]>().read() };
        Ok(Ok(u32::from_le_bytes(bytes)))
    }

    fn fail(&self, fault: Fault, message: String) {
        let mut pending = self.fault.borrow_mut();
        if pending.is_none() {
            *pending = Some(ExecutionError::Memory { fault, message });
        }
        unsafe { ffi::dynarmic_abort_access(self.inst.get()) };
    }

    fn walk_for_guest(
        &self,
        address: u32,
        bytes: usize,
        access: Access,
        protected: Fault,
    ) -> Option<u64> {
        match self.walk(address, bytes, access) {
            Ok(Ok(physical)) => Some(physical),
            Ok(Err(status)) => {
                self.abort.set(Some((address, status, access)));
                unsafe { ffi::dynarmic_abort_access(self.inst.get()) };
                None
            }
            Err(message) => {
                self.fail(protected, message);
                None
            }
        }
    }

    fn cross_page_ram(
        &self,
        address: u32,
        size: usize,
        split: usize,
        access: Access,
        protected: Fault,
        output: Option<u64>,
    ) -> u64 {
        let pieces = [
            (address, split),
            (address.wrapping_add(split as u32), size - split),
        ];
        let mut addresses = [(std::ptr::null_mut(), 0); 2];
        let needed = if output.is_some() { 2 } else { 1 };
        for (index, &(virtual_address, count)) in pieces.iter().enumerate() {
            let Some(physical) = self.walk_for_guest(virtual_address, count, access, protected)
            else {
                return 0;
            };
            let Some((Mapping::Backing { ptr, permissions, physical_base, .. }, offset)) = self.mapping(physical, count)
                .filter(|(mapping, _)| matches!(mapping, Mapping::Backing { permissions, .. } if permissions & needed != 0))
            else {
                self.fail(protected, format!("Dynarmic cross-page {access:?} requires RAM at {virtual_address:#010x}"));
                return 0;
            };
            debug_assert!(permissions & needed != 0);
            addresses[index] = (
                unsafe { ptr.as_ptr().add(offset) },
                physical_base + offset as u64,
            );
        }
        let mut bytes = output.unwrap_or(0).to_le_bytes();
        let (first, second) = bytes.split_at_mut(split);
        for ((&(virtual_address, count), (pointer, physical)), piece) in
            pieces.iter().zip(addresses).zip([first, second])
        {
            let mut value = [0; 8];
            value[..count].copy_from_slice(&piece[..count]);
            self.observe_memory(
                access,
                virtual_address,
                count as i32,
                i64::from_le_bytes(value),
            );
            if output.is_some() {
                unsafe { std::ptr::copy_nonoverlapping(piece.as_ptr(), pointer, count) };
                self.code_visibility.note_write(pointer, count);
                self.mark_backing_write(pointer, count);
                self.note_guest_store(physical, count);
            } else {
                unsafe { std::ptr::copy_nonoverlapping(pointer, piece.as_mut_ptr(), count) };
            }
        }
        u64::from_le_bytes(bytes)
    }

    fn inline_page(&self, address: u32) -> *mut u8 {
        let offset = unsafe { self.direct_pages.get().add((address >> 12) as usize).read() };
        if offset.is_null() {
            return offset;
        }
        offset.wrapping_add((address & !4095) as usize)
    }

    fn clear_inline_page(&self, page: u32) {
        unsafe {
            self.direct_pages
                .get()
                .add(page as usize)
                .write(std::ptr::null_mut())
        }
    }

    fn revoke_inline_stores(&self, watched: impl Fn(usize) -> bool) {
        self.touched_pages.borrow_mut().retain(|&page| {
            let pointer = self.inline_page(page << 12);
            if pointer.is_null() {
                return false;
            }
            if pointer.addr() & 1 != 0 || !watched(pointer.addr() & !4095) {
                return true;
            }
            if cfg!(target_arch = "aarch64") {
                // A new code watch changes store handling only. Reuse the
                // read translation while keeping stores on the checked path.
                let entry = unsafe { self.direct_pages.get().add(page as usize) };
                unsafe { entry.write(entry.read().map_addr(|bias| bias | 1)) };
                true
            } else {
                self.clear_inline_page(page);
                false
            }
        });
    }

    fn clear_inline_pages(&self) {
        self.native_generation
            .set(self.native_generation.get().wrapping_add(1));
        for page in self.touched_pages.borrow_mut().drain(..) {
            self.clear_inline_page(page);
        }
    }

    fn install_inline_page(&self, address: u32, pointer: *mut u8, access: Access) {
        let previous = self.inline_page(address);
        if !previous.is_null() && (previous.addr() & 1 == 0 || access != Access::Write) {
            return;
        }
        let within = (address & 4095) as usize;
        // A watched backing can never gain inline write access. Preserve its
        // existing read entry when the checked store uses the same bytes.
        if access == Access::Write
            && (!cfg!(target_arch = "aarch64")
                || (!previous.is_null()
                    && previous.map_addr(|p| p & !1).wrapping_add(within) == pointer))
            && self.code_visibility.watched(pointer)
        {
            return;
        }
        let page_address = address & !4095;
        let Some((checked, NativeAccess::Ram(backing_page, _))) =
            self.native_page(page_address, 4096, Access::Read)
        else {
            return;
        };
        if backing_page.addr() & 4095 != 0 || unsafe { backing_page.add(within) } != pointer {
            return;
        }
        for (descriptor, _) in checked.descriptors.into_iter().flatten() {
            self.watch_code_backing(descriptor.cast());
        }
        let Mapping::Backing { permissions, .. } = checked.mapping else {
            unreachable!("a checked RAM page must have backing permissions");
        };
        // The first store after rearming must mark the backing before any
        // generated store can bypass the callback. A read only installs a
        // writable page when there are no backing observers.
        let write_allowed = (access == Access::Write || self.observed_backings.borrow().is_empty())
            && !self.code_visibility.watched(pointer)
            && permissions & 2 != 0
            && (self.rprofile.is_none() && access == Access::Write
                || self
                    .native_page(page_address, 4096, Access::Write)
                    .is_some_and(|(page, _)| page.physical_page == checked.physical_page));
        // Only the ARM64 emitter checks the write-protection tag on stores.
        if !cfg!(target_arch = "aarch64") && (!write_allowed || self.observe_exclusive_stores.get())
        {
            return;
        }
        // The JIT adds the full guest address to this bias. Both page
        // starts are aligned, leaving the low bit for write protection.
        let bias = backing_page.wrapping_sub(page_address as usize);
        if bias.is_null() {
            return; // The JIT reserves zero for a missing page.
        }
        let page = address >> 12;
        let slot = unsafe { self.direct_pages.get().add(page as usize) };
        let tagged = bias.map_addr(|p| p | usize::from(!write_allowed));
        unsafe { slot.write(tagged) };
        if previous.is_null() {
            self.touched_pages.borrow_mut().push(page);
        }
    }

    fn write_transfer(word: u32) -> bool {
        if (word >> 25) & 7 <= 1 && word & 0x0e10_00f0 == 0x0000_00d0 {
            return false;
        }
        word & (1 << 20) == 0
    }

    fn guarded_span(&self, word: u32, base: u32) -> (u32, u32) {
        let major = (word >> 25) & 7;
        let before = word & (1 << 24) != 0;
        let up = word & (1 << 23) != 0;
        if major == 4 {
            let len = 4 * (word & 0xffff).count_ones();
            let start = if up {
                base.wrapping_add(4 * u32::from(before))
            } else {
                base.wrapping_sub(len - 4 * u32::from(!before))
            };
            return (start, len);
        }
        let extra = major <= 1;
        let len = if extra {
            if word & 0x0e10_00d0 == 0x0000_00d0 {
                8
            } else if word & 0x60 == 0x40 {
                1
            } else {
                2
            }
        } else if word & (1 << 22) != 0 {
            1
        } else {
            4
        };
        if !before {
            return (base, len);
        }
        let immediate = if extra {
            word & (1 << 22) != 0
        } else {
            word & (1 << 25) == 0
        };
        let offset = if immediate {
            if extra {
                ((word >> 4) & 0xf0) | (word & 15)
            } else {
                word & 4095
            }
        } else {
            let raw = unsafe { ffi::dynarmic_get_reg(self.inst.get(), word & 15) };
            if extra {
                raw
            } else {
                let count = (word >> 7) & 31;
                match (word >> 5) & 3 {
                    0 => raw << count,
                    1 => {
                        if count != 0 {
                            raw >> count
                        } else {
                            0
                        }
                    }
                    2 => ((raw as i32) >> if count == 0 { 31 } else { count }) as u32,
                    _ if count != 0 => raw.rotate_right(count),
                    _ => {
                        let flags = unsafe { ffi::dynarmic_get_cpsr(self.inst.get()) };
                        (raw >> 1) | ((flags & (1 << 29)) << 2)
                    }
                }
            }
        };
        (
            if up {
                base.wrapping_add(offset)
            } else {
                base.wrapping_sub(offset)
            },
            len,
        )
    }

    fn native_access(&self, address: u32, len: usize, access: Access) -> Option<NativeAccess> {
        self.native_page(address, len, access)
            .map(|(_, prepared)| prepared)
    }

    #[inline]
    fn native_page(
        &self,
        address: u32,
        len: usize,
        access: Access,
    ) -> Option<(NativePage, NativeAccess)> {
        let page = address >> 12;
        let offset = (address & 4095) as usize;
        if len > 4096 - offset {
            return None;
        }
        let generation = self.native_generation.get();
        let group = match access {
            Access::Read => 0,
            Access::Write => 8,
            Access::Fetch => 16,
        };
        let slot = &self.native_pages[group + (page as usize & 7)];
        if let Some(cached) = slot.get()
            && cached.generation == generation
            && cached.page == page
            && cached
                .descriptors
                .into_iter()
                .flatten()
                .all(|(pointer, value)| u32::from_le_bytes(unsafe { pointer.read() }) == value)
            && let Some(prepared) = cached.prepare(offset, len, access)
        {
            return Some((cached, prepared));
        }
        let mut descriptors = [None; 2];
        let mut n = 0;
        let physical = self
            .walk_native_page(address, access, |ptr, value| {
                descriptors[n] = Some((ptr, value));
                n += 1;
            })
            .ok()?
            .ok()?;
        let (mapping, mapped_offset) = self.mapping(physical, len)?;
        let cached = NativePage {
            generation,
            page,
            physical_page: physical.checked_sub(offset as u64)?,
            mapped_base: physical - mapped_offset as u64,
            mapping,
            descriptors,
        };
        let prepared = cached.prepare(offset, len, access)?;
        slot.set(Some(cached));
        Some((cached, prepared))
    }

    fn fetch_with_cached_page(&self, address: u32) -> Result<Result<u32, u32>, String> {
        if let Some(NativeAccess::Ram(pointer, _)) = self.native_access(address, 4, Access::Fetch) {
            Ok(Ok(u32::from_le_bytes(unsafe {
                pointer.cast::<[u8; 4]>().read()
            })))
        } else {
            self.fetch(address)
        }
    }

    fn access(&self, address: u32, size: usize, output: Option<u64>) -> u64 {
        self.access_at(address, size, output, None)
    }

    #[inline]
    fn repeat_projected_read(&self, address: u32, elapsed: u64) -> Option<u32> {
        let mut window = self.native_read_window.borrow_mut();
        let cached = window.as_mut()?;
        if cached.address != address
            || cached.generation != self.native_generation.get()
            || self.transfer.get().is_some()
            || !cached
                .descriptors
                .iter()
                .flatten()
                .all(|&(pointer, value)| u32::from_le_bytes(unsafe { pointer.read() }) == value)
        {
            return None;
        }
        let anchor = self.read_anchor.get()?;
        let relative = self
            .native_base
            .get()
            .checked_add(elapsed)?
            .checked_sub(anchor.entries)?;
        if relative >= cached.entry_limit {
            return None;
        }
        let value = match cached.window.countdown_expiry {
            Some(expiry) => (expiry - anchor.clock.after(relative).ok()?.now - 1) as u32,
            None => cached.value as u32,
        };
        cached.additional_reads += 1;
        self.projected_reads.set(self.projected_reads.get() + 1);
        Some(value)
    }

    fn direct_read_cache(
        &self,
        address: u32,
        anchor: DeviceReadAnchor,
        page: NativePage,
        value: u64,
        countdown_expiry: Option<u64>,
        entry_limit: u64,
    ) -> Option<ffi::DynarmicDirectRead> {
        let origin = self
            .native_base
            .get()
            .checked_add(self.native_budget.get())?
            .checked_sub(anchor.entries)?;
        let (value, denominator, numerator, scaled_origin) = match countdown_expiry {
            Some(expiry) => {
                let (numerator, denominator) = anchor.clock.ratio.parts();
                let scaled_origin = origin
                    .checked_mul(denominator)?
                    .checked_add(anchor.clock.phase)?;
                let initial_value = expiry.checked_sub(anchor.clock.now)?.checked_sub(1)?;
                (initial_value, denominator, numerator, scaled_origin)
            }
            None => (value, 0, 0, 0),
        };
        static UNCHANGED_DESCRIPTOR: u32 = 0;
        let inactive = (&raw const UNCHANGED_DESCRIPTOR).cast();
        let [
            (first_descriptor, first_value),
            (second_descriptor, second_value),
        ] = page
            .descriptors
            .map(|descriptor| descriptor.unwrap_or((inactive, 0)));
        Some(ffi::DynarmicDirectRead {
            remaining_floor: origin.saturating_sub(entry_limit),
            value: u64::from(value as u32),
            count: 0,
            first_descriptor: first_descriptor.cast(),
            second_descriptor: second_descriptor.cast(),
            expected: u64::from(first_value) | (u64::from(second_value) << 32),
            denominator,
            numerator,
            scaled_origin,
            address,
        })
    }

    fn project_read(
        &self,
        address: u32,
        page: NativePage,
        prepared: PreparedMmio,
        elapsed: Option<u64>,
    ) -> Option<u64> {
        let project = prepared.project?;
        let anchor = self.read_anchor.get()?;
        let entries = self.native_base.get().checked_add(elapsed?)?;
        let relative = entries.checked_sub(anchor.entries)?;
        self.account_read_window();
        let at = anchor.clock.after(relative).ok()?.now;
        let read = unsafe { project(prepared.offset, 4, at, prepared.callbacks.data) }?;
        if let Some(window) = read.window
            && let Ok(entry_limit) =
                anchor
                    .clock
                    .ratio
                    .budget(anchor.clock.now, anchor.clock.phase, window.before)
            && relative < entry_limit
        {
            self.native_read_window.replace(Some(NativeReadWindow {
                address,
                generation: page.generation,
                descriptors: page.descriptors,
                data: prepared.callbacks.data,
                entry_limit,
                value: read.value,
                window,
                additional_reads: 0,
            }));
            if cfg!(all(target_arch = "aarch64", target_endian = "little"))
                && let Some(cache) = self.direct_read_cache(
                    address,
                    anchor,
                    page,
                    read.value,
                    window.countdown_expiry,
                    entry_limit,
                )
            {
                // JIT execution and its callbacks are on this CPU's Rust thread.
                unsafe { self.direct_read.get().write(cache) };
            }
        }
        Some(read.value)
    }

    fn account_read_window(&self) {
        let cache = unsafe { &mut *self.direct_read.get() };
        let inline = cache.count;
        cache.address = u32::MAX;
        cache.remaining_floor = u64::MAX;
        cache.count = 0;
        if inline != 0 {
            #[cfg(test)]
            self.inline_device_reads
                .set(self.inline_device_reads.get() + inline);
            self.projected_reads
                .set(self.projected_reads.get() + inline);
        }
        if let Some(cached) = self.native_read_window.take() {
            let count = inline + cached.additional_reads;
            if count != 0 {
                unsafe { (cached.window.account)(cached.window.source, count, cached.data) };
            }
        }
    }

    fn access_at(
        &self,
        address: u32,
        size: usize,
        output: Option<u64>,
        elapsed: Option<u64>,
    ) -> u64 {
        if self.batching.get() {
            let access = if output.is_some() {
                Access::Write
            } else {
                Access::Read
            };
            let transfer = self.transfer.take();
            let len = match transfer {
                Some(GuardedTransfer::Start { word, .. }) if word & 0x0e00_0000 == 0x0800_0000 => {
                    4 * (word & 0xffff).count_ones() as usize
                }
                Some(GuardedTransfer::Start { word, .. }) if word & 0x0e10_00d0 == 0x0000_00d0 => 8,
                _ => size,
            };
            let memory = match transfer {
                Some(GuardedTransfer::Span {
                    next,
                    remaining,
                    pointer,
                    physical,
                }) if next == address && size <= remaining => Some((pointer, physical, remaining)),
                _ => match self.native_page(address, len, access) {
                    Some((_, NativeAccess::Ram(pointer, physical))) => {
                        Some((pointer, physical, len))
                    }
                    Some((page, NativeAccess::Mmio(prepared))) if len == size => {
                        if output.is_none()
                            && size == 4
                            && transfer.is_none()
                            && let Some(value) = self.project_read(address, page, prepared, elapsed)
                        {
                            self.projected_reads.set(self.projected_reads.get() + 1);
                            return value;
                        }
                        self.replay_mmio.set(Some(prepared));
                        None
                    }
                    _ => None,
                },
            };
            if let Some((pointer, physical, room)) = memory
                && (output.is_none() || !self.code_visibility.overlaps(pointer, room))
            {
                if room > size {
                    self.transfer.set(Some(GuardedTransfer::Span {
                        next: address.wrapping_add(size as u32),
                        remaining: room - size,
                        pointer: unsafe { pointer.add(size) },
                        physical: physical + size as u64,
                    }));
                }
                if let Some(value) = output {
                    unsafe {
                        std::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), pointer, size)
                    };
                    self.mark_backing_write(pointer, size);
                    self.note_guest_store(physical, size);
                }
                if room == size {
                    self.install_inline_page(address, pointer, access);
                }
                if output.is_some() {
                    return 0;
                }
                let mut value = [0; 8];
                unsafe { std::ptr::copy_nonoverlapping(pointer, value.as_mut_ptr(), size) };
                return u64::from_le_bytes(value);
            }
            if let Some(GuardedTransfer::Start { word, base }) = transfer {
                self.replay_base.set(Some(((word >> 16) & 15, base)));
            }
            self.replay_memory.set(Some(address));
            unsafe { ffi::dynarmic_abort_access(self.inst.get()) };
            return 0;
        }
        if self.fault.borrow().is_some() || self.abort.get().is_some() {
            return 0;
        }
        let (access, unmapped, protected) = if output.is_some() {
            (Access::Write, Fault::WriteUnmapped, Fault::WriteProtected)
        } else {
            (Access::Read, Fault::ReadUnmapped, Fault::ReadProtected)
        };
        let room = 4096 - (address as usize & 4095);
        if size > room {
            return self.cross_page_ram(address, size, room, access, protected, output);
        }
        let Some(physical) = self.walk_for_guest(address, size, access, protected) else {
            return 0;
        };
        let mut mapped = self.mapping(physical, size);
        if mapped.is_none() {
            let hooks: Vec<_> = self
                .fault_hooks
                .borrow()
                .iter()
                .map(|h| (h.callback, h.data))
                .collect();
            for (callback, data) in hooks {
                if unsafe {
                    callback(
                        unmapped,
                        address.into(),
                        size as i32,
                        output.unwrap_or(0) as i64,
                        data,
                    )
                } {
                    mapped = self.mapping(physical, size);
                    if mapped.is_some() {
                        break;
                    }
                }
            }
        }
        let Some((mapping, offset)) = mapped else {
            self.fail(
                unmapped,
                format!(
                    "Dynarmic unmapped {access:?} at {address:#010x} (physical {physical:#010x})"
                ),
            );
            return 0;
        };
        if let Mapping::Backing { permissions, .. } = mapping {
            let required = if output.is_some() { 2 } else { 1 };
            if permissions & required == 0 {
                self.fail(
                    protected,
                    format!("Dynarmic protected {access:?} at {address:#010x}"),
                );
                return 0;
            }
        }
        self.observe_memory(access, address, size as i32, output.unwrap_or(0) as i64);
        match (mapping, output) {
            (Mapping::Backing { ptr, .. }, None) => {
                let mut bytes = [0; 8];
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        ptr.as_ptr().add(offset),
                        bytes.as_mut_ptr(),
                        size,
                    )
                };
                u64::from_le_bytes(bytes)
            }
            (
                Mapping::Backing {
                    ptr, physical_base, ..
                },
                Some(value),
            ) => {
                let pointer = unsafe { ptr.as_ptr().add(offset) };
                unsafe {
                    std::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), pointer, size)
                };
                self.code_visibility.note_write(pointer, size);
                self.mark_backing_write(pointer, size);
                self.note_guest_store(physical_base + offset as u64, size);
                0
            }
            (Mapping::Mmio { callbacks, .. }, value) => {
                self.read_anchor.set(None);
                // A device operation can create a new deadline inside this slice.
                self.device_accessed.set(true);
                if let Some(value) = value {
                    unsafe { (callbacks.write)(offset as u64, size as u32, value, callbacks.data) };
                    0
                } else {
                    unsafe { (callbacks.read)(offset as u64, size as u32, callbacks.data) }
                }
            }
        }
    }
}

pub struct Core {
    inst: *mut ffi::DynarmicInstance,
    identity: Identity,
    model: Model,
    shared: Box<CoreShared>,
    last_progress: Cell<u64>,
    pending_anchor: Cell<Option<DeviceReadAnchor>>,

    run_limit: Cell<Option<u64>>,
    tightened: Cell<bool>,
    scheduler_hook: Cell<Option<HookId>>,
    board_owns_instructions: Cell<bool>,
    irq: Cell<bool>,
    exception_entries: Cell<armv7a::Entries>,
    loops: loops::Loops,
    owned: RefCell<BTreeMap<u64, Backing>>,
    banks: RefCell<status::Banks>,
    fpexc: Cell<u32>,
}

impl Core {
    pub fn new(model: Model) -> Result<Self, String> {
        Self::new_with_code_visibility(model, CodeVisibility::default())
    }

    pub fn new_with_code_visibility(
        model: Model,
        code_visibility: CodeVisibility,
    ) -> Result<Self, String> {
        let profile = match model {
            Model::CortexA7 => None,
            Model::CortexR5F => Some(RProfileConfig::default()),
            _ => return Err("Dynarmic requires a Cortex-A7 or Cortex-R5F processor".into()),
        };
        Self::construct(model, code_visibility, profile)
    }

    pub fn new_rprofile(
        config: RProfileConfig,
        code_visibility: CodeVisibility,
    ) -> Result<Self, String> {
        Self::construct(Model::CortexR5F, code_visibility, Some(config))
    }

    fn construct(
        model: Model,
        code_visibility: CodeVisibility,
        profile: Option<RProfileConfig>,
    ) -> Result<Self, String> {
        let shared = Box::new(CoreShared {
            rprofile: profile.map(rprofile::Profile::new).transpose()?,
            exclusive: RefCell::new(None),
            observe_exclusive_stores: Cell::new(false),
            #[cfg(test)]
            inlined_exclusives: Cell::new(0),
            mappings: RefCell::new(BTreeMap::new()),
            code_visibility,
            seen_revision: Cell::new(0),
            seen_watch_revision: Cell::new(0),
            memory_flushes: Cell::new(0),
            selective_pages: Cell::new(0),
            compiled_words: RefCell::new(HashMap::new()),
            compiled_descriptors: RefCell::new(HashMap::new()),
            control_flushes: Cell::new(0),
            mmu_context: Default::default(),
            contexts: Default::default(),
            fault_hooks: RefCell::new(Vec::new()),
            next_hook_id: Cell::new(0),
            observers: RefCell::new(Vec::new()),
            stopped: Cell::new(false),
            device_accessed: Cell::new(false),
            batching: Cell::new(false),
            read_anchor: Cell::new(None),
            projected_reads: Cell::new(0),
            native_base: Cell::new(0),
            native_budget: Cell::new(0),
            direct_read: UnsafeCell::new(ffi::DynarmicDirectRead::DISABLED),
            #[cfg(test)]
            inline_device_reads: Cell::new(0),
            native_read_window: RefCell::new(None),
            replay_memory: Cell::new(None),
            replay_mmio: Cell::new(None),
            replay_base: Cell::new(None),
            transfer: Cell::new(None),
            native_generation: Cell::new(0),
            direct_pages: Cell::new(std::ptr::null_mut()),
            touched_pages: RefCell::new(Vec::with_capacity(16)),
            observed_backings: RefCell::new(Vec::new()),

            native_pages: Default::default(),
            fault: RefCell::new(None),
            abort: Cell::new(None),
            inst: Cell::new(std::ptr::null_mut()),
        });
        let user_data = (&*shared as *const CoreShared as *mut CoreShared).cast();

        let cb = ffi::DynarmicCallbacks {
            user_data,
            inline_read32: if cfg!(all(target_arch = "aarch64", target_endian = "little")) {
                shared.direct_read.get()
            } else {
                std::ptr::null_mut()
            },
            batch_instruction,
            is_counted_loop: loops::is_counted_loop,
            guard_transfer: Self::guard_transfer,
            read_code: Self::read_code,
            read: Self::read,
            timed_read32: Self::timed_read32,
            write: Self::write,
        };

        let inst = unsafe { ffi::dynarmic_create(&cb, model == Model::CortexR5F) };
        if inst.is_null() {
            return Err("failed to create Dynarmic JIT instance".into());
        }
        shared.inst.set(inst);
        shared
            .direct_pages
            .set(unsafe { ffi::dynarmic_data_pages(inst) });
        unsafe { ffi::dynarmic_set_cpsr(inst, 0xd3) };
        if profile.is_some_and(|config| config.high_vectors) {
            unsafe { ffi::dynarmic_set_cp15(inst, 0, 1, 0, 0, 1 << 13) };
        }

        Ok(Self {
            inst,
            identity: Identity::allocate(),
            model,
            shared,
            last_progress: Cell::new(0),
            pending_anchor: Cell::new(None),

            run_limit: Cell::new(None),
            tightened: Cell::new(false),
            scheduler_hook: Cell::new(None),
            board_owns_instructions: Cell::new(false),
            irq: Cell::new(false),
            exception_entries: Cell::new(armv7a::Entries::default()),
            loops: loops::Loops::default(),
            owned: RefCell::new(BTreeMap::new()),
            banks: RefCell::new(status::Banks::default()),
            fpexc: Cell::new(0),
        })
    }

    fn install_mmio(
        &self,
        address: u64,
        bytes: usize,
        callbacks: Mmio,
        project: Option<DeviceReadProjection>,
    ) -> Result<(), String> {
        if self
            .shared
            .rprofile
            .as_ref()
            .is_some_and(|profile| profile.overlaps_tcm(address, bytes))
        {
            return Err("TCM overlay of MMIO is unmodeled".into());
        }
        self.shared.mapping_changed();
        self.shared.mappings.borrow_mut().insert(
            address,
            Mapping::Mmio {
                bytes,
                callbacks,
                project,
            },
        );
        Ok(())
    }

    pub fn set_cache_ids(&self, ids: [u32; 3]) {
        unsafe {
            ffi::dynarmic_set_cache_ids(self.inst, ids[0], ids[1], ids[2]);
        }
    }

    fn complete_mmio(
        &self,
        pc: u32,
        word: u32,
        address: u32,
        prepared: Option<PreparedMmio>,
    ) -> bool {
        // Native execution intercepted an actual single transfer before
        // any effects. Only forms without writeback or a PC write can finish here.
        let register = (word >> 12) & 15;
        if word & 0x0d20_0000 != 0x0500_0000
            || register == 15
            || unsafe { ffi::dynarmic_get_cpsr(self.inst) } & (1 << 9) != 0
        {
            return false;
        }
        let width = if word & (1 << 22) == 0 { 4 } else { 1 };
        if width > 4096 - (address as usize & 4095) {
            return false;
        }
        let read = word & (1 << 20) != 0;
        let PreparedMmio {
            callbacks, offset, ..
        } = if let Some(prepared) = prepared {
            prepared
        } else {
            let access = if read { Access::Read } else { Access::Write };
            let Some(NativeAccess::Mmio(prepared)) =
                self.shared.native_access(address, width, access)
            else {
                return false;
            };
            prepared
        };
        self.shared.read_anchor.set(None);
        self.shared.device_accessed.set(true);
        self.tightened.set(false);
        if read {
            let value = unsafe { (callbacks.read)(offset, width as u32, callbacks.data) };
            let mask = if width == 1 { 0xff } else { u32::MAX };
            unsafe {
                ffi::dynarmic_set_reg(self.inst, register, value as u32 & mask);
            }
        } else {
            let mask = if width == 1 { 0xff } else { u32::MAX };
            let value = unsafe { ffi::dynarmic_get_reg(self.inst, register) } & mask;
            unsafe { (callbacks.write)(offset, width as u32, u64::from(value), callbacks.data) };
        }
        unsafe { ffi::dynarmic_set_reg(self.inst, 15, pc.wrapping_add(4)) };
        self.last_progress.set(self.last_progress.get() + 1);

        if self.tightened.get() {
            self.shared.device_accessed.set(false);
        }
        true
    }

    unsafe extern "C" fn guard_transfer(user_data: *mut c_void, word: u32, base: u32) {
        let shared = unsafe { &*user_data.cast::<CoreShared>() };
        let (first, len) = shared.guarded_span(word, base);
        let last = first.wrapping_add(len - 1);
        if first >> 12 != last >> 12 {
            shared.clear_inline_page(first >> 12);
            shared.clear_inline_page(last >> 12);
        } else {
            let slot = shared.inline_page(first).addr();
            if slot != 0
                && (!CoreShared::write_transfer(word)
                    || slot & 1 == 0 && !shared.observe_exclusive_stores.get())
            {
                shared.transfer.set(None);
                return;
            }
        }
        // A multi-transfer has to go through the common memory path so
        // each word shares the instruction's span and permission checks.
        unsafe {
            let cache = &mut *shared.direct_read.get();
            cache.address = u32::MAX;
            cache.remaining_floor = u64::MAX;
        };
        shared
            .transfer
            .set(Some(GuardedTransfer::Start { word, base }));
    }

    unsafe extern "C" fn read_code(user_data: *mut c_void, address: u32, output: *mut u32) -> bool {
        let shared = unsafe { &*user_data.cast::<CoreShared>() };
        if let Ok(Ok(word)) = shared.fetch_code(address, true) {
            unsafe { *output = word };
            true
        } else {
            false
        }
    }

    unsafe extern "C" fn read(user_data: *mut c_void, address: u32, size: u32) -> u64 {
        let shared = unsafe { &*user_data.cast::<CoreShared>() };
        shared.access(address, size as usize, None)
    }

    unsafe extern "C" fn timed_read32(user_data: *mut c_void, address: u32, elapsed: u64) -> u32 {
        let shared = unsafe { &*user_data.cast::<CoreShared>() };
        if let Some(value) = shared.repeat_projected_read(address, elapsed) {
            return value;
        }
        shared.access_at(address, 4, None, Some(elapsed)) as u32
    }

    unsafe extern "C" fn write(user_data: *mut c_void, address: u32, size: u32, value: u64) {
        let shared = unsafe { &*user_data.cast::<CoreShared>() };
        shared.access(address, size as usize, Some(value));
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        if !self.inst.is_null() {
            unsafe { ffi::dynarmic_destroy(self.inst) };
        }
    }
}

struct ResetRunLimit<'a>(&'a Cell<Option<u64>>, &'a Cell<Option<DeviceReadAnchor>>);
impl Drop for ResetRunLimit<'_> {
    fn drop(&mut self) {
        self.0.set(None);
        self.1.set(None);
    }
}

impl Cpu for Core {
    fn identity(&self) -> Identity {
        self.identity
    }

    fn model(&self) -> Model {
        self.model
    }

    fn checkpoint_backings(&self) -> Result<Vec<(NonNull<u8>, usize)>, String> {
        Ok(self
            .shared
            .mappings
            .borrow()
            .values()
            .filter_map(|mapping| match *mapping {
                Mapping::Backing { ptr, bytes, .. } => Some((ptr, bytes)),
                Mapping::Mmio { .. } => None,
            })
            .collect())
    }

    fn set_armv7a_affinity(&self, mpidr: u32) -> Result<(), String> {
        unsafe { ffi::dynarmic_set_cp15(self.inst, 0, 0, 0, 5, mpidr) };
        Ok(())
    }

    fn set_armv7a_owner_exclusives(&self, on: bool) -> Result<(), String> {
        if on {
            self.board_owns_instructions.set(true);
            Ok(())
        } else {
            Err("Dynarmic requires exact execution policy".into())
        }
    }

    fn set_armv7a_exclusive_context(&self, monitors: exclusive::Shared, owner: exclusive::Owner) {
        self.shared
            .exclusive
            .replace(Some(BoardExclusive { monitors, owner }));
    }

    fn exclusive_domain(&self) -> Option<exclusive::Shared> {
        self.shared
            .exclusive
            .borrow()
            .as_ref()
            .map(|context| context.monitors.clone())
    }

    fn exclusive_address(&self, mapped: u64, bytes: usize, access: Access) -> Result<u64, String> {
        let required = if access == Access::Write { 2 } else { 1 };
        match self.shared.mapping(mapped, bytes) {
            Some((
                Mapping::Backing {
                    physical_base,
                    permissions,
                    ..
                },
                offset,
            )) if permissions & required != 0 => Ok(physical_base + offset as u64),
            _ => Err(format!(
                "exclusive access requires permitted RAM at {mapped:#x}"
            )),
        }
    }

    fn features(&self) -> Features {
        Features {
            architectural_exits: true,
            instruction_limit: true,
            instruction_progress: true,
            tighten_instruction_limit: true,
            guest_clock: self.shared.rprofile.is_some(),
            ..Default::default()
        }
    }

    fn supports_projected_device_reads(&self) -> bool {
        cfg!(target_arch = "aarch64")
    }

    fn set_guest_clock(&self, clock: Clock) -> Result<(), String> {
        let profile = self
            .shared
            .rprofile
            .as_ref()
            .ok_or("Dynarmic has no R-profile clock")?;
        profile.clock.set(Some(clock));
        Ok(())
    }

    fn set_device_read_anchor(&self, anchor: Option<DeviceReadAnchor>) {
        if self.run_limit.get().is_some() {
            self.shared.read_anchor.set(anchor);
        } else {
            self.pending_anchor.set(anchor);
        }
    }

    fn set_interrupt(&self, line: Interrupt, asserted: bool) -> Result<(), String> {
        match line {
            Interrupt::Irq => self.irq.set(asserted),
            Interrupt::Fiq if asserted => return Err("Dynarmic prototype has no routed FIQ".into()),
            Interrupt::Fiq => (),
        }
        Ok(())
    }

    fn exception_entries(&self) -> Option<armv7a::Entries> {
        Some(self.exception_entries.get())
    }

    fn read_register(&self, register: Register, out: &mut [u8]) -> Result<usize, String> {
        let val = match register {
            Register::Pc => unsafe { ffi::dynarmic_get_reg(self.inst, 15) },
            Register::Sp => unsafe { ffi::dynarmic_get_reg(self.inst, 13) },
            Register::Arm32(arm::Register::R(n @ 0..=15)) => unsafe {
                ffi::dynarmic_get_reg(self.inst, u32::from(n))
            },
            Register::Arm32(arm::Register::Cpsr) | Register::Pstate => unsafe {
                ffi::dynarmic_get_cpsr(self.inst)
            },
            Register::Arm32(arm::Register::Spsr) => self.spsr()?,
            Register::Arm32(arm::Register::Fpscr) => unsafe { ffi::dynarmic_get_fpscr(self.inst) },
            Register::Arm32(arm::Register::Cpacr) => unsafe {
                ffi::dynarmic_get_cp15(self.inst, 0, 1, 0, 2)
            },
            Register::Arm32(arm::Register::Fpexc) => self.fpexc.get(),
            _ => return Err(format!("unsupported register {register:?}")),
        };
        let bytes = val.to_le_bytes();
        let len = bytes.len().min(out.len());
        out[..len].copy_from_slice(&bytes[..len]);
        Ok(len)
    }

    fn write_register(&self, register: Register, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() < 4 {
            return Err("register write requires at least 4 bytes".into());
        }
        let val = u32::from_le_bytes(bytes[..4].try_into().unwrap());
        match register {
            Register::Pc => unsafe { ffi::dynarmic_set_reg(self.inst, 15, val) },
            Register::Sp => unsafe { ffi::dynarmic_set_reg(self.inst, 13, val) },
            Register::Arm32(arm::Register::R(n @ 0..=15)) => unsafe {
                ffi::dynarmic_set_reg(self.inst, u32::from(n), val)
            },
            Register::Arm32(arm::Register::Cpsr) | Register::Pstate => self.set_cpsr(val)?,
            Register::Arm32(arm::Register::Spsr) => self.set_spsr(val)?,
            Register::Arm32(arm::Register::Fpscr) => unsafe {
                ffi::dynarmic_set_fpscr(self.inst, val)
            },
            Register::Arm32(arm::Register::Fpexc) => self.set_fpexc(val)?,
            _ => return Err(format!("unsupported register {register:?}")),
        }
        Ok(())
    }

    fn coprocessor(&self, register: arm::Coprocessor, value: Option<u64>) -> Result<u64, String> {
        if self.shared.rprofile.is_some()
            && let Some(register) = rprofile::SystemRegister::decode(register)
        {
            let value = value
                .map(u32::try_from)
                .transpose()
                .map_err(|_| "CP15 value exceeds 32 bits")?;
            return self.rprofile_register(register, value).map(u64::from);
        }
        if register.cp != 15 || register.wide {
            return Err("Dynarmic only supports modeled 32-bit system accesses".into());
        }
        if let Some(val) = value {
            let word = u32::try_from(val).map_err(|_| "CP15 value exceeds 32 bits")?;
            unsafe {
                ffi::dynarmic_set_cp15(
                    self.inst,
                    register.opc1,
                    register.crn,
                    register.crm,
                    register.opc2,
                    word,
                );
            }
            Ok(val)
        } else {
            let val = unsafe {
                ffi::dynarmic_get_cp15(
                    self.inst,
                    register.opc1,
                    register.crn,
                    register.crm,
                    register.opc2,
                )
            };
            Ok(val as u64)
        }
    }

    fn read_memory(&self, address: u64, bytes: &mut [u8]) -> Result<(), String> {
        let (mapping, offset) = self
            .shared
            .mapping(address, bytes.len())
            .ok_or_else(|| format!("unmapped read at {address:#x}"))?;
        match mapping {
            Mapping::Backing { ptr, .. } => unsafe {
                std::ptr::copy_nonoverlapping(
                    ptr.as_ptr().add(offset),
                    bytes.as_mut_ptr(),
                    bytes.len(),
                );
            },
            Mapping::Mmio { callbacks, .. } => {
                for (i, b) in bytes.iter_mut().enumerate() {
                    *b = unsafe { (callbacks.read)((offset + i) as u64, 1, callbacks.data) as u8 };
                }
            }
        }
        Ok(())
    }

    fn translate(&self, address: u64, access: Access) -> Result<u64, String> {
        let address =
            u32::try_from(address).map_err(|_| "Dynarmic virtual address exceeds 32 bits")?;
        self.shared.translate(address, access)
    }

    fn read_virtual(&self, address: u64, access: Access, bytes: &mut [u8]) -> Result<(), String> {
        let mut address = address;
        let mut bytes = bytes;
        let required = match access {
            Access::Read => 1,
            Access::Write => 2,
            Access::Fetch => 4,
        };
        while !bytes.is_empty() {
            let room = 4096 - (address as usize & 4095);
            let (chunk, remaining) = bytes.split_at_mut(room.min(bytes.len()));
            let address32 =
                u32::try_from(address).map_err(|_| "Dynarmic virtual address exceeds 32 bits")?;
            let translated =
                self.shared
                    .walk(address32, chunk.len(), access)?
                    .map_err(|status| {
                        format!("ARMv7 memory fault at {address:#010x} (status {status:#04x})")
                    })?;
            let Some((
                Mapping::Backing {
                    ptr, permissions, ..
                },
                offset,
            )) = self.shared.mapping(translated, chunk.len())
            else {
                return Err(format!(
                    "Dynarmic virtual inspection requires RAM at {address:#x}"
                ));
            };
            if permissions & required == 0 {
                return Err(format!("Dynarmic virtual access denied at {address:#x}"));
            }
            unsafe {
                std::ptr::copy_nonoverlapping(
                    ptr.as_ptr().add(offset),
                    chunk.as_mut_ptr(),
                    chunk.len(),
                );
            }
            address += chunk.len() as u64;
            bytes = remaining;
        }
        Ok(())
    }

    fn write_memory(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
        if bytes.is_empty() {
            return Ok(());
        }
        let (mapping, offset) = self
            .shared
            .mapping(address, bytes.len())
            .ok_or_else(|| format!("unmapped write at {address:#x}"))?;
        match mapping {
            Mapping::Backing { ptr, .. } => {
                let pointer = unsafe { ptr.as_ptr().add(offset) };
                unsafe {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
                    ffi::dynarmic_invalidate_cache(self.inst, address as u32, bytes.len());
                }
                self.shared.code_visibility.note_write(pointer, bytes.len());
                self.shared.mark_backing_write(pointer, bytes.len());
            }
            Mapping::Mmio { callbacks, .. } => {
                for (i, &b) in bytes.iter().enumerate() {
                    unsafe {
                        (callbacks.write)((offset + i) as u64, 1, u64::from(b), callbacks.data)
                    };
                }
            }
        }
        Ok(())
    }

    fn memory_regions(&self) -> Result<Vec<MemoryRegion>, String> {
        Ok(self
            .shared
            .mappings
            .borrow()
            .iter()
            .map(|(&begin, &mapping)| MemoryRegion {
                begin,
                end: begin + mapping.bytes() as u64 - 1,
                permissions: mapping.permissions(),
            })
            .collect())
    }

    fn rprofile_identity(&self, value: Option<u32>) -> Result<u32, String> {
        let profile = self
            .shared
            .rprofile
            .as_ref()
            .ok_or("Dynarmic has no R-profile")?;
        if let Some(value) = value {
            if self.run_limit.get().is_some() {
                return Err("identity changes require a stopped processor".into());
            }
            profile.cp14_identity.set(value);
        }
        Ok(profile.cp14_identity.get())
    }

    unsafe fn map_backing(
        &self,
        address: u64,
        bytes: usize,
        permissions: u32,
        backing: NonNull<u8>,
    ) -> Result<(), String> {
        unsafe { self.map_physical(address, address, bytes, permissions, backing) }
    }

    unsafe fn map_physical(
        &self,
        address: u64,
        physical: u64,
        bytes: usize,
        permissions: u32,
        backing: NonNull<u8>,
    ) -> Result<(), String> {
        ram_window_end(address, physical, bytes)?;
        self.shared.mapping_changed();
        self.shared.mappings.borrow_mut().insert(
            address,
            Mapping::Backing {
                bytes,
                permissions,
                ptr: backing,
                physical_base: physical,
            },
        );

        Ok(())
    }

    unsafe fn replace_physical(
        &self,
        address: u64,
        physical: u64,
        bytes: usize,
        permissions: u32,
        backing: NonNull<u8>,
    ) -> Result<(), String> {
        if self.run_limit.get().is_some() {
            return Err("physical replacement requires a stopped Dynarmic processor".into());
        }
        let end = ram_window_end(address, physical, bytes)?;
        let mut maps = self.shared.mappings.borrow_mut();
        let mut previous = Vec::new();
        let mut pieces = Vec::new();
        let mut cursor = address;
        while cursor < end {
            let Some((
                &base,
                &Mapping::Backing {
                    bytes,
                    permissions,
                    ptr,
                    physical_base,
                },
            )) = maps.range(..=cursor).next_back()
            else {
                return Err("physical replacement has no complete external RAM underlay".into());
            };
            let limit = base + bytes as u64;
            if limit <= cursor {
                return Err("physical replacement has no complete external RAM underlay".into());
            }
            previous.push(base);
            if base < address {
                pieces.push((
                    base,
                    Mapping::Backing {
                        bytes: (address - base) as usize,
                        permissions,
                        ptr,
                        physical_base,
                    },
                ));
            }
            if end < limit {
                let offset = (end - base) as usize;
                pieces.push((
                    end,
                    Mapping::Backing {
                        bytes: (limit - end) as usize,
                        permissions,
                        ptr: unsafe { ptr.add(offset) },
                        physical_base: physical_base + offset as u64,
                    },
                ));
            }
            cursor = limit;
        }
        for base in previous {
            maps.remove(&base);
        }
        maps.extend(pieces);
        maps.insert(
            address,
            Mapping::Backing {
                bytes,
                permissions,
                ptr: backing,
                physical_base: physical,
            },
        );
        drop(maps);
        self.shared.mapping_changed();
        Ok(())
    }

    fn map_zeroed(&self, address: u64, bytes: usize, permissions: u32) -> Result<(), String> {
        let backing = Backing::aligned_zeros(bytes, 4096)?;
        let ptr = NonNull::new(backing.as_ptr()).ok_or("null Dynarmic RAM backing")?;
        unsafe { self.map_backing(address, bytes, permissions, ptr)? };
        self.owned.borrow_mut().insert(address, backing);
        Ok(())
    }

    unsafe fn map_mmio(&self, address: u64, bytes: usize, callbacks: Mmio) -> Result<(), String> {
        self.install_mmio(address, bytes, callbacks, None)
    }

    unsafe fn map_mmio_projected(
        &self,
        address: u64,
        bytes: usize,
        callbacks: Mmio,
        _fast_reads: Vec<FastMmioRead>,
        project: DeviceReadProjection,
    ) -> Result<(), String> {
        self.install_mmio(address, bytes, callbacks, Some(project))
    }

    fn unmap(&self, address: u64, bytes: usize) -> Result<(), String> {
        if !self
            .shared
            .mappings
            .borrow()
            .get(&address)
            .is_some_and(|mapping| mapping.bytes() == bytes)
        {
            return Err("Dynarmic can only unmap an entire mapped range".into());
        }
        self.shared.mapping_changed();
        self.shared.mappings.borrow_mut().remove(&address);
        self.owned.borrow_mut().remove(&address);
        self.invalidate_code(Some((address, address + bytes as u64)))?;
        Ok(())
    }

    fn protect(&self, _address: u64, _bytes: usize, _permissions: u32) -> Result<(), String> {
        Err("Dynarmic cannot change mapping protection".into())
    }

    unsafe fn backing_written(
        &self,
        _physical: u64,
        backing: NonNull<u8>,
        bytes: usize,
    ) -> Result<(), String> {
        // The next native entry consumes changed instructions and descriptors
        // through their watched backing, including every translated alias.
        self.shared
            .code_visibility
            .note_write(backing.as_ptr(), bytes);
        Ok(())
    }

    fn invalidate_code(&self, range: Option<(u64, u64)>) -> Result<(), String> {
        if let Some((begin, end)) = range {
            unsafe {
                ffi::dynarmic_invalidate_cache(
                    self.inst,
                    begin as u32,
                    (end.saturating_sub(begin)) as usize,
                );
            }
        } else {
            unsafe { ffi::dynarmic_clear_cache(self.inst) };
        }
        Ok(())
    }

    fn run(&self, request: Run) -> Result<Exit, ExecutionError> {
        if self.run_limit.get().is_some() {
            return Err("nested Dynarmic execution is unsupported".into());
        }
        self.last_progress.set(0);
        let stable = self.loops.stable.replace(false);
        if request.watchdog_us != 0 {
            return Err("Dynarmic does not support a host watchdog".into());
        }
        if request.instructions == Some(0) {
            return Ok(Exit::Stopped);
        }
        let begin = u32::try_from(request.begin).map_err(|_| "Dynarmic start exceeds 32 bits")?;
        unsafe { ffi::dynarmic_set_reg(self.inst, 15, begin & !1) };
        self.run_limit
            .set(Some(request.instructions.unwrap_or(usize::MAX) as u64));
        self.shared.read_anchor.set(self.pending_anchor.take());
        let _reset_run_limit = ResetRunLimit(&self.run_limit, &self.shared.read_anchor);
        let remaining = || {
            self.run_limit
                .get()
                .unwrap()
                .saturating_sub(self.last_progress.get())
        };
        self.shared.device_accessed.set(false);
        self.shared.stopped.set(false);
        let allow_batch = stable
            && request.until.is_none()
            && self.shared.observers.borrow().is_empty()
            && self.shared.fault_hooks.borrow().is_empty();
        self.shared.before_run(allow_batch);
        self.shared.refresh_exclusive_stores()?;
        let mut check_irq = true;
        let mut check_context = true;
        let mut replay = false;
        let status = loop {
            if remaining() == 0 {
                break ffi::DynarmicExitReason::Timeout;
            }
            self.shared
                .flush_changed_code(std::mem::take(&mut check_context), self.fpexc.get());
            if std::mem::take(&mut check_irq) && self.irq.get() && armv7a::enter_irq(self)? {
                self.shared.clear_reservation()?;
                let mut entries = self.exception_entries.get();
                entries.irq += 1;
                self.exception_entries.set(entries);
                return Ok(Exit::Stopped);
            }
            let must_step = std::mem::take(&mut replay);
            if !must_step && allow_batch && !self.shared.device_accessed.get() && remaining() >= 2 {
                let mut executed = 0;
                self.shared.native_base.set(self.last_progress.get());
                self.shared.native_budget.set(remaining());
                self.shared.batching.set(true);
                let native_exit =
                    unsafe { ffi::dynarmic_run(self.inst, remaining(), &mut executed) };
                self.shared.batching.set(false);
                self.shared.transfer.set(None);
                self.shared.account_read_window();

                if executed > remaining() {
                    return Err("Native instruction budget overrun".into());
                }
                self.last_progress.set(self.last_progress.get() + executed);
                if let Some(address) = self.shared.replay_memory.take() {
                    let prepared = self.shared.replay_mmio.take();
                    let pc = unsafe { ffi::dynarmic_get_reg(self.inst, 15) };
                    let instruction = self
                        .shared
                        .fetch_with_cached_page(pc)?
                        .map_err(|_| "Unable to fetch native memory instruction for replay")?;
                    if let Some((base, value)) = self.shared.replay_base.take() {
                        unsafe { ffi::dynarmic_set_reg(self.inst, base, value) };
                    } else if instruction & (1 << 24) == 0 {
                        let base = (instruction >> 16) & 15;
                        unsafe { ffi::dynarmic_set_reg(self.inst, base, address) };
                    }
                    if self.complete_mmio(pc, instruction, address, prepared) {
                        check_irq = true;
                        if self.shared.stopped.get() {
                            break ffi::DynarmicExitReason::Stopped;
                        }
                        continue;
                    }
                    replay = true;
                } else if native_exit == ffi::DynarmicExitReason::Replay {
                    replay = true;
                } else if native_exit != ffi::DynarmicExitReason::Timeout {
                    break native_exit;
                } else if remaining() != 0 {
                    // No whole native block fits the remaining exact budget.
                    replay = true;
                }
                continue;
            }
            let pc = unsafe { ffi::dynarmic_get_reg(self.inst, 15) };
            if request.until == Some(u64::from(pc)) {
                break ffi::DynarmicExitReason::Timeout;
            }
            let cpsr = unsafe { ffi::dynarmic_get_cpsr(self.inst) };
            if cpsr & 0x20 != 0 {
                return Err(format!(
                    "Dynarmic system emulation does not support Thumb at {pc:#010x}"
                )
                .into());
            }
            let fetched = if allow_batch {
                self.shared.fetch_with_cached_page(pc)
            } else {
                self.shared.fetch(pc)
            };
            let word = match fetched.map_err(|message| ExecutionError::Memory {
                fault: Fault::FetchUnmapped,
                message,
            })? {
                Ok(word) => word,
                Err(status) => {
                    armv7a::abort::enter_reported(self, pc, status, Access::Fetch)?;
                    self.shared.clear_reservation()?;
                    return Ok(Exit::Stopped);
                }
            };
            self.shared.stopped.set(false);
            self.shared.observe_instruction(pc);
            if self.shared.stopped.get() || remaining() == 0 {
                break ffi::DynarmicExitReason::Stopped;
            }
            if stable
                && word & 0x0f80_0000 == 0x0a80_0000
                && request.until.is_none()
                && self.advance_loop(pc, cpsr, word, remaining(), allow_batch)?
            {
                continue;
            }
            if (self.shared.rprofile.is_some() || (allow_batch && remaining() > 1))
                && self.board_owns_instructions.get()
                && armv7a::exclusive_instruction(word)
                && let Some(context) = self.shared.exclusive.borrow().clone()
            {
                if word >> 28 == 15 || arm::condition_passes(word >> 28, cpsr) {
                    armv7a::execute_exclusive(self, word, cpsr, context.owner, &context.monitors)?;
                    self.shared.refresh_exclusive_stores()?;
                }
                unsafe { ffi::dynarmic_set_reg(self.inst, 15, pc.wrapping_add(4)) };
                self.last_progress.set(self.last_progress.get() + 1);
                #[cfg(test)]
                self.shared
                    .inlined_exclusives
                    .set(self.shared.inlined_exclusives.get() + 1);
                continue;
            }
            if self.board_owns_instructions.get()
                && self.shared.rprofile.is_none()
                && ((armv7a::framework_instruction(word) && !armv7a::mpidr_read(word))
                    || armv7a::exclusive_instruction(word))
            {
                return Ok(Exit::BeforeInstruction(word));
            }
            if let Some(next) = self
                .status_instruction(pc, word)
                .map_err(|error| format!("{error} (instruction {word:#010x} at {pc:#010x})"))?
            {
                check_irq = true;
                check_context = true;
                unsafe { ffi::dynarmic_set_reg(self.inst, 15, next) };
                self.last_progress.set(self.last_progress.get() + 1);
                continue;
            }
            if armv7a::is_vfp_instruction(word)
                && (word >> 28 == 15 || arm::condition_passes(word >> 28, cpsr))
                && !self.vfp_accessible()
            {
                if self.shared.rprofile.is_some() && armv7a::enter_disabled_vfp(self)? {
                    self.shared.clear_reservation()?;
                    let mut entries = self.exception_entries.get();
                    entries.undefined += 1;
                    self.exception_entries.set(entries);
                    self.last_progress.set(self.last_progress.get() + 1);
                    return Ok(Exit::Stopped);
                }
                return Err(ExecutionError::InvalidInstruction(format!(
                    "VFP or Advanced SIMD disabled at {pc:#010x}"
                )));
            }
            let mut executed = 0;
            if !must_step
                && allow_batch
                && !self.shared.device_accessed.get()
                && remaining() >= 2
                && batch_instruction(word) != 0
            {
                continue;
            }
            self.shared.fault.replace(None);
            self.shared.abort.set(None);
            self.tightened.set(false);
            if armv7a::block_boundary(word) {
                check_irq = true;
                check_context = true;
            }
            let status = unsafe { ffi::dynarmic_step(self.inst, &mut executed) };

            if self.tightened.get() && self.shared.device_accessed.replace(false) {
                // The device callback refreshed the board's deadline. The
                // next observer entry checks the newly published IRQ pins.
                check_irq = true;
            }
            if let Some((address, status, access)) = self.shared.abort.take() {
                armv7a::abort::enter_reported(self, address, status, access)
                    .map_err(|error| format!("ARMv7 abort at {pc:#010x}, {access:?} {address:#010x}, FSR {status:#04x}: {error}"))?;
                self.shared.clear_reservation()?;
                return Ok(Exit::Stopped);
            }
            if let Some(error) = self.shared.fault.take() {
                return Err(error);
            }
            self.last_progress.set(self.last_progress.get() + executed);
            if status != ffi::DynarmicExitReason::Timeout {
                break status;
            }
            if executed == 0 {
                return Err(format!("Dynarmic did not advance at {pc:#010x}").into());
            }
        };

        match status {
            ffi::DynarmicExitReason::Timeout | ffi::DynarmicExitReason::Stopped => {
                Ok(Exit::Stopped)
            }
            ffi::DynarmicExitReason::Wfi => {
                Ok(Exit::Hint(fujimod_emulator_cpu::Hint::WaitForInterrupt))
            }
            ffi::DynarmicExitReason::Wfe => {
                Ok(Exit::Hint(fujimod_emulator_cpu::Hint::WaitForEvent))
            }
            ffi::DynarmicExitReason::Sev => Ok(Exit::Hint(fujimod_emulator_cpu::Hint::SendEvent)),
            ffi::DynarmicExitReason::Svc => Err(ExecutionError::Backend(
                "Dynarmic cannot enter ARMv7-A supervisor exceptions".into(),
            )),
            ffi::DynarmicExitReason::Undefined => Err(ExecutionError::InvalidInstruction(
                "undefined instruction".into(),
            )),
            ffi::DynarmicExitReason::NoExecute => Err(ExecutionError::Memory {
                fault: Fault::FetchUnmapped,
                message: "execution fault on unmapped code page".into(),
            }),
            ffi::DynarmicExitReason::Breakpoint => Ok(Exit::Stopped),
            ffi::DynarmicExitReason::Error | ffi::DynarmicExitReason::Replay => {
                Err(ExecutionError::Backend("dynarmic execution error".into()))
            }
        }
    }

    fn set_loop_window(&self, stable: bool) -> Result<(), String> {
        self.loops.stable.set(stable);
        Ok(())
    }

    fn set_loop_mode(&self, mode: fujimod_emulator_cpu::loops::LoopMode) -> Result<(), String> {
        self.loops.mode.set(mode);
        Ok(())
    }

    fn tighten_instruction_limit(&self, total_entries: u64) -> Result<(), String> {
        let cap = self.run_limit.get().ok_or("Dynarmic has no active run")?;
        self.run_limit.set(Some(cap.min(total_entries)));
        self.tightened.set(true);
        Ok(())
    }

    fn instruction_progress(&self) -> Option<u64> {
        Some(self.last_progress.get())
    }

    fn stop(&self) -> Result<(), String> {
        self.shared.stopped.set(true);
        unsafe { ffi::dynarmic_stop(self.inst) };
        Ok(())
    }

    fn snapshot(&self, previous: Option<&dyn Snapshot>) -> Result<Box<dyn Snapshot>, String> {
        if previous.is_some_and(|saved| saved.owner() != self.identity) {
            return Err("Dynarmic CPU snapshot belongs to another processor".into());
        }
        snapshot::Saved::capture(self)
    }

    fn restore(&self, snapshot: &dyn Snapshot) -> Result<(), String> {
        snapshot::Saved::restore(self, snapshot)
    }

    fn import_snapshot(&self, bytes: &[u8]) -> Result<Box<dyn Snapshot>, String> {
        snapshot::Saved::import(self, bytes)
    }

    unsafe fn install_scheduler_hook(
        &self,
        callback: unsafe fn(u64, u32, *mut c_void),
        data: *mut c_void,
    ) -> Result<HookId, String> {
        let hook = unsafe {
            self.install_hook(Hook::Instruction {
                callback,
                data,
                range: HookRange::All,
            })?
        };
        self.scheduler_hook.set(Some(hook));
        Ok(hook)
    }

    unsafe fn install_hook(&self, hook: Hook) -> Result<HookId, String> {
        let key = self.shared.next_hook_id.get() + 1;
        self.shared.next_hook_id.set(key);
        if let Hook::Fault { callback, data } = hook {
            self.shared.fault_hooks.borrow_mut().push(FaultHook {
                id: key,
                callback,
                data,
            });
        } else if matches!(hook, Hook::Instruction { .. } | Hook::Memory { .. }) {
            self.shared.observers.borrow_mut().push((key, hook));
        } else {
            return Err("Dynarmic does not support this CPU hook".into());
        }
        Ok(HookId {
            owner: self.identity,
            key,
        })
    }

    fn remove_hook(&self, hook: HookId) -> Result<(), String> {
        if hook.owner == self.identity {
            self.shared
                .observers
                .borrow_mut()
                .retain(|(id, _)| *id != hook.key);
            self.shared
                .fault_hooks
                .borrow_mut()
                .retain(|h| h.id != hook.key);
        }
        Ok(())
    }

    unsafe fn translation_hooks(&self, _hooks: Option<TranslationHooks>) -> Result<(), String> {
        Ok(())
    }

    fn retry_block(&self, _instructions: u32) -> Result<(), String> {
        Ok(())
    }

    fn request_restart(&self) -> Result<(), String> {
        Ok(())
    }

    fn accounting(&self) -> Option<NonNull<BlockAccounting>> {
        None
    }

    unsafe fn observe_backings(&self, ranges: &[BackingObserver]) -> Result<(), String> {
        if ranges.iter().any(|range| {
            range.page_shift != 12
                || range.backing.as_ptr().addr() & 4095 != 0
                || range.bytes & 4095 != 0
        }) {
            return Err("Dynarmic backing observers require aligned 4 KiB pages".into());
        }
        let mut observers = self.shared.observed_backings.borrow_mut();
        observers.clear();
        observers.extend_from_slice(ranges);
        self.shared.clear_inline_pages();
        Ok(())
    }

    fn watch_stores(&self, _address: u64, _bytes: u64) -> Result<(), String> {
        Ok(())
    }

    fn hook_watched(&self, _hook: HookId, _watched: bool) -> Result<(), String> {
        Ok(())
    }

    fn precise_store_pc(&self, _on: bool) -> Result<(), String> {
        Ok(())
    }

    fn exit_counters(&self) -> Result<BTreeMap<&'static str, u64>, String> {
        Ok(BTreeMap::from([
            ("loop_accelerated_instructions", self.loops.skipped.get()),
            ("projected_mmio_reads", self.shared.projected_reads.get()),
        ]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rearmed_backing_tracks_the_first_store_even_after_an_inline_read() {
        let backing = Backing::aligned_zeros(8192, 4096).unwrap();
        let tracker = memory::pages::Tracker::new(backing.len());
        let cpu = Core::new(Model::CortexA7).unwrap();
        let pointer = NonNull::new(backing.as_ptr()).unwrap();
        unsafe { cpu.map_backing(0x1000, backing.len(), 7, pointer) }.unwrap();
        assert_eq!(cpu.checkpoint_backings().unwrap(), [(pointer, 8192)]);
        for (offset, word) in [
            0xe591_2000u32, // ldr r2, [r1]
            0xe581_0000,    // str r0, [r1]
            0xe581_0004,    // str r0, [r1, #4]
        ]
        .into_iter()
        .enumerate()
        {
            cpu.write_memory(0x1000 + offset as u64 * 4, &word.to_le_bytes())
                .unwrap();
        }
        let observer = tracker.observer(pointer).unwrap();
        let mut previous = 0u32;
        for value in [17u32, 42] {
            let before = unsafe { tracker.capture(backing.as_ptr()) };
            unsafe { cpu.observe_backings(&[observer]) }.unwrap();
            cpu.write_register(arm::CORE[0], &value.to_le_bytes())
                .unwrap();
            cpu.write_register(arm::CORE[1], &0x2000u32.to_le_bytes())
                .unwrap();
            cpu.set_loop_window(true).unwrap();
            cpu.run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(3),
            })
            .unwrap();
            let after = unsafe { tracker.capture(backing.as_ptr()) };
            assert_eq!(before.page(1).unwrap()[..4], previous.to_le_bytes());
            assert_eq!(after.page(1).unwrap()[..4], value.to_le_bytes());
            assert_eq!(after.page(1).unwrap()[4..8], value.to_le_bytes());
            assert_eq!(before.shared_pages(&after), 1);
            previous = value;
        }
    }

    #[test]
    fn batched_word_transfers_use_the_current_checked_page() {
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.map_zeroed(0x1000, 0x4000, 7).unwrap();
        let code: [u32; 4] = [
            0xe898_00e0, // ldmia r8, {r5, r6, r7}
            0xe8a8_001c, // stmia r8!, {r2, r3, r4}
            0xe938_00e0, // ldmdb r8!, {r5, r6, r7}
            0xe289_9001, // add r9, r9, #1
        ];
        cpu.write_memory(0x1000, &code.map(u32::to_le_bytes).concat())
            .unwrap();
        for (round, base) in [0x2100u32, 0x2100, 0x2ff8, 0x4100, 0x4100, 0x1100, 0x1100]
            .into_iter()
            .enumerate()
        {
            let values = [0x11u32, 0x22, 0x33].map(|v| v + round as u32);
            for (register, value) in (2..5).zip(values) {
                cpu.write_register(arm::CORE[register], &value.to_le_bytes())
                    .unwrap();
            }
            cpu.write_register(arm::CORE[8], &base.to_le_bytes())
                .unwrap();
            cpu.set_loop_window(true).unwrap();
            cpu.run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(code.len()),
            })
            .unwrap();

            let register = |index| {
                let mut value = [0; 4];
                cpu.read_register(arm::CORE[index], &mut value).unwrap();
                u32::from_le_bytes(value)
            };
            assert_eq!([register(5), register(6), register(7)], values);
            assert_eq!(register(8), base);
            assert_eq!(register(9), round as u32 + 1);
            assert_eq!(cpu.instruction_progress(), Some(code.len() as u64));
            let mut stored = [0; 12];
            cpu.read_memory(u64::from(base), &mut stored).unwrap();
            assert_eq!(stored.as_slice(), values.map(u32::to_le_bytes).concat());
        }
    }

    #[test]
    fn test_dynarmic_execution() {
        let core = Core::new(Model::CortexA7).expect("create dynarmic core");
        let mut ram = vec![0u8; 0x10000];
        let ptr = NonNull::new(ram.as_mut_ptr()).unwrap();
        unsafe {
            core.map_backing(0x40000000, 0x10000, 7, ptr)
                .expect("map backing");
        }

        // ARM32 code:
        // mov r0, #42       (0xe3a0002a)
        // add r0, r0, #8    (0xe2800008)
        // wfi               (0xe320f003)
        let code: [u32; 3] = [0xe3a0002a, 0xe2800008, 0xe320f003];
        let mut code_bytes = Vec::new();
        for &w in &code {
            code_bytes.extend_from_slice(&w.to_le_bytes());
        }
        core.write_memory(0x40000000, &code_bytes)
            .expect("write code");

        let exit = core
            .run(Run {
                begin: 0x40000000,
                until: None,
                watchdog_us: 0,
                instructions: Some(10),
            })
            .expect("run");

        match exit {
            Exit::Hint(Hint::WaitForInterrupt) | Exit::Stopped => {}
            other => panic!("unexpected exit: {other:?}"),
        }

        let mut out = [0u8; 4];
        core.read_register(Register::Arm32(arm::Register::R(0)), &mut out)
            .expect("read r0");
        let r0 = u32::from_le_bytes(out);
        assert_eq!(r0, 50, "r0 should be 42 + 8 = 50");
    }

    #[test]
    fn a_translation_fault_enters_the_data_abort_vector() {
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.map_zeroed(0, 0x8000, 7).unwrap();
        // A first-level section maps VA 0–1 MiB to the same physical region.
        cpu.write_memory(0x4000, &(0x0000_0c02u32).to_le_bytes())
            .unwrap();
        cpu.write_memory(0x1000, &0xe591_0000u32.to_le_bytes())
            .unwrap(); // ldr r0, [r1]
        cpu.write_register(arm::CORE[1], &0x0200_0000u32.to_le_bytes())
            .unwrap();
        let cp = |crn| arm::Coprocessor {
            cp: 15,
            crn,
            crm: 0,
            opc1: 0,
            opc2: 0,
            secure: 0,
            wide: false,
        };
        cpu.coprocessor(cp(2), Some(0x4000)).unwrap();
        cpu.coprocessor(cp(3), Some(1)).unwrap();
        cpu.coprocessor(cp(1), Some(0x0080_0001)).unwrap();
        assert!(matches!(
            cpu.run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(1)
            })
            .unwrap(),
            Exit::Stopped
        ));
        assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 15) }, 0x10);
        assert_eq!(cpu.spsr().unwrap(), 0xd3);
        assert_eq!(cpu.coprocessor(cp(5), None).unwrap(), 5);
        assert_eq!(cpu.coprocessor(cp(6), None).unwrap(), 0x0200_0000);
    }

    #[test]
    fn an_unaligned_load_reads_both_translated_pages() {
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.map_zeroed(0, 0x8000, 7).unwrap();
        cpu.write_memory(0x4000, &0x6001u32.to_le_bytes()).unwrap(); // coarse table
        for (index, physical) in [(1, 0x1000u32), (2, 0x2000), (3, 0x5000)] {
            cpu.write_memory(0x6000 + index * 4, &(physical | 0x32).to_le_bytes())
                .unwrap();
        }
        cpu.write_memory(0x1000, &0xe591_0000u32.to_le_bytes())
            .unwrap(); // ldr r0, [r1]
        cpu.write_memory(0x2ffe, &[0x12, 0x34]).unwrap();
        cpu.write_memory(0x5000, &[0x56, 0x78]).unwrap();
        cpu.write_register(arm::CORE[1], &0x2ffeu32.to_le_bytes())
            .unwrap();
        let cp = |crn| arm::Coprocessor {
            cp: 15,
            crn,
            crm: 0,
            opc1: 0,
            opc2: 0,
            secure: 0,
            wide: false,
        };
        cpu.coprocessor(cp(2), Some(0x4000)).unwrap();
        cpu.coprocessor(cp(3), Some(1)).unwrap();
        cpu.coprocessor(cp(1), Some(0x0080_0001)).unwrap();
        cpu.run(Run {
            begin: 0x1000,
            until: None,
            watchdog_us: 0,
            instructions: Some(1),
        })
        .unwrap();
        assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 0) }, 0x7856_3412);
    }
    #[test]
    fn new_peer_code_watch_keeps_unrelated_inline_pages() {
        let backing = Backing::aligned_zeros(8192, 4096).unwrap();
        let first = backing.as_ptr();
        let second = unsafe { first.add(4096) };
        let visibility = CodeVisibility::default();
        let reader = Core::new_with_code_visibility(Model::CortexA7, visibility.clone()).unwrap();
        let writer = Core::new_with_code_visibility(Model::CortexA7, visibility).unwrap();
        unsafe {
            writer
                .map_backing(0x3000, 8192, 7, NonNull::new(first).unwrap())
                .unwrap();
        }
        writer
            .shared
            .install_inline_page(0x3000, first, Access::Write);
        writer
            .shared
            .install_inline_page(0x4000, second, Access::Write);
        assert_eq!(writer.shared.inline_page(0x3000), first);
        assert_eq!(writer.shared.inline_page(0x4000), second);

        reader.shared.watch_code_backing(first);
        writer.shared.before_run(true);
        assert_eq!(writer.shared.inline_page(0x4000), second);
        let watched = writer.shared.inline_page(0x3000);
        if cfg!(target_arch = "aarch64") {
            assert_eq!(watched, first.map_addr(|addr| addr | 1));
        } else {
            assert!(watched.is_null());
        }
    }

    #[test]
    fn peer_alias_write_is_visible_after_translation() {
        for allow_native in [false, true] {
            let backing = Backing::aligned_zeros(4096, 4096).unwrap();
            let ptr = NonNull::new(backing.as_ptr()).unwrap();
            let visibility = CodeVisibility::default();
            let reader =
                Core::new_with_code_visibility(Model::CortexA7, visibility.clone()).unwrap();
            let writer = Core::new_with_code_visibility(Model::CortexA7, visibility).unwrap();
            unsafe {
                reader.map_backing(0x1000, 4096, 7, ptr).unwrap();
                reader.map_backing(0x2000, 4096, 7, ptr).unwrap();
                writer.map_backing(0x3000, 4096, 7, ptr).unwrap();
            }
            reader
                .write_memory(0x1000, &0xe280_0001u32.to_le_bytes())
                .unwrap(); // add r0, r0, #1
            reader
                .write_memory(0x1004, &0xeaff_fffdu32.to_le_bytes())
                .unwrap(); // b 0x1000
            writer.map_zeroed(0x8000, 4096, 7).unwrap();
            writer
                .write_memory(0x8000, &0xe581_0000u32.to_le_bytes())
                .unwrap(); // str r0, [r1]
            writer
                .write_register(arm::CORE[0], &0xe280_0002u32.to_le_bytes())
                .unwrap();
            writer
                .write_register(arm::CORE[1], &0x3000u32.to_le_bytes())
                .unwrap();
            for (cpu, begin, count, window) in [
                (&reader, 0x1000, 40, allow_native),
                (&reader, 0x2000, 40, allow_native),
                (&writer, 0x8000, 1, false),
                (&reader, 0x1000, 40, allow_native),
                (&reader, 0x2000, 40, allow_native),
            ] {
                cpu.set_loop_window(window).unwrap();
                cpu.run(Run {
                    begin,
                    until: None,
                    watchdog_us: 0,
                    instructions: Some(count),
                })
                .unwrap();
            }
            let mut bytes = [0; 4];
            reader.read_register(arm::CORE[0], &mut bytes).unwrap();
            assert_eq!(u32::from_le_bytes(bytes), 120, "native={allow_native}");

            // A device writes the backing directly and reports its physical
            // address, while both cached translations used other addresses.
            unsafe {
                ptr.cast::<[u8; 4]>()
                    .as_ptr()
                    .write(0xe280_0003u32.to_le_bytes());
                reader.backing_written(0x3000, ptr, 4).unwrap();
            }
            for begin in [0x1000, 0x2000] {
                reader.set_loop_window(allow_native).unwrap();
                reader
                    .run(Run {
                        begin,
                        until: None,
                        watchdog_us: 0,
                        instructions: Some(40),
                    })
                    .unwrap();
            }
            reader.read_register(arm::CORE[0], &mut bytes).unwrap();
            assert_eq!(u32::from_le_bytes(bytes), 240, "native={allow_native}");
        }
    }
    #[test]
    fn indirect_dispatch_refetches_code_written_through_a_peer_alias() {
        let backing = Backing::aligned_zeros(4096, 4096).unwrap();
        let ptr = NonNull::new(backing.as_ptr()).unwrap();
        let visibility = CodeVisibility::default();
        let reader = Core::new_with_code_visibility(Model::CortexA7, visibility.clone()).unwrap();
        let writer = Core::new_with_code_visibility(Model::CortexA7, visibility).unwrap();
        unsafe {
            reader.map_backing(0x1000, 4096, 7, ptr).unwrap();
            writer.map_backing(0x3000, 4096, 7, ptr).unwrap();
        }
        reader
            .write_memory(0x1000, &0xe12f_ff11u32.to_le_bytes())
            .unwrap(); // bx r1
        reader
            .write_memory(0x1040, &0xe280_0001u32.to_le_bytes())
            .unwrap(); // add r0, r0, #1
        reader
            .write_memory(0x1044, &0xe12f_ff12u32.to_le_bytes())
            .unwrap(); // bx r2
        reader
            .write_register(arm::CORE[1], &0x1040u32.to_le_bytes())
            .unwrap();
        reader
            .write_register(arm::CORE[2], &0x1000u32.to_le_bytes())
            .unwrap();
        let run = || {
            reader.set_loop_window(true).unwrap();
            reader
                .run(Run {
                    begin: 0x1000,
                    until: None,
                    watchdog_us: 0,
                    instructions: Some(60),
                })
                .unwrap();
            assert_eq!(reader.instruction_progress(), Some(60));
            unsafe { ffi::dynarmic_get_reg(reader.inst, 0) }
        };
        assert_eq!(run(), 20);
        writer
            .write_memory(0x3040, &0xe280_0002u32.to_le_bytes())
            .unwrap();
        assert_eq!(run(), 60);
    }

    #[test]
    #[cfg(target_arch = "aarch64")]
    fn repeated_projected_read_observes_a_descriptor_written_in_the_same_run() {
        struct Port {
            value: u64,
            reads: Cell<u64>,
            repeats: Cell<u64>,
        }
        unsafe fn read(
            offset: u64,
            size: u32,
            _: u64,
            data: *mut c_void,
        ) -> Option<ProjectedDeviceRead> {
            assert_eq!((offset, size), (0, 4));
            let port = unsafe { &*data.cast::<Port>() };
            port.reads.set(port.reads.get() + 1);
            Some(ProjectedDeviceRead {
                value: port.value,
                window: Some(ProjectedDeviceWindow {
                    before: 100,
                    countdown_expiry: None,
                    source: 0,
                    account,
                }),
            })
        }
        unsafe fn account(_: usize, count: u64, data: *mut c_void) {
            let port = unsafe { &*data.cast::<Port>() };
            port.repeats.set(port.repeats.get() + count);
        }
        unsafe fn ordinary_read(_: u64, _: u32, _: *mut c_void) -> u64 {
            panic!("expected a projected read");
        }
        unsafe fn write(_: u64, _: u32, _: u64, _: *mut c_void) {
            panic!("the port is read only");
        }
        let left = Port {
            value: 19,
            reads: Cell::new(0),
            repeats: Cell::new(0),
        };
        let right = Port {
            value: 47,
            reads: Cell::new(0),
            repeats: Cell::new(0),
        };
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.map_zeroed(0, 0x8000, 7).unwrap();
        for (physical, port) in [(0x101000, &left), (0x201000, &right)] {
            unsafe {
                cpu.map_mmio_projected(
                    physical,
                    4096,
                    Mmio {
                        read: ordinary_read,
                        write,
                        data: (port as *const Port).cast_mut().cast(),
                    },
                    Vec::new(),
                    read,
                )
                .unwrap();
            }
        }
        cpu.write_memory(0x4000, &0x0000_0c02u32.to_le_bytes())
            .unwrap();
        cpu.write_memory(0x4004, &0x0010_0c02u32.to_le_bytes())
            .unwrap();
        let code = [
            0xe591_0000u32,
            0xe591_2000, // two reads of the first port
            0xe584_3000, // update the descriptor for the data page
            0xe591_5000,
            0xe591_6000, // the same address now selects the second port
            0xeaff_fffe,
        ];
        for (index, word) in code.into_iter().enumerate() {
            cpu.write_memory(0x1000 + index as u64 * 4, &word.to_le_bytes())
                .unwrap();
        }
        for (register, value) in [(1, 0x101000u32), (3, 0x0020_0c02), (4, 0x4004)] {
            cpu.write_register(arm::CORE[register], &value.to_le_bytes())
                .unwrap();
        }
        let cp = |crn| arm::Coprocessor {
            cp: 15,
            crn,
            crm: 0,
            opc1: 0,
            opc2: 0,
            secure: 0,
            wide: false,
        };
        cpu.coprocessor(cp(2), Some(0x4000)).unwrap();
        cpu.coprocessor(cp(3), Some(1)).unwrap();
        cpu.coprocessor(cp(1), Some(0x0080_0001)).unwrap();
        cpu.set_device_read_anchor(Some(DeviceReadAnchor {
            entries: 0,
            clock: Clock {
                now: 0,
                phase: 0,
                ratio: timeline::Ratio::new(1, 1).unwrap(),
            },
        }));
        cpu.set_loop_window(true).unwrap();
        cpu.run(Run {
            begin: 0x1000,
            until: None,
            watchdog_us: 0,
            instructions: Some(6),
        })
        .unwrap();
        let values = [0, 2, 5, 6].map(|r| unsafe { ffi::dynarmic_get_reg(cpu.inst, r) });
        assert_eq!(values, [19, 19, 47, 47]);
        assert_eq!(cpu.instruction_progress(), Some(6));
        #[cfg(target_endian = "little")]
        assert_eq!(cpu.shared.inline_device_reads.get(), 2);
        for port in [left, right] {
            assert_eq!((port.reads.get(), port.repeats.get()), (1, 1));
        }
    }

    #[test]
    fn cached_device_translation_follows_a_live_descriptor_write() {
        struct Probe {
            value: u64,
            reads: Cell<u32>,
        }
        unsafe fn read(offset: u64, size: u32, data: *mut c_void) -> u64 {
            let probe = unsafe { &*data.cast::<Probe>() };
            assert_eq!((offset, size), (0, 4));
            probe.reads.set(probe.reads.get() + 1);
            probe.value
        }
        unsafe fn write(_: u64, _: u32, _: u64, _: *mut c_void) {
            panic!("the program only reads the device");
        }
        let left = Probe {
            value: 19,
            reads: Cell::new(0),
        };
        let right = Probe {
            value: 47,
            reads: Cell::new(0),
        };
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.map_zeroed(0, 0x8000, 7).unwrap();
        for (physical, probe) in [(0x101000, &left), (0x201000, &right)] {
            unsafe {
                cpu.map_mmio(
                    physical,
                    4096,
                    Mmio {
                        read,
                        write,
                        data: (probe as *const Probe).cast_mut().cast(),
                    },
                )
                .unwrap();
            }
        }
        cpu.write_memory(0x4000, &0x0000_0c02u32.to_le_bytes())
            .unwrap();
        cpu.write_memory(0x4004, &0x0010_0c02u32.to_le_bytes())
            .unwrap();
        cpu.write_memory(0x1000, &0xe591_0000u32.to_le_bytes())
            .unwrap(); // ldr r0, [r1]
        cpu.write_memory(0x1004, &0xeaff_fffdu32.to_le_bytes())
            .unwrap(); // b 0x1000
        cpu.write_register(arm::CORE[1], &0x101000u32.to_le_bytes())
            .unwrap();
        let cp = |crn| arm::Coprocessor {
            cp: 15,
            crn,
            crm: 0,
            opc1: 0,
            opc2: 0,
            secure: 0,
            wide: false,
        };
        cpu.coprocessor(cp(2), Some(0x4000)).unwrap();
        cpu.coprocessor(cp(3), Some(1)).unwrap();
        cpu.coprocessor(cp(1), Some(0x0080_0001)).unwrap();
        let run = || {
            cpu.set_loop_window(true).unwrap();
            cpu.run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(2),
            })
            .unwrap();
            assert_eq!(cpu.instruction_progress(), Some(2));
            unsafe { ffi::dynarmic_get_reg(cpu.inst, 0) }
        };
        assert_eq!(run(), 19);
        assert_eq!(run(), 19);
        cpu.write_memory(0x4004, &0x0020_0c02u32.to_le_bytes())
            .unwrap();
        assert_eq!(run(), 47);
        assert_eq!((left.reads.get(), right.reads.get()), (2, 1));
    }

    #[test]
    #[cfg(target_arch = "aarch64")]
    fn native_store_through_a_virtual_alias_clears_the_exclusive_reservation() {
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.map_zeroed(0, 0x8000, 7).unwrap();
        for address in [0x4000, 0x4004] {
            cpu.write_memory(address, &0x0000_0c02u32.to_le_bytes())
                .unwrap();
        }
        cpu.write_memory(0x2000, &47u32.to_le_bytes()).unwrap();
        let instructions = [
            0xe583_0000u32, // str r0, [r3] primes the writable translation
            0xe192_0f9f,    // ldrex r0, [r2]
            0xe583_0000,    // str r0, [r3] stores the same value via the alias
            0xe182_4f90,    // strex r4, r0, [r2]
            0xeaff_fffe,    // b .
        ];
        for (index, instruction) in instructions.into_iter().enumerate() {
            cpu.write_memory(0x1000 + 4 * index as u64, &instruction.to_le_bytes())
                .unwrap();
        }
        for (index, value) in [(0, 47u32), (2, 0x2000), (3, 0x102000)] {
            cpu.write_register(arm::CORE[index], &value.to_le_bytes())
                .unwrap();
        }
        let cp = |crn| arm::Coprocessor {
            cp: 15,
            crn,
            crm: 0,
            opc1: 0,
            opc2: 0,
            secure: 0,
            wide: false,
        };
        cpu.coprocessor(cp(2), Some(0x4000)).unwrap();
        cpu.coprocessor(cp(3), Some(1)).unwrap();
        cpu.coprocessor(cp(1), Some(0x0080_0001)).unwrap();
        let monitors = exclusive::Shared::default();
        let owner = monitors.lock().unwrap().register(64).unwrap();
        cpu.set_armv7a_owner_exclusives(true).unwrap();
        cpu.set_armv7a_exclusive_context(monitors.clone(), owner);
        cpu.set_loop_window(true).unwrap();
        cpu.run(Run {
            begin: 0x1000,
            until: None,
            watchdog_us: 0,
            instructions: Some(12),
        })
        .unwrap();
        assert_eq!(cpu.instruction_progress(), Some(12));
        assert_eq!(cpu.shared.inlined_exclusives.get(), 2);
        assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 4) }, 1);
        assert_eq!(monitors.lock().unwrap().reservation(owner), None);
    }

    #[test]
    #[cfg(target_arch = "aarch64")]
    fn native_load_after_register_updates_uses_its_own_entry_tick() {
        unsafe fn normal_read(_: u64, _: u32, _: *mut c_void) -> u64 {
            panic!("the projected read should not fall back");
        }
        unsafe fn write(_: u64, _: u32, _: u64, _: *mut c_void) {
            panic!("the program only reads");
        }
        unsafe fn read(
            offset: u64,
            size: u32,
            tick: u64,
            data: *mut c_void,
        ) -> Option<ProjectedDeviceRead> {
            assert_eq!((offset, size), (0, 4));
            let observed = unsafe { &*data.cast::<RefCell<Vec<u64>>>() };
            observed.borrow_mut().push(tick);
            Some(ProjectedDeviceRead {
                value: 42,
                window: None,
            })
        }
        for (mapped, post_indexed) in [(true, false), (false, false), (true, true), (false, true)] {
            let cpu = Core::new(Model::CortexA7).unwrap();
            cpu.map_zeroed(0x1000, 4096, 7).unwrap();
            let instructions = [
                0xe3a0_0005u32,
                0xe280_0007,
                if post_indexed {
                    0xe491_0004
                } else {
                    0xe591_0000
                },
                0xe280_0001,
                0xe592_0000,
                0xeaff_fffe,
            ];
            for (index, instruction) in instructions.into_iter().enumerate() {
                cpu.write_memory(0x1000 + 4 * index as u64, &instruction.to_le_bytes())
                    .unwrap();
            }
            cpu.write_register(arm::CORE[1], &0x8000u32.to_le_bytes())
                .unwrap();
            cpu.write_register(arm::CORE[2], &0x8000u32.to_le_bytes())
                .unwrap();
            let observed = RefCell::new(Vec::<u64>::new());
            if mapped {
                unsafe {
                    cpu.map_mmio_projected(
                        0x8000,
                        4096,
                        Mmio {
                            read: normal_read,
                            write,
                            data: (&observed as *const RefCell<Vec<u64>>).cast_mut().cast(),
                        },
                        Vec::new(),
                        read,
                    )
                    .unwrap();
                }
            }
            cpu.set_device_read_anchor(Some(DeviceReadAnchor {
                entries: 0,
                clock: Clock {
                    now: 100,
                    phase: 0,
                    ratio: timeline::Ratio::new(1, 1).unwrap(),
                },
            }));
            cpu.set_loop_window(true).unwrap();
            let result = cpu.run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(8),
            });
            let loaded = unsafe { ffi::dynarmic_get_reg(cpu.inst, 0) };
            let base = unsafe { ffi::dynarmic_get_reg(cpu.inst, 1) };
            if mapped {
                result.unwrap();
                assert_eq!(*observed.borrow(), [102, 104]);
                assert_eq!(cpu.instruction_progress(), Some(8));
                assert_eq!(loaded, 42);
                assert_eq!(base, if post_indexed { 0x8004 } else { 0x8000 });
            } else {
                assert!(matches!(
                    result,
                    Err(ExecutionError::Memory {
                        fault: Fault::ReadUnmapped,
                        ..
                    })
                ));
                assert_eq!(cpu.instruction_progress(), Some(2));
                assert_eq!(loaded, 12);
            }
        }
    }

    #[test]
    fn disabled_mmu_and_enabled_translation_use_distinct_keys() {
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.map_zeroed(0, 0x102000, 7).unwrap();
        for (address, add) in [(0x1000, 0xe280_0001u32), (0x101000, 0xe280_0002u32)] {
            cpu.write_memory(address, &add.to_le_bytes()).unwrap();
            cpu.write_memory(address + 4, &0xeaff_fffdu32.to_le_bytes())
                .unwrap();
        }
        cpu.write_memory(0x4000, &0x0010_0c02u32.to_le_bytes())
            .unwrap();
        let cp = |crn| arm::Coprocessor {
            cp: 15,
            crn,
            crm: 0,
            opc1: 0,
            opc2: 0,
            secure: 0,
            wide: false,
        };
        let run = |expected| {
            cpu.set_loop_window(true).unwrap();
            cpu.run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(8),
            })
            .unwrap();
            assert_eq!(cpu.instruction_progress(), Some(8));
            assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 0) }, expected);
        };
        run(4);
        cpu.coprocessor(cp(2), Some(0x4000)).unwrap();
        cpu.coprocessor(cp(3), Some(1)).unwrap();
        cpu.coprocessor(cp(1), Some(0x0080_0001)).unwrap();
        run(12);
        cpu.coprocessor(cp(2), Some(0x4008)).unwrap();
        cpu.coprocessor(cp(1), Some(0x0080_1001)).unwrap();
        run(20);
        cpu.coprocessor(cp(1), Some(0x0080_1000)).unwrap();
        run(24);
        if cfg!(target_arch = "aarch64") {
            assert_eq!(cpu.shared.contexts.borrow().len(), 2);
            assert_eq!(cpu.shared.control_flushes.get(), 3);
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn native_link_stops_before_zero_cycle_replay_when_budget_is_exhausted() {
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.map_zeroed(0x1000, 4096, 7).unwrap();
        for (address, word) in [
            (0x1000, 0xe280_0001u32), // add r0, r0, #1
            (0x1004, 0xea00_003d),    // b 0x1100
            (0x1100, 0xe320_f003),    // wfi, an exact replay entry
        ] {
            cpu.write_memory(address, &word.to_le_bytes()).unwrap();
        }
        cpu.set_loop_window(true).unwrap();
        let warm = cpu
            .run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(3),
            })
            .unwrap();
        assert!(matches!(warm, Exit::Hint(Hint::WaitForInterrupt)));

        let mut executed = 0;
        let exit = unsafe {
            ffi::dynarmic_set_reg(cpu.inst, 15, 0x1000);
            ffi::dynarmic_run(cpu.inst, 2, &mut executed)
        };
        assert!(matches!(exit, ffi::DynarmicExitReason::Timeout));
        assert_eq!(executed, 2);
        assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 15) }, 0x1100);
    }

    #[test]
    fn native_run_and_step_discard_cached_code_after_a_host_write() {
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.map_zeroed(0x1000, 4096, 7).unwrap();
        let set_increment = |increment: u32| {
            let add = 0xe280_0000u32 | increment;
            cpu.write_memory(0x1000, &add.to_le_bytes()).unwrap();
        };
        // add r0, r0, #2
        cpu.write_memory(0x1004, &0xe280_0002u32.to_le_bytes())
            .unwrap();
        // b .
        cpu.write_memory(0x1008, &0xeaff_fffeu32.to_le_bytes())
            .unwrap();
        let run = |count, expected| {
            cpu.set_loop_window(true).unwrap();
            cpu.run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(count),
            })
            .unwrap();
            assert_eq!(cpu.instruction_progress(), Some(count as u64));
            assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 0) }, expected);
        };

        set_increment(1);
        run(2, 3);
        run(1, 4);
        run(2, 7);
        run(1, 8);
        set_increment(4);
        run(1, 12);
        run(2, 18);
    }

    #[test]
    fn native_conditional_block_stops_at_the_instruction_budget() {
        for (cpsr, expected) in [(0x4000_00d3u32, 2), (0x0000_00d3, 0)] {
            let core = Core::new(Model::CortexA7).unwrap();
            core.map_zeroed(0x1000, 4096, 7).unwrap();
            for address in [0x1000, 0x1004, 0x1008, 0x100c] {
                // addeq r0, r0, #1
                core.write_memory(address, &0x0280_0001u32.to_le_bytes())
                    .unwrap();
            }
            // b .
            core.write_memory(0x1010, &0xeaff_fffeu32.to_le_bytes())
                .unwrap();
            core.write_register(arm::CPSR, &cpsr.to_le_bytes()).unwrap();
            core.set_loop_window(true).unwrap();
            core.run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(2),
            })
            .unwrap();
            assert_eq!(core.instruction_progress(), Some(2));
            assert_eq!(unsafe { ffi::dynarmic_get_reg(core.inst, 0) }, expected);
            assert_eq!(unsafe { ffi::dynarmic_get_reg(core.inst, 15) }, 0x1008);
        }
    }

    #[test]
    fn predicated_alu_keeps_live_flags_and_respects_short_budgets() {
        let run = |batch, count| {
            let cpu = Core::new(Model::CortexA7).unwrap();
            cpu.map_zeroed(0x1000, 4096, 7).unwrap();
            for (index, word) in [
                0xe150_0121u32, // cmp r0, r1, lsr #2
                0x2040_0121,    // subcs r0, r0, r1, lsr #2
                0x2182_2003,    // orrcs r2, r2, r3
                0x33a0_4048,    // movcc r4, #72
                0xe150_0121,    // cmp r0, r1, lsr #2
                0x2040_0121,    // subcs r0, r0, r1, lsr #2
                0x2182_20a3,    // orrcs r2, r2, r3, lsr #1
                0x31a0_4001,    // movcc r4, r1
                0x23a0_4050,    // movcs r4, #80
                0x4284_4005,    // addmi r4, r4, #5
                0x4084_4083,    // addmi r4, r4, r3, lsl #1
                0x4264_4064,    // rsbmi r4, r4, #100
                0x4060_0083,    // rsbmi r0, r0, r3, lsl #1
            ]
            .into_iter()
            .enumerate()
            {
                cpu.write_memory(0x1000 + index as u64 * 4, &word.to_le_bytes())
                    .unwrap();
            }
            for (reg, value) in [17u32, 44, 1, 8, 7].into_iter().enumerate() {
                cpu.write_register(arm::CORE[reg], &value.to_le_bytes())
                    .unwrap();
            }
            cpu.set_loop_window(batch).unwrap();
            cpu.run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(count),
            })
            .unwrap();
            assert_eq!(cpu.instruction_progress(), Some(count as u64));
            let regs = [0, 2, 4, 15].map(|reg| unsafe { ffi::dynarmic_get_reg(cpu.inst, reg) });
            let flags = unsafe { ffi::dynarmic_get_cpsr(cpu.inst) } >> 28;
            (regs, flags)
        };
        for count in 1..=13 {
            assert_eq!(run(true, count), run(false, count), "budget {count}");
        }
        assert_eq!(run(true, 13), ([10, 9, 35, 0x1034], 8));
    }

    #[test]
    fn flags_before_a_memory_fault_remain_visible_after_a_predicated_select() {
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.map_zeroed(0x1000, 4096, 7).unwrap();
        for (index, word) in [
            0xe150_0000u32, // cmp r0, r0
            0x03a0_0005,    // moveq r0, #5
            0xe591_2000,    // ldr r2, [r1]
            0xe350_0000,    // cmp r0, #0
        ]
        .into_iter()
        .enumerate()
        {
            cpu.write_memory(0x1000 + index as u64 * 4, &word.to_le_bytes())
                .unwrap();
        }
        cpu.write_register(arm::CORE[1], &0x2000u32.to_le_bytes())
            .unwrap();
        cpu.set_loop_window(true).unwrap();
        let result = cpu.run(Run {
            begin: 0x1000,
            until: None,
            watchdog_us: 0,
            instructions: Some(4),
        });
        assert!(matches!(
            result,
            Err(ExecutionError::Memory {
                fault: Fault::ReadUnmapped,
                ..
            })
        ));
        assert_eq!(cpu.instruction_progress(), Some(2));
        assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 0) }, 5);
        assert_eq!(unsafe { ffi::dynarmic_get_cpsr(cpu.inst) } >> 28, 6);
    }

    #[test]
    fn native_shift_counting_matches_single_step_at_each_boundary() {
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.map_zeroed(0x1000, 4096, 7).unwrap();
        for (index, word) in [
            0xe1a0_2122u32, // mov r2, r2, lsr #2
            0xe284_4003,    // add r4, r4, #3
            0xe352_0003,    // cmp r2, #3
            0x8aff_fffb,    // bhi 0x1000
            0xe3a0_500c,    // mov r5, #12
            0xeaff_fffe,    // b .
        ]
        .into_iter()
        .enumerate()
        {
            cpu.write_memory(0x1000 + index as u64 * 4, &word.to_le_bytes())
                .unwrap();
        }
        let run = |batch, value: u32, budget| {
            for (reg, value) in [(2, value), (4, u32::MAX - 1), (5, 0)] {
                cpu.write_register(arm::CORE[reg], &value.to_le_bytes())
                    .unwrap();
            }
            cpu.set_cpsr(0x0800_00d3).unwrap();
            cpu.set_loop_window(batch).unwrap();
            cpu.run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(budget),
            })
            .unwrap();
            assert_eq!(cpu.instruction_progress(), Some(budget as u64));
            let regs = [2, 4, 5, 15].map(|reg| unsafe { ffi::dynarmic_get_reg(cpu.inst, reg) });
            (regs, unsafe { ffi::dynarmic_get_cpsr(cpu.inst) })
        };
        let mut skipped_inside_loop = false;
        for value in [0, 16, 0x7fff_ffff, u32::MAX] {
            for budget in 1..=65 {
                let skipped = cpu.loops.skipped.get();
                assert_eq!(run(true, value, budget), run(false, value, budget));
                if value == u32::MAX && budget < 60 {
                    skipped_inside_loop |= cpu.loops.skipped.get() > skipped;
                }
            }
        }
        assert!(skipped_inside_loop);
    }

    #[test]
    fn native_word_fill_matches_single_step_across_ram_pages() {
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.map_zeroed(0x1000, 0x3000, 7).unwrap();
        for (index, word) in [
            0xe480_1004u32, // str r1, [r0], #4
            0xe252_2001,    // subs r2, r2, #1
            0x1aff_fffc,    // bne 0x1000
            0xe280_1001,    // add r1, r0, #1
        ]
        .into_iter()
        .enumerate()
        {
            cpu.write_memory(0x1000 + index as u64 * 4, &word.to_le_bytes())
                .unwrap();
        }
        let run = |batch, budget| {
            cpu.write_memory(0x2ff0, &[0x7d; 48]).unwrap();
            for (reg, value) in [0x2ff0u32, 0x89ab_cdef, 9].into_iter().enumerate() {
                cpu.write_register(arm::CORE[reg], &value.to_le_bytes())
                    .unwrap();
            }
            cpu.set_cpsr(0xd3).unwrap();
            cpu.set_loop_window(batch).unwrap();
            cpu.run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(budget),
            })
            .unwrap();
            assert_eq!(cpu.instruction_progress(), Some(budget as u64));
            let mut memory = [0; 48];
            cpu.read_memory(0x2ff0, &mut memory).unwrap();
            let regs = [0, 1, 2, 15].map(|reg| unsafe { ffi::dynarmic_get_reg(cpu.inst, reg) });
            (memory, regs, unsafe { ffi::dynarmic_get_cpsr(cpu.inst) })
        };
        for budget in 1..=28 {
            assert_eq!(run(true, budget), run(false, budget), "budget {budget}");
        }
        assert!(cpu.loops.skipped.get() > 0);
    }

    #[test]
    fn native_backward_branch_counts_taken_and_fallthrough_paths() {
        let core = Core::new(Model::CortexA7).unwrap();
        core.map_zeroed(0x1000, 4096, 7).unwrap();
        for (offset, word) in [
            (0, 0xe280_0001u32), // add r0, r0, #1
            (4, 0xaaff_fffd),    // bge 0x1000
            (8, 0xe280_0002),    // add r0, r0, #2
            (12, 0xeaff_fffe),   // b .
        ] {
            core.write_memory(0x1000 + offset, &word.to_le_bytes())
                .unwrap();
        }
        for (cpsr, expected_pc, expected_value) in
            [(0x0000_00d3, 0x1004, 1), (0x8000_00d3, 0x100c, 2)]
        {
            core.set_cpsr(cpsr).unwrap();
            core.write_register(arm::CORE[0], &0u32.to_le_bytes())
                .unwrap();
            core.set_loop_window(true).unwrap();
            core.run(Run {
                begin: 0x1004,
                until: None,
                watchdog_us: 0,
                instructions: Some(2),
            })
            .unwrap();
            assert_eq!(core.instruction_progress(), Some(2));
            assert_eq!(
                unsafe { ffi::dynarmic_get_reg(core.inst, 0) },
                expected_value
            );
            assert_eq!(unsafe { ffi::dynarmic_get_reg(core.inst, 15) }, expected_pc);
        }
    }

    #[test]
    fn native_conditional_branch_observes_flags_set_by_its_prefix() {
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.map_zeroed(0x1000, 4096, 7).unwrap();
        for (offset, word) in [
            (0, 0xe350_0000u32), // cmp r0, #0
            (4, 0x0a00_0000),    // beq 0x100c
            (8, 0xe3a0_1001),    // mov r1, #1
            (12, 0xe3a0_1002),   // mov r1, #2
        ] {
            cpu.write_memory(0x1000 + offset, &word.to_le_bytes())
                .unwrap();
        }
        for (r0, initial_flags, branch_target) in [(0u32, 0, 0x100c), (1, 1 << 30, 0x1008)] {
            for (count, pc) in [(1, 0x1004), (2, branch_target), (3, branch_target + 4)] {
                cpu.set_cpsr(initial_flags | 0xd3).unwrap();
                cpu.write_register(arm::CORE[0], &r0.to_le_bytes()).unwrap();
                cpu.set_loop_window(true).unwrap();
                cpu.run(Run {
                    begin: 0x1000,
                    until: None,
                    watchdog_us: 0,
                    instructions: Some(count),
                })
                .unwrap();
                assert_eq!(cpu.instruction_progress(), Some(count as u64));
                assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 15) }, pc);
            }
        }
    }

    #[test]
    fn native_branches_test_each_saved_flag_and_its_inverse() {
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.map_zeroed(0x1000, 4096, 7).unwrap();
        cpu.write_memory(0x1000, &0xe1a0_0000u32.to_le_bytes())
            .unwrap(); // mov r0, r0
        for condition in 0..=7 {
            let word = (condition << 28) | 0x0a00_0000u32;
            cpu.write_memory(0x1004, &word.to_le_bytes()).unwrap();
            for flags in [0, 1 << 28, 1 << 29, 1 << 30, 1 << 31] {
                let cpsr = flags | 0xd3;
                cpu.set_cpsr(cpsr).unwrap();
                cpu.set_loop_window(true).unwrap();
                cpu.run(Run {
                    begin: 0x1000,
                    until: None,
                    watchdog_us: 0,
                    instructions: Some(2),
                })
                .unwrap();
                let target = if arm::condition_passes(condition, cpsr) {
                    0x100c
                } else {
                    0x1008
                };
                assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 15) }, target);
                assert_eq!(unsafe { ffi::dynarmic_get_cpsr(cpu.inst) }, cpsr);
            }
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn restoring_division_matches_steps_and_watches_its_back_edge() {
        let code = 0x1000;
        let remainder = 7;
        let divisor = 4;
        let place = 6;
        let quotient = 3;
        let mut words = [0xe1a0_0000u32; 64];
        for shift in 0..4 {
            let operand_shift = if shift == 0 { 0 } else { (shift << 7) | 32 };
            words[shift as usize * 3] = 0xe150_0000 | remainder << 16 | divisor | operand_shift;
            words[shift as usize * 3 + 1] =
                0x2040_0000 | remainder << 16 | remainder << 12 | divisor | operand_shift;
            words[shift as usize * 3 + 2] =
                0x2180_0000 | quotient << 16 | quotient << 12 | place | operand_shift;
        }
        words[12] = 0xe350_0000 | remainder << 16;
        words[13] = 0x11b0_0220 | place << 12 | place;
        words[14] = 0x11a0_0220 | divisor << 12 | divisor;
        words[15] = 0x1aff_ffef;
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.map_zeroed(code, 4096, 7).unwrap();
        let raw: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        cpu.write_memory(code, &raw).unwrap();
        let run = |batched, budget, operands: [u32; 4]| {
            for (register, value) in [remainder, divisor, place, quotient]
                .into_iter()
                .zip(operands)
            {
                unsafe { ffi::dynarmic_set_reg(cpu.inst, register, value) };
            }
            cpu.set_cpsr(0xb000_00d3).unwrap();
            cpu.set_loop_window(batched).unwrap();
            cpu.run(Run {
                begin: code,
                until: None,
                watchdog_us: 0,
                instructions: Some(budget),
            })
            .unwrap();
            assert_eq!(cpu.instruction_progress(), Some(budget as u64));
            let registers = [remainder, divisor, place, quotient, 15]
                .map(|reg| unsafe { ffi::dynarmic_get_reg(cpu.inst, reg) });
            (registers, unsafe { ffi::dynarmic_get_cpsr(cpu.inst) })
        };
        for operands in [
            [5678, 17 << 8, 1 << 8, 0x8000_0000], // Remainder after a partial final group.
            [255 * 17, 17 << 8, 1 << 8, 0],       // Exact division.
            [256 * 17, 17 << 8, 1 << 8, 0],       // Early exit after the first group.
            [5678, 17 << 8, 3 << 8, 0],           // Multiple bits in the place.
            [9000, 17 << 8 | 3, 1 << 8, 0],       // Unaligned divisor.
            [u32::MAX, 17 << 8, 1 << 8, 0],       // Quotient exceeds the place range.
        ] {
            for budget in [9, 15, 16, 17, 32, 48, 49] {
                assert_eq!(run(true, budget, operands), run(false, budget, operands));
            }
        }
        // Change a word beyond the initial translated prefix and verify that
        // the warmed fast entry observes the new fall-through behavior.
        cpu.write_memory(code + 60, &0xe1a0_0000u32.to_le_bytes())
            .unwrap();
        let operands = [5678, 17 << 8, 1 << 8, 0];
        assert_eq!(run(true, 48, operands), run(false, 48, operands));
    }

    #[test]
    fn predicated_halfword_transfers_run_natively_and_suppress_false_accesses() {
        let cpu = Core::new(Model::CortexA7).unwrap();
        let address = 0x1000;
        let ram = 0x2000;
        cpu.map_zeroed(address, 4096, 7).unwrap();
        cpu.map_zeroed(ram, 4096, 3).unwrap();
        for (offset, word) in [
            (0, 0xe350_0000u32), // cmp r0, #0
            (4, 0xc1c1_20b0),    // strhgt r2, [r1]
            (8, 0xc1d1_30f0),    // ldrshgt r3, [r1]
            (12, 0xe280_0001),   // add r0, r0, #1
        ] {
            cpu.write_memory(address + offset, &word.to_le_bytes())
                .unwrap();
        }
        cpu.set_cpsr(0xd3).unwrap();
        for (condition, base, expected) in [(1, ram as u32, 0xffff_8001), (0, 0x4000, 0x55)] {
            let mut executed = 0;
            cpu.shared.batching.set(true);
            let reason = unsafe {
                ffi::dynarmic_set_reg(cpu.inst, 0, condition);
                ffi::dynarmic_set_reg(cpu.inst, 1, base);
                ffi::dynarmic_set_reg(cpu.inst, 2, 0x8001);
                ffi::dynarmic_set_reg(cpu.inst, 3, 0x55);
                ffi::dynarmic_set_reg(cpu.inst, 15, address as u32);
                ffi::dynarmic_run(cpu.inst, 4, &mut executed)
            };
            cpu.shared.batching.set(false);
            assert_eq!(reason, ffi::DynarmicExitReason::Timeout);
            assert_eq!(executed, 4);
            assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 0) }, condition + 1);
            assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 3) }, expected);
            assert!(cpu.shared.replay_memory.get().is_none());
        }
        let mut output = [0; 2];
        cpu.read_memory(ram, &mut output).unwrap();
        assert_eq!(u16::from_le_bytes(output), 0x8001);
    }

    #[test]
    fn translated_vfp_arithmetic_tracks_enable_and_privilege() {
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.map_zeroed(0x1000, 4096, 7).unwrap();
        // Supervisor-only access to CP10 and CP11.
        let cpacr = arm::Coprocessor {
            cp: 15,
            crn: 1,
            crm: 0,
            opc1: 0,
            opc2: 2,
            secure: 0,
            wide: false,
        };
        cpu.coprocessor(cpacr, Some(0x0050_0000)).unwrap();
        cpu.set_cpsr(0xd3).unwrap();
        cpu.set_fpexc(1 << 30).unwrap();
        for (index, word) in [
            0xee00_0a10u32, // vmov s0, r0
            0xee00_1a90,    // vmov s1, r1
            0xee30_0a20,    // vadd.f32 s0, s0, s1
            0xee30_0a20,    // vadd.f32 s0, s0, s1
            0xee10_2a10,    // vmov r2, s0
        ]
        .into_iter()
        .enumerate()
        {
            cpu.write_memory(0x1000 + index as u64 * 4, &word.to_le_bytes())
                .unwrap();
        }
        cpu.write_register(arm::CORE[0], &1.0f32.to_bits().to_le_bytes())
            .unwrap();
        cpu.write_register(arm::CORE[1], &2.0f32.to_bits().to_le_bytes())
            .unwrap();
        let run = |begin, count| {
            cpu.set_loop_window(true).unwrap();
            cpu.run(Run {
                begin,
                until: None,
                watchdog_us: 0,
                instructions: Some(count),
            })
        };
        run(0x1000, 5).unwrap();
        assert_eq!(
            unsafe { ffi::dynarmic_get_reg(cpu.inst, 2) },
            5.0f32.to_bits()
        );
        // Warm the entry point that the following requests will try to reuse.
        run(0x1008, 3).unwrap();
        assert_eq!(
            unsafe { ffi::dynarmic_get_reg(cpu.inst, 2) },
            9.0f32.to_bits()
        );
        cpu.set_cpsr(0xd0).unwrap();
        assert!(matches!(
            run(0x1008, 3),
            Err(ExecutionError::InvalidInstruction(_))
        ));
        cpu.set_cpsr(0xd3).unwrap();
        cpu.set_fpexc(0).unwrap();
        assert!(matches!(
            run(0x1008, 3),
            Err(ExecutionError::InvalidInstruction(_))
        ));
        cpu.set_fpexc(1 << 30).unwrap();
        run(0x1008, 3).unwrap();
        assert_eq!(
            unsafe { ffi::dynarmic_get_reg(cpu.inst, 2) },
            13.0f32.to_bits()
        );

        #[cfg(target_arch = "aarch64")]
        {
            let read_fpcr = || {
                let value: u64;
                unsafe { std::arch::asm!("mrs {value}, fpcr", value = out(reg) value) };
                value
            };
            let saved = read_fpcr();
            let round_up = 1 << 22;
            let host = saved & !(3 << 22) | round_up;
            cpu.write_register(arm::CORE[0], &1.0f32.to_bits().to_le_bytes())
                .unwrap();
            cpu.write_register(arm::CORE[1], &0x3380_0000u32.to_le_bytes())
                .unwrap(); // Half an ULP at 1.0.
            unsafe { ffi::dynarmic_set_fpscr(cpu.inst, 0) };
            unsafe { std::arch::asm!("msr fpcr, {value}", value = in(reg) host) };
            let different_modes = run(0x1000, 5);
            let different_host = read_fpcr();
            let nearest = unsafe { ffi::dynarmic_get_reg(cpu.inst, 2) };
            unsafe { ffi::dynarmic_set_fpscr(cpu.inst, round_up as u32) };
            let equal_modes = run(0x1000, 5);
            let equal_host = read_fpcr();
            let upward = unsafe { ffi::dynarmic_get_reg(cpu.inst, 2) };
            unsafe { std::arch::asm!("msr fpcr, {value}", value = in(reg) saved) };
            different_modes.unwrap();
            equal_modes.unwrap();
            assert_eq!(different_host, host);
            assert_eq!(equal_host, host);
            assert_eq!(nearest, 1.0f32.to_bits());
            assert_eq!(upward, 1.0f32.to_bits() + 2);
        }
    }

    #[test]
    fn same_guest_pc_uses_the_current_translation_context() {
        let core = Core::new(Model::CortexA7).unwrap();
        core.map_zeroed(0, 0x102000, 7).unwrap();
        core.write_memory(0x4000, &0x0000_0c02u32.to_le_bytes())
            .unwrap();
        core.write_memory(0x8000, &0x0010_0c02u32.to_le_bytes())
            .unwrap();
        for (address, add) in [(0x1000, 0xe280_0001u32), (0x101000, 0xe280_0002u32)] {
            core.write_memory(address, &add.to_le_bytes()).unwrap();
            core.write_memory(address + 4, &0xeaff_fffdu32.to_le_bytes())
                .unwrap();
        }
        let cp = |crn| arm::Coprocessor {
            cp: 15,
            crn,
            crm: 0,
            opc1: 0,
            opc2: 0,
            secure: 0,
            wide: false,
        };
        core.coprocessor(cp(3), Some(1)).unwrap();
        core.coprocessor(cp(1), Some(0x0080_0001)).unwrap();
        for (ttbr, expected) in [(0x4000, 20), (0x8000, 60), (0x4000, 80)] {
            core.coprocessor(cp(2), Some(ttbr)).unwrap();
            core.set_loop_window(true).unwrap();
            core.run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(40),
            })
            .unwrap();
            assert_eq!(core.instruction_progress(), Some(40));
            let mut bytes = [0; 4];
            core.read_register(arm::CORE[0], &mut bytes).unwrap();
            assert_eq!(u32::from_le_bytes(bytes), expected);
        }
        if cfg!(target_arch = "aarch64") {
            assert_eq!(core.shared.contexts.borrow().len(), 2);
        }
        assert_eq!(core.shared.memory_flushes.get(), 0);

        // Rewrite a live table entry without changing TTBR, then return to
        // the identical virtual PC. The former code must not be reused.
        core.write_memory(0x4000, &0x0010_0c02u32.to_le_bytes())
            .unwrap();
        core.set_loop_window(true).unwrap();
        core.run(Run {
            begin: 0x1000,
            until: None,
            watchdog_us: 0,
            instructions: Some(40),
        })
        .unwrap();
        assert_eq!(unsafe { ffi::dynarmic_get_reg(core.inst, 0) }, 120);
        if cfg!(target_arch = "aarch64") {
            assert_eq!(core.shared.contexts.borrow().len(), 2);
        }
        assert_eq!(core.shared.memory_flushes.get(), 1);
    }
    #[test]
    fn cached_loop_proof_checks_changed_instructions() {
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.map_zeroed(0x1000, 4096, 7).unwrap();
        cpu.write_memory(0x1004, &0xe250_0001u32.to_le_bytes())
            .unwrap(); // subs r0, r0, #1
        cpu.write_memory(0x1008, &0xaaff_fffcu32.to_le_bytes())
            .unwrap(); // bge 0x1000
        for value in [1u32, 1, 2] {
            cpu.write_memory(0x1000, &(0xe3a0_1000u32 | value).to_le_bytes())
                .unwrap();
            cpu.write_register(arm::CORE[0], &5u32.to_le_bytes())
                .unwrap();
            cpu.set_loop_window(true).unwrap();
            let skipped = cpu.loops.skipped.get();
            cpu.run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(18),
            })
            .unwrap();
            assert_eq!(cpu.instruction_progress(), Some(18));
            assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 1) }, value);
            assert!(cpu.loops.skipped.get() > skipped);
        }
    }

    #[test]
    fn a_self_branch_can_only_use_the_current_instruction_budget() {
        use fujimod_emulator_cpu::loops::LoopMode;

        for (word, condition) in [(0xeaff_fffeu32, true), (0x0aff_fffe, false)] {
            let run = |mode| {
                let cpu = Core::new(Model::CortexA7).unwrap();
                cpu.map_zeroed(0x1000, 4096, 7).unwrap();
                cpu.write_memory(0x1000, &word.to_le_bytes()).unwrap();
                for address in (0x1004..0x1200).step_by(4) {
                    cpu.write_memory(address, &0xe280_0001u32.to_le_bytes())
                        .unwrap(); // add r0, r0, #1
                }
                cpu.set_armv7a_owner_exclusives(true).unwrap();
                cpu.set_loop_mode(mode).unwrap();
                cpu.set_loop_window(true).unwrap();
                cpu.run(Run {
                    begin: 0x1000,
                    until: None,
                    watchdog_us: 0,
                    instructions: Some(97),
                })
                .unwrap();
                assert_eq!(cpu.instruction_progress(), Some(97));
                (
                    cpu.snapshot(None).unwrap().export().unwrap(),
                    cpu.loops.skipped.get(),
                )
            };
            let (stepped, no_skips) = run(LoopMode::Disabled);
            let (accelerated, skips) = run(LoopMode::Accelerate);
            assert_eq!(accelerated, stepped);
            assert_eq!(no_skips, 0);
            assert_eq!(skips, if condition { 97 } else { 0 });
        }
    }

    #[test]
    fn a_two_instruction_countdown_exits_at_the_same_instruction() {
        use fujimod_emulator_cpu::loops::LoopMode;

        let run = |mode| {
            let cpu = Core::new(Model::CortexA7).unwrap();
            cpu.map_zeroed(0x1000, 4096, 7).unwrap();
            for (address, word) in [
                (0x1000, 0xe250_0001u32), // subs r0, r0, #1
                (0x1004, 0x1aff_fffd),    // bne 0x1000
                (0x1008, 0xe3a0_1005),    // mov r1, #5
            ] {
                cpu.write_memory(address, &word.to_le_bytes()).unwrap();
            }
            cpu.write_register(arm::CORE[0], &100u32.to_le_bytes())
                .unwrap();
            cpu.set_armv7a_owner_exclusives(true).unwrap();
            cpu.set_loop_mode(mode).unwrap();
            cpu.set_loop_window(true).unwrap();
            cpu.run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(201),
            })
            .unwrap();
            assert_eq!(cpu.instruction_progress(), Some(201));
            assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 0) }, 0);
            assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 1) }, 5);
            (
                cpu.snapshot(None).unwrap().export().unwrap(),
                cpu.loops.skipped.get(),
            )
        };
        let (plain, _) = run(LoopMode::Disabled);
        let (fast, skipped) = run(LoopMode::Accelerate);
        assert_eq!(plain, fast);
        assert!(skipped > 100);
    }
}
