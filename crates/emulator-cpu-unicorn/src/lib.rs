//! Version-pinned Unicorn backend. Raw handles, register IDs and extension
//! calls are confined here; observers receive architectural execution events.
// Links the static Unicorn archive that `ffi` binds.
use fujimod_unicorn_sys as _;
#[cfg(target_os = "emscripten")]
mod available_memory;
pub mod accounting;
pub mod aprofile;
pub mod memory;
mod observers;
pub mod rprofile;
#[cfg(test)]
mod test_support;

use fujimod_emulator_cpu::*;
use std::{any::Any, ffi::c_void, ptr::NonNull};
mod architecture;
mod aarch64;
mod durable;
mod ffi;
#[cfg(test)]
mod hook_tlb_flush_tests;
pub mod jit;
use std::cell::{Cell, Ref, RefCell};
use std::collections::BTreeMap;
use std::ffi::CStr;
use std::rc::Rc;

#[cfg(target_os = "emscripten")]
pub use available_memory::remaining_heap_bytes;

pub struct Library {
    api: Rc<ffi::Api>,
}
impl Library {
    pub fn linked() -> Result<Self, String> {
        Self::checked(ffi::Api::linked()?)
    }
    fn checked(api: ffi::Api) -> Result<Self, String> {
        let api = Rc::new(api);
        let (mut major, mut minor) = (0, 0);
        let version = unsafe { (api.uc_version)(&mut major, &mut minor) };
        if major != 2 || minor != 1 || ((version >> 8) & 255) != 4 {
            return Err(format!("Unicorn 2.1.4 required; loaded {version:#x}"));
        }
        Ok(Self { api })
    }
    pub fn open(&self, model: Model, translation_cache_size_bytes: u32) -> Result<Core, String> {
        if model == Model::CortexM4 && self.api.extensions.features & 4 == 0 {
            return Err(
                "M-profile requires stoppable Thumb IT hooks from pinned Unicorn; see the crate README"
                    .into(),
            );
        }
        let (architecture, mode, id) = match model {
            Model::CortexR5F => (1, 0, 13i32),
            Model::CortexA7 => (1, 0, 14),
            Model::CortexM4 => (1, (1 << 4) | (1 << 5), 9),
            Model::ArmV8A => (2, 0, 1),
        };
        let mut raw = std::ptr::null_mut();
        check(&self.api, unsafe { (self.api.uc_open)(architecture, mode, &mut raw) })?;
        let core = Core {
            api: self.api.clone(),
            raw,
            model,
            identity: Identity::allocate(),
            hooks: RefCell::new(BTreeMap::new()),
            next_hook: Cell::new(1),
            translation: RefCell::new(None),
            thumb_metadata: Rc::new(Cell::new(false)),
            instruction_active: Rc::new(Cell::new(false)),
            callback_error: Rc::new(RefCell::new(None)),
            inspection: Rc::new(Inspection::default()),
            run_until: Cell::new(None),
            memory_regions: RefCell::new(None),
            #[cfg(test)]
            memory_region_queries: Cell::new(0),
            mmio: RefCell::new(BTreeMap::new()),
            mmio_contexts: RefCell::new(Vec::new()),
        };
        check(&core.api, unsafe {
            (core.api.uc_ctl)(raw, ffi::ctl(7, 1, 1), id)
        })?;
        // An absent end address means no address exit. In the legacy API,
        // passing zero instead makes address zero an exit, breaking low vectors.
        check(&core.api, unsafe {
            (core.api.uc_ctl)(raw, ffi::ctl(4, 1, 1), 1i32)
        })?;
        if matches!(model, Model::CortexA7 | Model::CortexM4 | Model::ArmV8A) {
            // UC_CTL_TLB_TYPE = 12, UC_TLB_CPU = 0 (pinned unicorn.h).
            // System emulation must apply the processor's MMU/MPU view.
            check(&core.api, unsafe {
                (core.api.uc_ctl)(raw, ffi::ctl(12, 1, 1), 0i32)
            })?;
        }
        check(&core.api, unsafe {
            (core.api.uc_ctl)(raw, ffi::ctl(13, 1, 1), translation_cache_size_bytes)
        })?;
        // Page-hook code invalidation requires translation-installed watches.
        // The AArch64 board still uses ordinary instruction observers.
        if let Some(enable) = core.api.extensions.page_hooks {
            check(&core.api, unsafe { enable(raw, model != Model::ArmV8A) })?;
        }
        Ok(core)
    }
}
fn check(api: &ffi::Api, status: i32) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(unsafe { CStr::from_ptr((api.uc_strerror)(status)) }
            .to_string_lossy()
            .into_owned())
    }
}
pub struct Core {
    api: Rc<ffi::Api>,
    raw: ffi::Engine,
    model: Model,
    identity: Identity,
    hooks: RefCell<BTreeMap<usize, StoredHook>>,
    next_hook: Cell<usize>,
    translation: RefCell<Option<Box<TranslationHooks>>>,
    thumb_metadata: Rc<Cell<bool>>,
    instruction_active: Rc<Cell<bool>>,
    callback_error: Rc<RefCell<Option<String>>>,
    inspection: Rc<Inspection>,
    run_until: Cell<Option<u64>>,
    memory_regions: RefCell<Option<Vec<MemoryRegion>>>,
    #[cfg(test)]
    memory_region_queries: Cell<u64>,
    // Stable callback contexts survive partial unmaps and remapping. They are
    // released only after uc_close has destroyed all remaining mappings.
    #[allow(clippy::vec_box)]
    mmio_contexts: RefCell<Vec<Box<MmioData>>>,
    mmio: RefCell<BTreeMap<u64, MmioMapping>>,
}
impl Core {
    pub(crate) fn set_parallel_cpus(&self, enabled: bool) -> Result<(), String> {
        let set = self
            .api
            .extensions
            .set_parallel_cpus
            .ok_or("CPU backend has no per-engine parallel TCG control")?;
        check(&self.api, unsafe { set(self.raw, enabled) })
    }
    /// Test exit requests only at block entry. Device callbacks' stop requests
    /// then take effect at the next block; instruction-hook exits stay immediate.
    pub(crate) fn set_block_exits(&self, enabled: bool) -> Result<(), String> {
        let set = self
            .api
            .extensions
            .set_block_exits
            .ok_or("CPU backend has no block-entry exit control")?;
        check(&self.api, unsafe { set(self.raw, enabled) })
    }
    pub(crate) fn set_instruction_counter(&self, enabled: bool) -> Result<(), String> {
        let set = self
            .api
            .extensions
            .set_instruction_counter
            .ok_or("CPU backend has no translated instruction counter")?;
        check(&self.api, unsafe { set(self.raw, enabled) })
    }
    pub(crate) fn reset_instruction_counter(&self) -> Result<(), String> {
        let reset = self
            .api
            .extensions
            .reset_instruction_counter
            .ok_or("CPU backend has no translated instruction counter")?;
        check(&self.api, unsafe { reset(self.raw) })
    }
    pub(crate) fn instruction_counter(&self) -> Result<u64, String> {
        let read = self
            .api
            .extensions
            .instruction_counter
            .ok_or("CPU backend has no translated instruction counter")?;
        Ok(unsafe { read(self.raw) })
    }
}
#[derive(Default)]
struct Inspection {
    depth: Cell<usize>,
    blocked: Cell<bool>,
}
struct InspectionGuard<'a>(&'a Inspection);
impl Drop for InspectionGuard<'_> {
    fn drop(&mut self) {
        self.0.depth.set(self.0.depth.get() - 1);
    }
}
#[derive(Clone)]
struct MmioMapping {
    end: u64,
    callbacks: Mmio,
    fast_reads: Vec<FastMmioRead>,
    offset: u64,
}
struct MmioData {
    callbacks: Mmio,
    fast_reads: Vec<FastMmioRead>,
    inspection: Rc<Inspection>,
    offset: u64,
}
fn protection(access: Access) -> u32 {
    match access {
        Access::Read => 1,
        Access::Write => 2,
        Access::Fetch => 4,
    }
}
unsafe extern "C" fn mmio_read(_: ffi::Engine, offset: u64, size: u32, data: *mut c_void) -> u64 {
    let context = unsafe { &*data.cast::<MmioData>() };
    if context.inspection.depth.get() != 0 {
        context.inspection.blocked.set(true);
        return 0;
    }
    let offset = context.offset + offset;
    let tick = rprofile::approximate_guest_tick();
    context
        .fast_reads
        .iter()
        .find_map(|read| read.read_at(offset, size, tick))
        .unwrap_or_else(|| unsafe {
            (context.callbacks.read)(offset, size, context.callbacks.data)
        })
}
unsafe extern "C" fn mmio_write(
    _: ffi::Engine,
    offset: u64,
    size: u32,
    value: u64,
    data: *mut c_void,
) {
    let context = unsafe { &*data.cast::<MmioData>() };
    if context.inspection.depth.get() != 0 {
        context.inspection.blocked.set(true);
        return;
    }
    unsafe {
        (context.callbacks.write)(context.offset + offset, size, value, context.callbacks.data)
    }
}
impl Drop for Core {
    fn drop(&mut self) {
        unsafe {
            (self.api.uc_close)(self.raw);
        }
    }
}
struct StoredHook {
    native: usize,
    _context: Box<HookData>,
}
struct HookData {
    hook: Hook,
    thumb_metadata: Rc<Cell<bool>>,
    instruction_active: Rc<Cell<bool>>,
    error: Rc<RefCell<Option<String>>>,
    api: Rc<ffi::Api>,
}
impl HookData {
    fn fail(&self, raw: ffi::Engine, message: String) {
        *self.error.borrow_mut() = Some(message);
        unsafe {
            (self.api.uc_emu_stop)(raw);
        }
    }
}
struct Context {
    api: Rc<ffi::Api>,
    raw: ffi::Context,
    owner: Identity,
    durable: Result<durable::Architecture, String>,
}
impl Drop for Context {
    fn drop(&mut self) {
        unsafe {
            (self.api.uc_context_free)(self.raw);
        }
    }
}
impl Snapshot for Context {
    fn owner(&self) -> Identity {
        self.owner
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn export(&self) -> Result<Vec<u8>, String> {
        durable::export(self.durable.as_ref().map_err(Clone::clone)?)
    }
}
fn register_id(register: Register) -> Result<i32, String> {
    use arm::Register as R;
    match register {
        Register::Pc => Ok(11),
        Register::Sp => Ok(12),
        Register::Pstate => Ok(3),
        Register::Arm32(R::R(n @ 0..=12)) => Ok(66 + i32::from(n)),
        Register::Arm32(R::R(13)) => Ok(12),
        Register::Arm32(R::R(14)) => Ok(10),
        Register::Arm32(R::R(15)) => Ok(11),
        Register::Arm32(R::Cpsr) => Ok(3),
        Register::Arm32(R::Spsr) => Ok(13),
        Register::Arm32(R::Fpexc) => Ok(4),
        Register::Arm32(R::Cpacr) => Ok(111),
        Register::Diagnostic(Diagnostic::ExceptionSyndrome) => Ok(140),
        Register::Arm32(R::Fpscr) => Ok(6),
        Register::Arm32(R::Fpsid) => Ok(8),
        Register::Arm32(R::D(n @ 0..=31)) => Ok(14 + i32::from(n)),
        Register::Arm32(R::Q(n @ 0..=15)) => Ok(50 + i32::from(n)),
        Register::Arm32(R::S(n @ 0..=31)) => Ok(79 + i32::from(n)),
        Register::Arm32(R::Ipsr) => Ok(114),
        Register::Arm32(R::Msp) => Ok(115),
        Register::Arm32(R::Psp) => Ok(116),
        Register::Arm32(R::Control) => Ok(117),
        Register::Arm32(R::Xpsr) => Ok(120),
        Register::Arm32(R::Primask) => Ok(123),
        Register::Arm32(R::Basepri) => Ok(124),
        Register::Arm32(R::Faultmask) => Ok(126),
        _ => Err(format!(
            "register {register:?} is not supported by this ARM32 backend"
        )),
    }
}
fn range(range: HookRange) -> (u64, u64) {
    match range {
        HookRange::All => (1, 0),
        HookRange::Inclusive { begin, end } => (begin, end),
    }
}
impl Core {
    fn check(&self, status: i32) -> Result<(), String> {
        check(&self.api, status)
    }
    fn inspect(&self, operation: impl FnOnce() -> i32) -> Result<(), String> {
        if self.inspection.depth.get() == 0 {
            self.inspection.blocked.set(false);
        }
        self.inspection.depth.set(self.inspection.depth.get() + 1);
        let _guard = InspectionGuard(&self.inspection);
        let result = self.check(operation());
        if self.inspection.blocked.get() {
            Err("host inspection cannot access MMIO".into())
        } else {
            result
        }
    }
    unsafe fn map_io(
        &self,
        address: u64,
        bytes: usize,
        callbacks: Mmio,
        fast_reads: Vec<FastMmioRead>,
        offset: u64,
    ) -> Result<(), String> {
        let end = address
            .checked_add(bytes as u64)
            .ok_or("MMIO address overflow")?;
        let mut context = Box::new(MmioData {
            callbacks,
            fast_reads: fast_reads.clone(),
            inspection: self.inspection.clone(),
            offset,
        });
        let pointer = (&mut *context as *mut MmioData).cast();
        self.check(unsafe {
            (self.api.uc_mmio_map)(
                self.raw,
                address,
                bytes as u64,
                mmio_read as *mut c_void,
                pointer,
                mmio_write as *mut c_void,
                pointer,
            )
        })?;
        self.invalidate_memory_regions();
        self.mmio_contexts.borrow_mut().push(context);
        self.mmio.borrow_mut().insert(
            address,
            MmioMapping {
                end,
                callbacks,
                fast_reads,
                offset,
            },
        );
        Ok(())
    }
    fn affected_mmio(&self, address: u64, bytes: usize) -> Result<Vec<(u64, MmioMapping)>, String> {
        let end = address
            .checked_add(bytes as u64)
            .ok_or("memory range overflow")?;
        Ok(self
            .mmio
            .borrow()
            .range(..end)
            .filter_map(|(&base, mapping)| {
                (address < mapping.end).then_some((base, mapping.clone()))
            })
            .collect())
    }
    fn normalize_mmio(&self, affected: Vec<(u64, MmioMapping)>) -> Result<(), String> {
        if affected.is_empty() {
            return Ok(());
        }
        let regions = self.cached_memory_regions()?.to_vec();
        for (base, mapping) in affected {
            let fragments = regions
                .iter()
                .filter(|r| r.begin < mapping.end && base <= r.end)
                .collect::<Vec<_>>();
            if fragments.len() == 1
                && fragments[0].begin == base
                && fragments[0].end + 1 == mapping.end
            {
                continue;
            }
            self.mmio.borrow_mut().remove(&base);
            for fragment in fragments {
                if fragment.begin < base || fragment.end >= mapping.end {
                    return Err("backend MMIO fragment exceeds its original mapping".into());
                }
                let bytes = fragment.end - fragment.begin + 1;
                // Unicorn rebases callbacks when unmap/protect splits MMIO.
                // Rebind only those fragments, retaining original offsets and
                // permissions. Large windows remain a single native mapping.
                self.check(unsafe { (self.api.uc_mem_unmap)(self.raw, fragment.begin, bytes) })?;
                self.invalidate_memory_regions();
                unsafe {
                    self.map_io(
                        fragment.begin,
                        bytes as usize,
                        mapping.callbacks,
                        mapping.fast_reads.clone(),
                        mapping.offset + fragment.begin - base,
                    )?;
                }
                self.check(unsafe {
                    (self.api.uc_mem_protect)(self.raw, fragment.begin, bytes, fragment.permissions)
                })?;
                self.invalidate_memory_regions();
            }
        }
        Ok(())
    }
    fn cached_memory_regions(&self) -> Result<Ref<'_, [MemoryRegion]>, String> {
        if self.memory_regions.borrow().is_none() {
            let regions = self.query_memory_regions()?;
            *self.memory_regions.borrow_mut() = Some(regions);
        }
        Ok(Ref::map(self.memory_regions.borrow(), |regions| {
            regions.as_deref().expect("region cache populated")
        }))
    }
    fn query_memory_regions(&self) -> Result<Vec<MemoryRegion>, String> {
        #[cfg(test)]
        self.memory_region_queries
            .set(self.memory_region_queries.get().saturating_add(1));
        let mut raw = std::ptr::null_mut();
        let mut count = 0;
        self.check(unsafe { (self.api.uc_mem_regions)(self.raw, &mut raw, &mut count) })?;
        let regions = if count == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(raw, count as usize) }
                .iter()
                .map(|r| MemoryRegion {
                    begin: r.begin,
                    end: r.end,
                    permissions: r.perms,
                })
                .collect()
        };
        self.check(unsafe { (self.api.uc_free)(raw.cast()) })?;
        Ok(regions)
    }
    fn invalidate_memory_regions(&self) {
        *self.memory_regions.borrow_mut() = None;
    }
    fn hook_key(&self, hook: HookId) -> Result<usize, String> {
        if hook.owner != self.identity || !self.hooks.borrow().contains_key(&hook.key) {
            return Err("hook belongs to a different CPU or was removed".into());
        }
        Ok(self.hooks.borrow().get(&hook.key).unwrap().native)
    }
}
impl Cpu for Core {
    fn identity(&self) -> Identity {
        self.identity
    }
    fn model(&self) -> Model {
        self.model
    }
    fn features(&self) -> Features {
        let e = &self.api.extensions;
        let frontend = self.model != Model::ArmV8A
            || e.features & ffi::FEATURE_AARCH64_ACCOUNTING != 0;
        Features {
            asynchronous_runs: false,
            guest_clock: false,
            architectural_exits: false,
            tighten_instruction_limit: false,
            instruction_limit: true,
            shared_physical_maps: false,
            instruction_progress: false,
            translation_filter: frontend && e.insn_filter.is_some()
                && e.retry_block.is_some()
                && e.request_restart.is_some(),
            // The R-profile backend writes the appended pause pointer. Require
            // its ABI bit before exposing the accounting record to that backend.
            block_accounting: frontend && e.features & 2 != 0
                && e.features & ffi::FEATURE_PAUSE_ACCOUNTING_LAYOUT != 0
                && e.features & ffi::FEATURE_PARALLEL_TCG != 0
                && e.set_parallel_cpus.is_some()
                && e.accounting.is_some(),
            watched_stores: e.hook_watched.is_some() && e.watch.is_some(),
            backing_observation: e.observe_backings.is_some(),
            store_pc_control: e.write_hook_pc_sync.is_some(),
        }
    }
    fn set_execution_policy(&self, _policy: ExecutionPolicy) -> Result<(), String> {
        // Unicorn keeps exact instruction limits for both policies. The board
        // decides whether to serialize this synchronous backend with peers.
        Ok(())
    }
    fn read_register(&self, register: Register, out: &mut [u8]) -> Result<usize, String> {
        let mut size = out.len();
        if self.model == Model::ArmV8A {
            return self.read_aarch64(register, out);
        }
        let id = register_id(register)?;
        self.check(unsafe {
            (self.api.uc_reg_read2)(self.raw, id, out.as_mut_ptr().cast(), &mut size)
        })?;
        if size > out.len() {
            return Err("CPU register exceeds supplied output buffer".into());
        }
        Ok(size)
    }
    fn write_register(&self, register: Register, bytes: &[u8]) -> Result<(), String> {
        if self.model == Model::ArmV8A {
            return self.write_aarch64(register, bytes);
        }
        let mut size = bytes.len();
        let id = register_id(register)?;
        if matches!(register, arm::PC | Register::Pc) {
            if bytes.len() < 4 {
                return Err("PC write requires four bytes".into());
            }
            // In pinned 2.1.4, write2 omits skip_sync_pc_on_exit. A PC change
            // from an instruction observer must survive the translated block
            // exit; the scalar write API supplies that guarantee (uc.c).
            return self
                .check(unsafe { (self.api.uc_reg_write)(self.raw, id, bytes.as_ptr().cast()) });
        }
        self.check(unsafe {
            (self.api.uc_reg_write2)(self.raw, id, bytes.as_ptr().cast(), &mut size)
        })
    }
    fn coprocessor(&self, register: arm::Coprocessor, value: Option<u64>) -> Result<u64, String> {
        if self.model == Model::ArmV8A {
            return Err("ARM32 coprocessor selector on an AArch64 processor".into());
        }
        let mut raw = ffi::CpReg {
            cp: register.cp,
            is64: u32::from(register.wide),
            sec: register.secure,
            crn: register.crn,
            crm: register.crm,
            opc1: register.opc1,
            opc2: register.opc2,
            val: value.unwrap_or(0),
        };
        let status = unsafe {
            if value.is_some() {
                (self.api.uc_reg_write)(self.raw, 139, (&raw as *const ffi::CpReg).cast())
            } else {
                (self.api.uc_reg_read)(self.raw, 139, (&mut raw as *mut ffi::CpReg).cast())
            }
        };
        self.check(status)?;
        Ok(raw.val)
    }
    fn read_memory(&self, address: u64, bytes: &mut [u8]) -> Result<(), String> {
        self.inspect(|| unsafe {
            (self.api.uc_mem_read)(
                self.raw,
                address,
                bytes.as_mut_ptr().cast(),
                bytes.len() as u64,
            )
        })
    }
    fn translate(&self, address: u64, access: Access) -> Result<u64, String> {
        let mut physical = 0;
        self.inspect(|| unsafe {
            (self.api.uc_vmem_translate)(self.raw, address, protection(access), &mut physical)
        })?;
        Ok(physical)
    }
    fn read_virtual(&self, address: u64, access: Access, bytes: &mut [u8]) -> Result<(), String> {
        address
            .checked_add(bytes.len() as u64)
            .ok_or("virtual read range overflow")?;
        let mut page: u32 = 0;
        self.check(unsafe {
            (self.api.uc_ctl)(self.raw, ffi::ctl(1, 1, 2), &mut page as *mut u32)
        })?;
        if !page.is_power_of_two() {
            return Err("invalid backend page size".into());
        }
        // uc_vmem_read uses a fast host-pointer lookup, which refuses writable
        // pages carrying a write observer/dirty flag. Translate with the requested
        // access, then use the observational physical read instead.
        let regions = self.memory_regions()?;
        let mut offset = 0;
        while offset < bytes.len() {
            let at = address + offset as u64;
            let n = (bytes.len() - offset).min((page as u64 - (at & (page as u64 - 1))) as usize);
            let physical = self.translate(at, access)?;
            let end = physical
                .checked_add(n as u64 - 1)
                .ok_or("physical read range overflow")?;
            if !regions.iter().any(|r| {
                r.begin <= physical && end <= r.end && r.permissions & protection(access) != 0
            }) {
                return Err(format!(
                    "physical memory denies {access:?} inspection at {physical:#x}"
                ));
            }
            self.read_memory(physical, &mut bytes[offset..offset + n])?;
            offset += n;
        }
        Ok(())
    }
    fn write_memory(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
        self.check(unsafe {
            (self.api.uc_mem_write)(self.raw, address, bytes.as_ptr().cast(), bytes.len() as u64)
        })
    }
    fn memory_regions(&self) -> Result<Vec<MemoryRegion>, String> {
        Ok(self.cached_memory_regions()?.to_vec())
    }
    unsafe fn map_backing(
        &self,
        address: u64,
        bytes: usize,
        permissions: u32,
        backing: NonNull<u8>,
    ) -> Result<(), String> {
        self.check(unsafe {
            (self.api.uc_mem_map_ptr)(
                self.raw,
                address,
                bytes as u64,
                permissions,
                backing.as_ptr().cast(),
            )
        })?;
        // The pointer-mapping API records permissions without setting QEMU's
        // read-only backing flag. Apply them through the protection API too,
        // so CPU-managed translation cannot write through a ROM mapping.
        self.protect(address, bytes, permissions)?;
        self.invalidate_memory_regions();
        Ok(())
    }
    unsafe fn replace_physical(
        &self,
        address: u64,
        _physical: u64,
        bytes: usize,
        permissions: u32,
        backing: NonNull<u8>,
    ) -> Result<(), String> {
        if self.model != Model::CortexA7
            || address & 0xfff != 0
            || bytes == 0
            || !bytes.is_multiple_of(4096)
            || permissions & !7 != 0
            || address
                .checked_add(bytes as u64)
                .is_none_or(|end| end > 1 << 32)
        {
            return Err("Unsupported physical window replacement".into());
        }
        let replace = self
            .api
            .extensions
            .replace_backing
            .ok_or("CPU backend has no physical window replacement")?;
        self.check(unsafe {
            replace(
                self.raw,
                address,
                bytes as u64,
                permissions,
                backing.as_ptr().cast(),
            )
        })?;
        self.invalidate_memory_regions();
        Ok(())
    }
    fn map_zeroed(&self, address: u64, bytes: usize, permissions: u32) -> Result<(), String> {
        self.check(unsafe { (self.api.uc_mem_map)(self.raw, address, bytes as u64, permissions) })?;
        self.invalidate_memory_regions();
        Ok(())
    }
    unsafe fn map_mmio(&self, address: u64, bytes: usize, callbacks: Mmio) -> Result<(), String> {
        unsafe { self.map_io(address, bytes, callbacks, Vec::new(), 0) }
    }
    unsafe fn map_mmio_fast(
        &self,
        address: u64,
        bytes: usize,
        callbacks: Mmio,
        fast_reads: Vec<FastMmioRead>,
    ) -> Result<(), String> {
        unsafe { self.map_io(address, bytes, callbacks, fast_reads, 0) }
    }
    fn unmap(&self, address: u64, bytes: usize) -> Result<(), String> {
        let affected = self.affected_mmio(address, bytes)?;
        self.check(unsafe { (self.api.uc_mem_unmap)(self.raw, address, bytes as u64) })?;
        self.invalidate_memory_regions();
        self.normalize_mmio(affected)
    }
    fn protect(&self, address: u64, bytes: usize, permissions: u32) -> Result<(), String> {
        let affected = self.affected_mmio(address, bytes)?;
        self.check(unsafe {
            (self.api.uc_mem_protect)(self.raw, address, bytes as u64, permissions)
        })?;
        self.invalidate_memory_regions();
        self.normalize_mmio(affected)
    }
    fn invalidate_code(&self, range: Option<(u64, u64)>) -> Result<(), String> {
        self.check(unsafe {
            match range {
                Some((start, end)) => (self.api.uc_ctl)(self.raw, ffi::ctl(9, 2, 1), start, end),
                None => (self.api.uc_ctl)(self.raw, ffi::ctl(10, 0, 1)),
            }
        })
    }
    fn run(&self, request: Run) -> Result<Exit, ExecutionError> {
        if request.instructions == Some(0) {
            return Ok(Exit::Stopped);
        }
        if request.until != self.run_until.get() {
            let address = request.until.unwrap_or(0);
            self.check(unsafe {
                (self.api.uc_ctl)(
                    self.raw,
                    ffi::ctl(6, 2, 1),
                    &address as *const u64,
                    usize::from(request.until.is_some()),
                )
            })?;
            // Address exits are compiled into translation blocks.
            self.invalidate_code(None)?;
            self.run_until.set(request.until);
        }
        self.callback_error.borrow_mut().take();
        let status = unsafe {
            (self.api.uc_emu_start)(
                self.raw,
                request.begin,
                request.until.unwrap_or(0),
                request.watchdog_us,
                request.instructions.unwrap_or(0),
            )
        };
        if let Some(error) = self.callback_error.borrow_mut().take() {
            return Err(ExecutionError::Backend(error));
        }
        if let Err(message) = self.check(status) {
            // A faulting access leaves the PC at the instruction that made it.
            let mut pc = [0u8; 4];
            let message = match self.read_register(Register::Pc, &mut pc) {
                Ok(_) => format!("{message} at PC {:#x}", u32::from_le_bytes(pc)),
                Err(_) => message,
            };
            return Err(match status {
                10 => ExecutionError::InvalidInstruction(message),
                6 => ExecutionError::Memory {
                    fault: Fault::ReadUnmapped,
                    message,
                },
                7 => ExecutionError::Memory {
                    fault: Fault::WriteUnmapped,
                    message,
                },
                8 => ExecutionError::Memory {
                    fault: Fault::FetchUnmapped,
                    message,
                },
                12 => ExecutionError::Memory {
                    fault: Fault::WriteProtected,
                    message,
                },
                13 => ExecutionError::Memory {
                    fault: Fault::ReadProtected,
                    message,
                },
                14 => ExecutionError::Memory {
                    fault: Fault::FetchProtected,
                    message,
                },
                _ => ExecutionError::Backend(message),
            });
        }
        Ok(Exit::Stopped)
    }
    fn stop(&self) -> Result<(), String> {
        self.check(unsafe { (self.api.uc_emu_stop)(self.raw) })
    }
    fn stop_before_instruction(&self) -> Result<(), String> {
        if !self.instruction_active.replace(false) {
            return Err("instruction interception requires a live, unconsumed entry".into());
        }
        self.stop()
    }
    fn snapshot(&self, _previous: Option<&dyn Snapshot>) -> Result<Box<dyn Snapshot>, String> {
        let mut raw = std::ptr::null_mut();
        self.check(unsafe { (self.api.uc_context_alloc)(self.raw, &mut raw) })?;
        let mut context = Context {
            api: self.api.clone(),
            raw,
            owner: self.identity,
            durable: Err("processor architecture has not been captured".into()),
        };
        self.check(unsafe { (self.api.uc_context_save)(self.raw, raw) })?;
        context.durable = durable::capture(self, &context);
        Ok(Box::new(context))
    }
    fn restore(&self, snapshot: &dyn Snapshot) -> Result<(), String> {
        if snapshot.owner() != self.identity {
            return Err("CPU snapshot belongs to a different processor".into());
        }
        if let Some(imported) = snapshot.as_any().downcast_ref::<durable::Imported>() {
            return imported.restore(self);
        }
        let context = snapshot
            .as_any()
            .downcast_ref::<Context>()
            .ok_or("CPU snapshot belongs to a different backend")?;
        self.check(unsafe { (self.api.uc_context_restore)(self.raw, context.raw) })
    }
    fn import_snapshot(&self, bytes: &[u8]) -> Result<Box<dyn Snapshot>, String> {
        durable::import(self, bytes)
    }
    unsafe fn install_hook(&self, hook: Hook) -> Result<HookId, String> {
        let (kind, callback, span) = match hook {
            Hook::Instruction { range, .. } => (4, instruction as *const (), range),
            Hook::Block { range, .. } => (8, block as *const (), range),
            Hook::Exception { .. } => (1, exception as *const (), HookRange::All),
            Hook::HostWrite { .. } => {
                return Err("host-write hooks require the R-profile backend".into());
            }
            Hook::Memory {
                read, write, range, ..
            } => {
                if !read && !write {
                    return Err("memory hook has no selected accesses".into());
                }
                (
                    (i32::from(read) << 10) | (i32::from(write) << 11),
                    memory as *const (),
                    range,
                )
            }
            Hook::Fault { .. } => (0x3f0, fault as *const (), HookRange::All),
            Hook::CompletedStore { .. } => {
                return Err("completed-store hooks require the R-profile execution adapter".into());
            }
            Hook::Synchronization { .. } => {
                return Err(
                    "owner synchronization hooks require the R-profile execution adapter".into(),
                );
            }
        };
        let mut data = Box::new(HookData {
            hook,
            thumb_metadata: self.thumb_metadata.clone(),
            instruction_active: self.instruction_active.clone(),
            error: self.callback_error.clone(),
            api: self.api.clone(),
        });
        let (begin, end) = range(span);
        let mut key = 0;
        self.check(unsafe {
            (self.api.uc_hook_add)(
                self.raw,
                &mut key,
                kind,
                callback.cast_mut().cast(),
                (&mut *data as *mut HookData).cast(),
                begin,
                end,
            )
        })?;
        let native = key;
        let key = self.next_hook.get();
        let Some(next) = key.checked_add(1) else {
            self.check(unsafe { (self.api.uc_hook_del)(self.raw, native) })?;
            return Err("CPU hook identity overflow".into());
        };
        self.next_hook.set(next);
        self.hooks.borrow_mut().insert(
            key,
            StoredHook {
                native,
                _context: data,
            },
        );
        Ok(HookId {
            owner: self.identity,
            key,
        })
    }
    fn remove_hook(&self, hook: HookId) -> Result<(), String> {
        let key = self.hook_key(hook)?;
        self.check(unsafe { (self.api.uc_hook_del)(self.raw, key) })?;
        self.hooks.borrow_mut().remove(&hook.key);
        Ok(())
    }
    unsafe fn translation_hooks(&self, hooks: Option<TranslationHooks>) -> Result<(), String> {
        if !self.features().translation_filter {
            return Err("CPU backend has no translation observer".into());
        }
        let install = self.api.extensions.insn_filter.unwrap();
        let mut context = hooks.map(Box::new);
        let data = context.as_mut().map_or(std::ptr::null_mut(), |h| {
            (&mut **h as *mut TranslationHooks).cast()
        });
        let on = context.is_some();
        self.check(unsafe {
            install(
                self.raw,
                on.then_some(classify),
                on.then_some(restart),
                data,
            )
        })?;
        *self.translation.borrow_mut() = context;
        self.thumb_metadata
            .set(on && self.api.extensions.features & 1 != 0);
        Ok(())
    }
    fn retry_block(&self, instructions: u32) -> Result<(), String> {
        let retry = self
            .api
            .extensions
            .retry_block
            .ok_or("CPU backend has no block retry")?;
        self.check(unsafe { retry(self.raw, instructions) })
    }
    fn request_restart(&self) -> Result<(), String> {
        let restart = self
            .api
            .extensions
            .request_restart
            .ok_or("CPU backend has no instruction restart")?;
        self.check(unsafe { restart(self.raw) })
    }
    fn accounting(&self) -> Option<NonNull<BlockAccounting>> {
        if !self.features().block_accounting {
            return None;
        }
        NonNull::new(unsafe { self.api.extensions.accounting.unwrap()(self.raw) }.cast())
    }
    unsafe fn observe_backings(&self, ranges: &[BackingObserver]) -> Result<(), String> {
        let observe = self
            .api
            .extensions
            .observe_backings
            .ok_or("CPU backend has no backing observer")?;
        self.check(unsafe { observe(self.raw, ranges.as_ptr(), ranges.len()) })
    }
    fn watch_stores(&self, address: u64, bytes: u64) -> Result<(), String> {
        let watch = self
            .api
            .extensions
            .watch
            .ok_or("CPU backend has no watched stores")?;
        self.check(unsafe { watch(self.raw, address, bytes) })
    }
    fn hook_watched(&self, hook: HookId, watched: bool) -> Result<(), String> {
        let key = self.hook_key(hook)?;
        let set = self
            .api
            .extensions
            .hook_watched
            .ok_or("CPU backend has no watched hooks")?;
        self.check(unsafe { set(self.raw, key, watched) })
    }
    fn precise_store_pc(&self, on: bool) -> Result<(), String> {
        let set = self
            .api
            .extensions
            .write_hook_pc_sync
            .ok_or("CPU backend has no store-PC control")?;
        self.check(unsafe { set(self.raw, on) })
    }
    fn exit_counters(&self) -> Result<BTreeMap<&'static str, u64>, String> {
        let read = self
            .api
            .extensions
            .exit_counters
            .ok_or("CPU backend has no exit counters")?;
        let names = [
            "cpu_loop_exit",
            "cpu_loop_exit_restore",
            "cpu_loop_exit_noexc",
            "cpu_loop_exit_atomic",
            "check_exit_restore",
            "check_exit_no_sync",
            "embedder_retry",
            "restart_requests",
            "retry_requests",
            "stop_requests",
            "normal_check_exit",
            "normal_retry",
            "translations",
            "limited_translations",
            "translation_flushes",
            "translation_invalidations",
        ];
        let values = unsafe { std::slice::from_raw_parts(read(self.raw), names.len()) };
        Ok(names.into_iter().zip(values.iter().copied()).collect())
    }
}

unsafe extern "C" fn instruction(_: ffi::Engine, address: u64, size: u32, data: *mut c_void) {
    let data = unsafe { &*data.cast::<HookData>() };
    if let Hook::Instruction { callback, data: argument, .. } = data.hook {
        data.instruction_active.set(true);
        unsafe { callback(address, size, argument); }
        data.instruction_active.set(false);
    }
}
unsafe extern "C" fn block(_: ffi::Engine, address: u64, size: u32, data: *mut c_void) {
    let context = unsafe { &*data.cast::<HookData>() };
    if let Hook::Block { callback, data, .. } = context.hook {
        let thumb = context.thumb_metadata.get().then_some(size >> 31 != 0);
        unsafe {
            callback(
                Block {
                    address,
                    bytes: if thumb.is_some() {
                        size & 0x3fffffff
                    } else {
                        size
                    },
                    thumb,
                },
                data,
            );
        }
    }
}
unsafe extern "C" fn exception(_: ffi::Engine, number: u32, data: *mut c_void) {
    let data = unsafe { &*data.cast::<HookData>() };
    let exception = match number {
        1 => arm::Exception::UndefinedInstruction,
        2 => arm::Exception::SupervisorCall,
        3 => arm::Exception::PrefetchAbort,
        4 => arm::Exception::DataAbort,
        5 => arm::Exception::Irq,
        6 => arm::Exception::Fiq,
        7 => arm::Exception::Breakpoint,
        n => arm::Exception::Unknown(n),
    };
    if let Hook::Exception { callback, data } = data.hook {
        unsafe {
            callback(exception, data);
        }
    }
}
unsafe extern "C" fn memory(
    raw: ffi::Engine,
    kind: i32,
    address: u64,
    size: i32,
    value: i64,
    data: *mut c_void,
) {
    let context = unsafe { &*data.cast::<HookData>() };
    let access = match kind {
        16 => Access::Read,
        17 => Access::Write,
        18 => Access::Fetch,
        _ => {
            context.fail(raw, format!("unknown backend memory access {kind}"));
            return;
        }
    };
    if let Hook::Memory { callback, data, .. } = context.hook {
        unsafe {
            callback(access, address, size, value, data);
        }
    }
}
unsafe extern "C" fn fault(
    raw: ffi::Engine,
    kind: i32,
    address: u64,
    size: i32,
    value: i64,
    data: *mut c_void,
) -> bool {
    let context = unsafe { &*data.cast::<HookData>() };
    let fault = match kind {
        19 => Fault::ReadUnmapped,
        20 => Fault::WriteUnmapped,
        21 => Fault::FetchUnmapped,
        22 => Fault::WriteProtected,
        23 => Fault::ReadProtected,
        24 => Fault::FetchProtected,
        _ => {
            context.fail(raw, format!("unknown backend memory fault {kind}"));
            return false;
        }
    };
    if let Hook::Fault { callback, data } = context.hook {
        unsafe { callback(fault, address, size, value, data) }
    } else {
        false
    }
}
unsafe extern "C" fn classify(data: *mut c_void, address: u64, word: u32) -> i32 {
    let hooks = unsafe { &*data.cast::<TranslationHooks>() };
    let decision = unsafe { (hooks.classify)(hooks.data, address, word) };
    let action = match decision.instruction {
        InstructionDisposition::Execute => 0,
        InstructionDisposition::Observe => 1,
        InstructionDisposition::ObserveAndEndBlock => 2,
        InstructionDisposition::Replace => 3,
    };
    action | if decision.observe_block { 4 } else { 0 }
}
unsafe extern "C" fn restart(data: *mut c_void, pc: u64) {
    let hooks = unsafe { &*data.cast::<TranslationHooks>() };
    unsafe {
        (hooks.restart)(hooks.data, pc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::UnsafeCell;
    #[derive(Default)]
    struct Probe {
        instructions: u64,
        stores: Vec<(Access, u64, i32, i64)>,
    }
    unsafe fn instruction_probe(_: u64, _: u32, data: *mut c_void) {
        unsafe {
            (*data.cast::<Probe>()).instructions += 1;
        }
    }
    unsafe fn store_probe(access: Access, address: u64, size: i32, value: i64, data: *mut c_void) {
        unsafe {
            (*data.cast::<Probe>())
                .stores
                .push((access, address, size, value));
        }
    }
    use crate::test_support::library;
    fn reg(cpu: &dyn Cpu, register: Register) -> u32 {
        let mut bytes = [0; 4];
        assert_eq!(cpu.read_register(register, &mut bytes).unwrap(), 4);
        u32::from_le_bytes(bytes)
    }
    #[test]
    fn absent_address_exit_allows_zero_and_changing_explicit_exits_retranslates_code() {
        let cpu = library().open(Model::CortexR5F, 1024 * 1024).unwrap();
        cpu.map_zeroed(0, 0x1000, 7).unwrap();
        cpu.write_memory(
            0,
            &[0xe2800001u32, 0xeafffffdu32]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        for (begin, until, instructions, value) in [
            (0, None, 3, 2),
            (0, Some(4), 20, 3),
            (4, None, 2, 4),
            (0, Some(4), 20, 5),
            (0, None, 1, 6),
        ] {
            cpu.run(Run {
                begin,
                until,
                instructions: Some(instructions),
                watchdog_us: 1_000_000,
            })
            .unwrap_or_else(|e| panic!("{begin:#x}/{until:?}: {e}"));
            assert_eq!(reg(&cpu, arm::CORE[0]), value, "{begin:#x}/{until:?}");
        }
    }
    #[test]
    fn synthetic_arm_runs_through_typed_registers_maps_hooks_and_snapshots() {
        let library = library();
        let words = [0xe3a0002au32, 0xe2800001, 0xe5810000, 0xe3a02007];
        let code: Vec<UnsafeCell<u8>> = (0..4096).map(|_| UnsafeCell::new(0)).collect();
        let data: Vec<UnsafeCell<u8>> = (0..4096).map(|_| UnsafeCell::new(0)).collect();
        for (at, byte) in words.into_iter().flat_map(u32::to_le_bytes).enumerate() {
            unsafe {
                *code[at].get() = byte;
            }
        }
        // Owned cells outlive every mapping; no Rust slice aliases guest writes.
        let cpu = library.open(Model::CortexR5F, 4 * 1024 * 1024).unwrap();
        unsafe {
            let pointer = NonNull::new(code.as_ptr().cast::<u8>().cast_mut()).unwrap();
            cpu.map_backing(0x1000, 4096, 7, pointer).unwrap();
            cpu.map_backing(0x40001000, 4096, 7, pointer).unwrap();
            cpu.map_backing(
                0x2000,
                4096,
                7,
                NonNull::new(data.as_ptr().cast::<u8>().cast_mut()).unwrap(),
            )
            .unwrap();
        }
        let mut probe = Box::<Probe>::default();
        let pointer = (&mut *probe as *mut Probe).cast();
        let instruction = unsafe {
            cpu.install_hook(Hook::Instruction {
                callback: instruction_probe,
                data: pointer,
                range: HookRange::All,
            })
        }
        .unwrap();
        let store = unsafe {
            cpu.install_hook(Hook::Memory {
                callback: store_probe,
                data: pointer,
                read: false,
                write: true,
                range: HookRange::All,
            })
        }
        .unwrap();
        cpu.write_register(arm::CORE[1], &0x2000u32.to_le_bytes())
            .unwrap();
        let pc = reg(&cpu, Register::Pc);
        cpu.run(Run {
            begin: 0xdead0000,
            until: None,
            watchdog_us: 0,
            instructions: Some(0),
        })
        .unwrap();
        assert_eq!(reg(&cpu, Register::Pc), pc);
        assert_eq!(probe.instructions, 0);
        cpu.run(Run {
            begin: 0x1000,
            until: None,
            watchdog_us: 0,
            instructions: Some(2),
        })
        .unwrap();
        assert_eq!(reg(&cpu, arm::CORE[0]), 43);
        assert_eq!(reg(&cpu, arm::CORE[15]), 0x1008);
        assert_eq!(probe.instructions, 2);
        assert!(probe.stores.is_empty());
        cpu.run(Run {
            begin: 0x1008,
            until: Some(0x1010),
            watchdog_us: 0,
            instructions: None,
        })
        .unwrap();
        assert_eq!(reg(&cpu, arm::CORE[2]), 7);
        assert_eq!(probe.instructions, 4);
        assert_eq!(probe.stores, [(Access::Write, 0x2000, 4, 43)]);
        let mut bytes = [0; 4];
        cpu.read_memory(0x2000, &mut bytes).unwrap();
        assert_eq!(u32::from_le_bytes(bytes), 43);
        let snapshot = cpu.snapshot(None).unwrap();
        cpu.write_register(arm::CORE[0], &99u32.to_le_bytes())
            .unwrap();
        cpu.restore(snapshot.as_ref()).unwrap();
        assert_eq!(reg(&cpu, arm::CORE[0]), 43);
        let other = library.open(Model::CortexR5F, 4 * 1024 * 1024).unwrap();
        assert!(other.restore(snapshot.as_ref()).is_err());
        assert!(other.remove_hook(store).is_err());
        cpu.remove_hook(instruction).unwrap();
        cpu.remove_hook(store).unwrap();
        cpu.run(Run {
            begin: 0x40001000,
            until: None,
            watchdog_us: 0,
            instructions: Some(1),
        })
        .unwrap();
        assert_eq!(reg(&cpu, arm::CORE[0]), 42);
        assert_eq!(probe.instructions, 4);
        assert!(cpu.remove_hook(store).is_err());
    }
    struct Redirect<'a> {
        cpu: &'a dyn Cpu,
        error: Option<String>,
    }
    unsafe fn redirect_pc(address: u64, _: u32, data: *mut c_void) {
        let context = unsafe { &mut *data.cast::<Redirect<'_>>() };
        if address == 0x1000 {
            if let Err(error) = context
                .cpu
                .write_register(Register::Pc, &0x1004u32.to_le_bytes())
            {
                context.error = Some(error);
                let _ = context.cpu.stop();
            }
        }
    }
    #[test]
    fn observer_pc_write_survives_the_translation_exit() {
        let library = library();
        let cpu = library.open(Model::CortexR5F, 4 * 1024 * 1024).unwrap();
        cpu.map_zeroed(0x1000, 4096, 7).unwrap();
        let code = [0xe3a0002au32, 0xe3a01007]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        cpu.write_memory(0x1000, &code).unwrap();
        let mut context = Redirect {
            cpu: &cpu,
            error: None,
        };
        let hook = unsafe {
            cpu.install_hook(Hook::Instruction {
                callback: redirect_pc,
                data: (&mut context as *mut Redirect<'_>).cast(),
                range: HookRange::All,
            })
        }
        .unwrap();
        cpu.run(Run {
            begin: 0x1000,
            until: Some(0x1008),
            watchdog_us: 0,
            instructions: Some(4),
        })
        .unwrap();
        assert!(context.error.is_none());
        assert_eq!(reg(&cpu, arm::CORE[0]), 0);
        assert_eq!(reg(&cpu, arm::CORE[1]), 7);
        assert_eq!(reg(&cpu, arm::PC), 0x1008);
        cpu.remove_hook(hook).unwrap();
    }
    #[test]
    fn m4_thumb_execution_and_banked_stack_registers_use_the_same_backend() {
        let library = library();
        let cpu = library.open(Model::CortexM4, 4 * 1024 * 1024).unwrap();
        cpu.map_zeroed(0x1000, 0x2000, 7).unwrap();
        // MOVS R0,#7; STR R0,[R1]; MOVS R2,#3.
        cpu.write_memory(0x1000, &[0x07, 0x20, 0x08, 0x60, 0x03, 0x22])
            .unwrap();
        let msp = Register::Arm32(arm::Register::Msp);
        let psp = Register::Arm32(arm::Register::Psp);
        cpu.write_register(msp, &0x2ff0u32.to_le_bytes()).unwrap();
        cpu.write_register(psp, &0x2f00u32.to_le_bytes()).unwrap();
        cpu.write_register(arm::CORE[1], &0x2000u32.to_le_bytes())
            .unwrap();
        cpu.run(Run {
            begin: 0x1001,
            until: None,
            watchdog_us: 0,
            instructions: Some(2),
        })
        .unwrap();
        assert_eq!(reg(&cpu, arm::PC), 0x1004);
        assert_eq!(reg(&cpu, arm::CORE[2]), 0);
        let mut bytes = [0; 4];
        cpu.read_memory(0x2000, &mut bytes).unwrap();
        assert_eq!(u32::from_le_bytes(bytes), 7);
        let snapshot = cpu.snapshot(None).unwrap();
        cpu.write_register(msp, &0x2e00u32.to_le_bytes()).unwrap();
        cpu.restore(snapshot.as_ref()).unwrap();
        assert_eq!(reg(&cpu, msp), 0x2ff0);
        assert_eq!(reg(&cpu, Register::Sp), 0x2ff0);
        assert_eq!(reg(&cpu, Register::Pstate), reg(&cpu, arm::CPSR));
        assert_eq!(reg(&cpu, psp), 0x2f00);
    }
    #[test]
    fn m4_accepts_interactive_policy_while_preserving_exact_instruction_limits() {
        let cpu = library().open(Model::CortexM4, 4 * 1024 * 1024).unwrap();
        cpu.set_execution_policy(ExecutionPolicy::Interactive).unwrap();
        cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
        cpu.write_memory(0x1000, &[0x07, 0x20, 0x08, 0x22]).unwrap();
        cpu.run(Run {
            begin: 0x1001,
            until: None,
            watchdog_us: 0,
            instructions: Some(1),
        })
        .unwrap();
        assert_eq!(reg(&cpu, arm::PC), 0x1002);
        assert_eq!(reg(&cpu, arm::CORE[0]), 7);
    }
    #[test]
    fn a7_page_tables_translate_and_read_across_virtual_sections() {
        let library = library();
        let mut probe = Box::<MmioProbe>::default();
        let cpu = library.open(Model::CortexA7, 4 * 1024 * 1024).unwrap();
        cpu.map_zeroed(0, 0x200000, 7).unwrap();
        let table = [0x00000c02u32, 0, 0x00100c02, 0x00000c02]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        cpu.write_memory(0x4000, &table).unwrap();
        cpu.write_memory(0x100100, &[0x34, 0x12, 0xed, 0xfe])
            .unwrap();
        cpu.write_memory(0x1ffffe, &[1, 2]).unwrap();
        cpu.write_memory(0, &[3, 4]).unwrap();
        let cp = |crn| arm::Coprocessor {
            cp: 15,
            opc1: 0,
            crn,
            crm: 0,
            opc2: 0,
            secure: 0,
            wide: false,
        };
        cpu.coprocessor(cp(2), Some(0x4000)).unwrap(); // TTBR0
        cpu.coprocessor(cp(3), Some(0xffffffff)).unwrap(); // DACR
        let sctlr = cpu.coprocessor(cp(1), None).unwrap();
        cpu.coprocessor(cp(1), Some(sctlr | 1)).unwrap();
        for access in [Access::Read, Access::Write, Access::Fetch] {
            assert_eq!(cpu.translate(0x200100, access).unwrap(), 0x100100);
            let mut bytes = [0; 4];
            cpu.read_virtual(0x200100, access, &mut bytes).unwrap();
            assert_eq!(bytes, [0x34, 0x12, 0xed, 0xfe]);
        }
        let mut boundary = [0; 4];
        cpu.read_virtual(0x2ffffe, Access::Read, &mut boundary)
            .unwrap();
        assert_eq!(boundary, [1, 2, 3, 4]);
        assert!(cpu.translate(0x400000, Access::Read).is_err());
        assert!(
            cpu.read_virtual(0x400000, Access::Read, &mut boundary)
                .is_err()
        );
        unsafe {
            cpu.map_mmio(
                0x400000,
                0x4000,
                Mmio {
                    read: device_read,
                    write: device_write,
                    data: (&mut *probe as *mut MmioProbe).cast(),
                },
            )
            .unwrap();
        }
        cpu.coprocessor(cp(2), Some(0x400000)).unwrap();
        assert!(
            cpu.translate(0x500000, Access::Read)
                .unwrap_err()
                .contains("MMIO")
        );
        assert!(probe.accesses.is_empty());
    }
    #[test]
    fn virtual_reads_reuse_mapped_regions_until_mapping_changes() {
        let library = library();
        let cpu = library.open(Model::CortexR5F, 4 * 1024 * 1024).unwrap();
        cpu.map_zeroed(0x1000, 0x2000, 7).unwrap();
        cpu.write_memory(0x1000, &[0x34, 0x12, 0xed, 0xfe]).unwrap();

        let mut bytes = [0; 4];
        cpu.read_virtual(0x1000, Access::Read, &mut bytes).unwrap();
        cpu.read_virtual(0x1000, Access::Read, &mut bytes).unwrap();
        assert_eq!(bytes, [0x34, 0x12, 0xed, 0xfe]);
        assert_eq!(cpu.memory_region_queries.get(), 1);

        cpu.protect(0x1000, 0x1000, 1).unwrap();
        cpu.read_virtual(0x1000, Access::Read, &mut bytes).unwrap();
        assert_eq!(cpu.memory_region_queries.get(), 2);
        assert!(cpu.read_virtual(0x1000, Access::Write, &mut bytes).is_err());
        assert_eq!(cpu.memory_region_queries.get(), 2);

        cpu.unmap(0x1000, 0x1000).unwrap();
        assert!(cpu.read_virtual(0x1000, Access::Read, &mut bytes).is_err());
        assert_eq!(cpu.memory_region_queries.get(), 3);

        cpu.map_zeroed(0x1000, 0x1000, 3).unwrap();
        cpu.read_virtual(0x1000, Access::Read, &mut bytes).unwrap();
        assert_eq!(bytes, [0; 4]);
        assert_eq!(cpu.memory_region_queries.get(), 4);
    }
    #[derive(Default)]
    struct MmioProbe {
        accesses: Vec<(Access, u64, u32, u64)>,
    }
    unsafe fn device_read(offset: u64, bytes: u32, data: *mut c_void) -> u64 {
        let probe = unsafe { &mut *data.cast::<MmioProbe>() };
        probe.accesses.push((Access::Read, offset, bytes, 0));
        0x12345678
    }
    unsafe fn device_write(offset: u64, bytes: u32, value: u64, data: *mut c_void) {
        let probe = unsafe { &mut *data.cast::<MmioProbe>() };
        probe.accesses.push((Access::Write, offset, bytes, value));
    }
    #[test]
    fn typed_mmio_preserves_offsets_after_partial_unmap_and_refuses_host_reads() {
        let library = library();
        let mut probe = Box::<MmioProbe>::default();
        let cpu = library.open(Model::CortexR5F, 4 * 1024 * 1024).unwrap();
        cpu.map_zeroed(0x1000, 4096, 7).unwrap();
        let code = [0xe5910000u32, 0xe5810004] // LDR R0,[R1]; STR R0,[R1,#4]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        cpu.write_memory(0x1000, &code).unwrap();
        unsafe {
            cpu.map_mmio(
                0x8000,
                0x3000,
                Mmio {
                    read: device_read,
                    write: device_write,
                    data: (&mut *probe as *mut MmioProbe).cast(),
                },
            )
            .unwrap();
        }
        cpu.protect(0x9000, 0x1000, 1).unwrap();
        cpu.write_register(arm::CORE[1], &0x9020u32.to_le_bytes())
            .unwrap();
        let fault = cpu
            .run(Run {
                begin: 0x1000,
                until: Some(0x1008),
                watchdog_us: 0,
                instructions: Some(2),
            })
            .unwrap_err();
        assert!(matches!(
            fault,
            ExecutionError::Memory {
                fault: Fault::WriteProtected,
                ..
            }
        ));
        assert_eq!(probe.accesses, [(Access::Read, 0x1020, 4, 0)]);
        probe.accesses.clear();
        cpu.unmap(0x9000, 0x1000).unwrap();
        let mut bytes = [0; 4];
        assert!(cpu.read_memory(0xa020, &mut bytes).is_err());
        assert!(cpu.read_virtual(0xa020, Access::Read, &mut bytes).is_err());
        assert!(probe.accesses.is_empty());
        cpu.write_register(arm::CORE[1], &0xa020u32.to_le_bytes())
            .unwrap();
        cpu.run(Run {
            begin: 0x1000,
            until: Some(0x1008),
            watchdog_us: 0,
            instructions: Some(2),
        })
        .unwrap();
        assert_eq!(reg(&cpu, arm::CORE[0]), 0x12345678);
        assert_eq!(
            probe.accesses,
            [
                (Access::Read, 0x2020, 4, 0),
                (Access::Write, 0x2024, 4, 0x12345678)
            ]
        );
        cpu.unmap(0x8000, 0x1000).unwrap();
        cpu.unmap(0xa000, 0x1000).unwrap();
        unsafe {
            cpu.map_mmio(
                0x10000000,
                0x1000000,
                Mmio {
                    read: device_read,
                    write: device_write,
                    data: (&mut *probe as *mut MmioProbe).cast(),
                },
            )
            .unwrap();
        }
        let regions = cpu.memory_regions().unwrap();
        let window = regions
            .iter()
            .filter(|r| r.begin >= 0x10000000)
            .collect::<Vec<_>>();
        assert_eq!(window.len(), 1, "large MMIO windows must stay compact");
        assert_eq!(window[0].end, 0x10ffffff);
        cpu.unmap(0x10800000, 0x1000).unwrap();
        cpu.write_register(arm::CORE[1], &0x10801020u32.to_le_bytes())
            .unwrap();
        cpu.run(Run {
            begin: 0x1000,
            until: Some(0x1008),
            watchdog_us: 0,
            instructions: Some(2),
        })
        .unwrap();
        assert_eq!(
            &probe.accesses[2..],
            &[
                (Access::Read, 0x801020, 4, 0),
                (Access::Write, 0x801024, 4, 0x12345678)
            ]
        );
    }
}
