#![cfg(all(
    feature = "jit",
    any(target_arch = "aarch64", target_arch = "x86_64"),
    any(target_os = "linux", target_os = "macos")
))]

mod ffi;
mod loops;
mod status;

use fujimod_emulator_cpu::{memory::Backing, *};
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::ptr::NonNull;

#[derive(Clone, Copy)]
enum Mapping {
    Backing {
        bytes: usize,
        permissions: u32,
        ptr: NonNull<u8>,
    },
    Mmio {
        bytes: usize,
        callbacks: Mmio,
    },
}

struct FaultHook {
    id: usize,
    callback: unsafe fn(Fault, u64, i32, i64, *mut c_void) -> bool,
    data: *mut c_void,
}

struct CoreShared {
    mappings: RefCell<BTreeMap<u64, Mapping>>,
    fault_hooks: RefCell<Vec<FaultHook>>,
    next_hook_id: Cell<usize>,
    observers: RefCell<Vec<(usize, Hook)>>,
    stopped: Cell<bool>,
    device_accessed: Cell<bool>,
    fault: RefCell<Option<ExecutionError>>,
    abort: Cell<Option<(u32, u32, Access)>>,
    inst: Cell<*mut ffi::DynarmicInstance>,
}

fn contains(range: HookRange, address: u64) -> bool {
    match range {
        HookRange::All => true,
        HookRange::Inclusive { begin, end } => (begin..=end).contains(&address),
    }
}

impl CoreShared {
    fn observe_memory(&self, access: Access, addr: u32, size: i32, value: i64) {
        let observers = self.observers.borrow().clone();
        for (_, hook) in observers {
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
                unsafe { callback(access, addr.into(), size, value, data) };
            }
        }
    }

    fn mapping(&self, address: u64, bytes: usize) -> Option<(Mapping, usize)> {
        let mappings = self.mappings.borrow();
        let (&base, &mapping) = mappings.range(..=address).next_back()?;
        let size = match mapping {
            Mapping::Backing { bytes, .. } | Mapping::Mmio { bytes, .. } => bytes,
        };
        let offset = usize::try_from(address - base).ok()?;
        (offset.checked_add(bytes)? <= size).then_some((mapping, offset))
    }

    fn translate(&self, address: u32, access: Access) -> Result<u64, String> {
        self.walk(address, access)?
            .map_err(|status| format!("ARMv7 page fault at {address:#010x} (status {status:#04x})"))
    }

    fn walk(&self, address: u32, access: Access) -> Result<Result<u64, u32>, String> {
        let inst = self.inst.get();
        let cp15 = |crn, crm, opc2| Ok(unsafe { ffi::dynarmic_get_cp15(inst, 0, crn, crm, opc2) });
        if cp15(1, 0, 0)? & 1 == 0 {
            return Ok(Ok(address.into()));
        }
        let cpsr = unsafe { ffi::dynarmic_get_cpsr(inst) };
        let mmu = armv7a::mmu::Mmu::read(cp15, cpsr)?;
        mmu.translate(address, access, mmu.user, |physical| {
            let Some((Mapping::Backing { ptr, .. }, offset)) = self.mapping(physical.into(), 4)
            else {
                return Err(format!(
                    "Dynarmic page table outside RAM at {physical:#010x}"
                ));
            };
            let bytes = unsafe { ptr.as_ptr().add(offset).cast::<[u8; 4]>().read() };
            Ok(u32::from_le_bytes(bytes))
        })
    }

    fn fail(&self, fault: Fault, message: String) {
        let mut pending = self.fault.borrow_mut();
        if pending.is_none() {
            *pending = Some(ExecutionError::Memory { fault, message });
        }
        unsafe { ffi::dynarmic_abort_access(self.inst.get()) };
    }

    fn walk_for_guest(&self, address: u32, access: Access, protected: Fault) -> Option<u64> {
        match self.walk(address, access) {
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
        let mut pointers = [std::ptr::null_mut(); 2];
        let needed = if output.is_some() { 2 } else { 1 };
        for (index, &(virtual_address, count)) in pieces.iter().enumerate() {
            let Some(physical) = self.walk_for_guest(virtual_address, access, protected) else {
                return 0;
            };
            let Some((Mapping::Backing { ptr, permissions, .. }, offset)) = self.mapping(physical, count)
                .filter(|(mapping, _)| matches!(mapping, Mapping::Backing { permissions, .. } if permissions & needed != 0))
            else {
                self.fail(protected, format!("Dynarmic cross-page {access:?} requires RAM at {virtual_address:#010x}"));
                return 0;
            };
            debug_assert!(permissions & needed != 0);
            pointers[index] = unsafe { ptr.as_ptr().add(offset) };
        }
        let mut bytes = output.unwrap_or(0).to_le_bytes();
        let (first, second) = bytes.split_at_mut(split);
        for ((&(virtual_address, count), pointer), piece) in
            pieces.iter().zip(pointers).zip([first, second])
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
            } else {
                unsafe { std::ptr::copy_nonoverlapping(pointer, piece.as_mut_ptr(), count) };
            }
        }
        u64::from_le_bytes(bytes)
    }

    fn access(&self, address: u32, size: usize, output: Option<u64>) -> u64 {
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
        let Some(physical) = self.walk_for_guest(address, access, protected) else {
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
            (Mapping::Backing { ptr, .. }, Some(value)) => {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        value.to_le_bytes().as_ptr(),
                        ptr.as_ptr().add(offset),
                        size,
                    )
                };
                0
            }
            (Mapping::Mmio { callbacks, .. }, value) => {
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
    memory_regions: RefCell<Vec<MemoryRegion>>,
    last_progress: Cell<u64>,
    scheduler_hook: Cell<Option<HookId>>,
    loops: loops::Loops,
    owned: RefCell<BTreeMap<u64, Backing>>,
    banks: RefCell<status::Banks>,
    fpexc: Cell<u32>,
}

impl Core {
    pub fn new(model: Model) -> Result<Self, String> {
        if model != Model::CortexA7 {
            return Err("Dynarmic requires a Cortex-A7 processor".into());
        }
        let shared = Box::new(CoreShared {
            mappings: RefCell::new(BTreeMap::new()),
            fault_hooks: RefCell::new(Vec::new()),
            next_hook_id: Cell::new(0),
            observers: RefCell::new(Vec::new()),
            stopped: Cell::new(false),
            device_accessed: Cell::new(false),
            fault: RefCell::new(None),
            abort: Cell::new(None),
            inst: Cell::new(std::ptr::null_mut()),
        });
        let user_data = (&*shared as *const CoreShared as *mut CoreShared).cast();

        let cb = ffi::DynarmicCallbacks {
            user_data,
            read_code: Self::read_code,
            read: Self::read,
            write: Self::write,
        };

        let inst = unsafe { ffi::dynarmic_create(&cb) };
        if inst.is_null() {
            return Err("failed to create Dynarmic JIT instance".into());
        }
        shared.inst.set(inst);
        unsafe { ffi::dynarmic_set_cpsr(inst, 0xd3) };

        Ok(Self {
            inst,
            identity: Identity::allocate(),
            model,
            shared,
            memory_regions: RefCell::new(Vec::new()),
            last_progress: Cell::new(0),
            scheduler_hook: Cell::new(None),
            loops: loops::Loops::default(),
            owned: RefCell::new(BTreeMap::new()),
            banks: RefCell::new(status::Banks::default()),
            fpexc: Cell::new(0),
        })
    }

    pub fn set_cache_ids(&self, ids: [u32; 3]) {
        unsafe {
            ffi::dynarmic_set_cache_ids(self.inst, ids[0], ids[1], ids[2]);
        }
    }

    unsafe extern "C" fn read_code(user_data: *mut c_void, address: u32, output: *mut u32) -> bool {
        let shared = unsafe { &*user_data.cast::<CoreShared>() };
        let Ok(physical) = shared.translate(address, Access::Fetch) else {
            return false;
        };
        let Some((
            Mapping::Backing {
                ptr, permissions, ..
            },
            offset,
        )) = shared.mapping(physical, 4)
        else {
            return false;
        };
        if permissions & 4 == 0 {
            return false;
        }
        let bytes = unsafe { ptr.as_ptr().add(offset).cast::<[u8; 4]>().read() };
        unsafe { *output = u32::from_le_bytes(bytes) };
        true
    }

    unsafe extern "C" fn read(user_data: *mut c_void, address: u32, size: u32) -> u64 {
        let shared = unsafe { &*user_data.cast::<CoreShared>() };
        shared.access(address, size as usize, None)
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

pub struct SnapshotData {
    owner: Identity,
    regs: [u32; 16],
    cpsr: u32,
    fpscr: u32,
    banks: status::Banks,
    fpexc: u32,
}

impl Snapshot for SnapshotData {
    fn owner(&self) -> Identity {
        self.owner
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Cpu for Core {
    fn identity(&self) -> Identity {
        self.identity
    }

    fn model(&self) -> Model {
        self.model
    }

    fn set_armv7a_owner_exclusives(&self, on: bool) -> Result<(), String> {
        if on {
            Ok(())
        } else {
            Err("Dynarmic requires exact execution policy".into())
        }
    }

    fn features(&self) -> Features {
        Features {
            instruction_limit: true,
            instruction_progress: true,
            ..Default::default()
        }
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
        if register.cp != 15 || register.wide {
            return Err("Dynarmic only supports 32-bit CP15 accesses".into());
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
        let mappings = self.shared.mappings.borrow();
        if let Some((&base, mapping)) = mappings.range(..=address).next_back() {
            match mapping {
                Mapping::Backing {
                    bytes: m_bytes,
                    ptr,
                    ..
                } => {
                    let offset = (address - base) as usize;
                    if offset + bytes.len() <= *m_bytes {
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                ptr.as_ptr().add(offset),
                                bytes.as_mut_ptr(),
                                bytes.len(),
                            );
                        }
                        return Ok(());
                    }
                }
                Mapping::Mmio {
                    bytes: m_bytes,
                    callbacks,
                } => {
                    let offset = address - base;
                    if offset + (bytes.len() as u64) <= *m_bytes as u64 {
                        for (i, b) in bytes.iter_mut().enumerate() {
                            *b = unsafe {
                                (callbacks.read)(offset + (i as u64), 1, callbacks.data) as u8
                            };
                        }
                        return Ok(());
                    }
                }
            }
        }
        Err(format!("unmapped read at {address:#x}"))
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
            let translated = self.translate(address, access)?;
            let room = 4096 - (address as usize & 4095);
            let (chunk, remaining) = bytes.split_at_mut(room.min(bytes.len()));
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
        let mappings = self.shared.mappings.borrow();
        if let Some((&base, mapping)) = mappings.range(..=address).next_back() {
            match mapping {
                Mapping::Backing {
                    bytes: m_bytes,
                    ptr,
                    ..
                } => {
                    let offset = (address - base) as usize;
                    if offset + bytes.len() <= *m_bytes {
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                bytes.as_ptr(),
                                ptr.as_ptr().add(offset),
                                bytes.len(),
                            );
                            ffi::dynarmic_invalidate_cache(self.inst, address as u32, bytes.len());
                        }
                        return Ok(());
                    }
                }
                Mapping::Mmio {
                    bytes: m_bytes,
                    callbacks,
                } => {
                    let offset = address - base;
                    if offset + (bytes.len() as u64) <= *m_bytes as u64 {
                        for (i, &b) in bytes.iter().enumerate() {
                            unsafe {
                                (callbacks.write)(
                                    offset + (i as u64),
                                    1,
                                    u64::from(b),
                                    callbacks.data,
                                )
                            };
                        }
                        return Ok(());
                    }
                }
            }
        }
        Err(format!("unmapped write at {address:#x}"))
    }

    fn memory_regions(&self) -> Result<Vec<MemoryRegion>, String> {
        Ok(self.memory_regions.borrow().clone())
    }

    unsafe fn map_backing(
        &self,
        address: u64,
        bytes: usize,
        permissions: u32,
        backing: NonNull<u8>,
    ) -> Result<(), String> {
        let end = address
            .checked_add(bytes as u64)
            .ok_or("Dynarmic mapping overflows")?;
        if bytes == 0
            || end > 1 << 32
            || !address.is_multiple_of(4096)
            || !bytes.is_multiple_of(4096)
        {
            return Err("Dynarmic requires a nonempty page-aligned 32-bit mapping".into());
        }
        self.shared.mappings.borrow_mut().insert(
            address,
            Mapping::Backing {
                bytes,
                permissions,
                ptr: backing,
            },
        );

        self.memory_regions.borrow_mut().push(MemoryRegion {
            begin: address,
            end: address + (bytes as u64) - 1,
            permissions,
        });

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
        self.shared
            .mappings
            .borrow_mut()
            .insert(address, Mapping::Mmio { bytes, callbacks });
        self.memory_regions.borrow_mut().push(MemoryRegion {
            begin: address,
            end: address + (bytes as u64) - 1,
            permissions: 3,
        });
        Ok(())
    }

    fn unmap(&self, address: u64, bytes: usize) -> Result<(), String> {
        if !self.memory_regions.borrow().iter().any(|r| {
            r.begin == address
                && r.end
                    .checked_sub(address)
                    .and_then(|delta| delta.checked_add(1))
                    == Some(bytes as u64)
        }) {
            return Err("Dynarmic can only unmap an entire mapped range".into());
        }
        self.shared.mappings.borrow_mut().remove(&address);
        self.owned.borrow_mut().remove(&address);
        self.invalidate_code(Some((address, address + bytes as u64)))?;
        self.memory_regions
            .borrow_mut()
            .retain(|r| r.begin != address);
        Ok(())
    }

    fn protect(&self, _address: u64, _bytes: usize, _permissions: u32) -> Result<(), String> {
        Err("Dynarmic cannot change mapping protection".into())
    }

    unsafe fn backing_written(
        &self,
        physical: u64,
        _backing: NonNull<u8>,
        bytes: usize,
    ) -> Result<(), String> {
        unsafe { ffi::dynarmic_invalidate_cache(self.inst, physical as u32, bytes) };
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
        let budget = request.instructions.unwrap_or(usize::MAX) as u64;
        self.shared.device_accessed.set(false);
        let status = loop {
            if self.last_progress.get() >= budget {
                break ffi::DynarmicExitReason::Timeout;
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
            if let Err(status) = self.shared.walk(pc, Access::Fetch)? {
                armv7a::abort::enter_reported(self, pc, status, Access::Fetch)?;
                return Ok(Exit::Stopped);
            }
            let mut instruction = [0; 4];
            self.read_virtual(pc.into(), Access::Fetch, &mut instruction)
                .map_err(|message| ExecutionError::Memory {
                    fault: Fault::FetchUnmapped,
                    message,
                })?;
            self.shared.stopped.set(false);
            let observers = self.shared.observers.borrow().clone();
            for (_, hook) in observers {
                if let Hook::Instruction {
                    callback,
                    data,
                    range,
                } = hook
                    && contains(range, pc.into())
                {
                    unsafe { callback(pc.into(), 4, data) };
                    if self.shared.stopped.get() {
                        break;
                    }
                }
            }
            if self.shared.stopped.get() {
                break ffi::DynarmicExitReason::Stopped;
            }
            let word = u32::from_le_bytes(instruction);
            if stable
                && request.until.is_none()
                && self.advance_loop(pc, cpsr, word, budget - self.last_progress.get())?
            {
                continue;
            }
            if let Some(next) = self
                .status_instruction(pc, word)
                .map_err(|error| format!("{error} (instruction {word:#010x} at {pc:#010x})"))?
            {
                unsafe { ffi::dynarmic_set_reg(self.inst, 15, next) };
                self.last_progress.set(self.last_progress.get() + 1);
                continue;
            }
            if armv7a::is_vfp_instruction(word)
                && (word >> 28 == 15 || arm::condition_passes(word >> 28, cpsr))
                && !self.vfp_accessible()
            {
                return Err(ExecutionError::InvalidInstruction(format!(
                    "VFP or Advanced SIMD disabled at {pc:#010x}"
                )));
            }
            let mut executed = 0;
            self.shared.fault.replace(None);
            self.shared.abort.set(None);
            let status = unsafe { ffi::dynarmic_step(self.inst, &mut executed) };
            if let Some((address, status, access)) = self.shared.abort.take() {
                armv7a::abort::enter_reported(self, address, status, access)
                    .map_err(|error| format!("ARMv7 abort at {pc:#010x}, {access:?} {address:#010x}, FSR {status:#04x}: {error}"))?;
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
            ffi::DynarmicExitReason::Error => {
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

    fn instruction_progress(&self) -> Option<u64> {
        Some(self.last_progress.get())
    }

    fn stop(&self) -> Result<(), String> {
        self.shared.stopped.set(true);
        unsafe { ffi::dynarmic_stop(self.inst) };
        Ok(())
    }

    fn snapshot(&self, _previous: Option<&dyn Snapshot>) -> Result<Box<dyn Snapshot>, String> {
        let regs = std::array::from_fn(|i| unsafe { ffi::dynarmic_get_reg(self.inst, i as u32) });
        let cpsr = unsafe { ffi::dynarmic_get_cpsr(self.inst) };
        let fpscr = unsafe { ffi::dynarmic_get_fpscr(self.inst) };
        Ok(Box::new(SnapshotData {
            owner: self.identity,
            regs,
            cpsr,
            fpscr,
            banks: self.banks.borrow().clone(),
            fpexc: self.fpexc.get(),
        }))
    }

    fn restore(&self, snapshot: &dyn Snapshot) -> Result<(), String> {
        if snapshot.owner() != self.identity {
            return Err("Dynarmic CPU snapshot belongs to another processor".into());
        }
        let state = snapshot
            .as_any()
            .downcast_ref::<SnapshotData>()
            .ok_or("invalid snapshot type")?;
        *self.banks.borrow_mut() = state.banks.clone();
        self.fpexc.set(state.fpexc);
        for (i, &val) in state.regs.iter().enumerate() {
            unsafe { ffi::dynarmic_set_reg(self.inst, i as u32, val) };
        }
        unsafe {
            ffi::dynarmic_set_cpsr(self.inst, state.cpsr);
            ffi::dynarmic_set_fpscr(self.inst, state.fpscr);
        }
        Ok(())
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

    unsafe fn observe_backings(&self, _ranges: &[BackingObserver]) -> Result<(), String> {
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
        Ok(BTreeMap::from([(
            "loop_accelerated_instructions",
            self.loops.skipped.get(),
        )]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
