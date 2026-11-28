use std::ffi::c_void;

#[repr(C)]
#[allow(dead_code)] // Constructed and returned by the C++ shim.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DynarmicExitReason {
    Timeout = 0,
    Stopped = 1,
    Wfi = 2,
    Wfe = 3,
    Sev = 4,
    Svc = 5,
    Undefined = 6,
    NoExecute = 7,
    Breakpoint = 8,
    Error = 9,
    Replay = 10,
}

// Layout shared with Dynarmic's ARM64 TimedRead32Cache.
#[repr(C)]
pub struct DynarmicDirectRead {
    pub remaining_floor: u64,
    pub value: u64,
    pub count: u64,
    pub first_descriptor: *const u8,
    pub second_descriptor: *const u8,
    pub expected: u64,
    pub denominator: u64,
    pub numerator: u64,
    pub scaled_origin: u64,
    pub address: u32,
}

impl DynarmicDirectRead {
    pub const DISABLED: Self = Self {
        remaining_floor: u64::MAX,
        value: 0,
        count: 0,
        first_descriptor: std::ptr::null(),
        second_descriptor: std::ptr::null(),
        expected: 0,
        denominator: 0,
        numerator: 0,
        scaled_origin: 0,
        address: u32::MAX,
    };
}

#[repr(C)]
pub struct DynarmicCallbacks {
    pub user_data: *mut c_void,
    pub inline_read32: *mut DynarmicDirectRead,
    pub batch_instruction: extern "C" fn(u32) -> u32,
    pub guard_transfer: unsafe extern "C" fn(*mut c_void, u32, u32),
    pub read_code: unsafe extern "C" fn(*mut c_void, u32, *mut u32) -> bool,
    pub read: unsafe extern "C" fn(*mut c_void, u32, u32) -> u64,
    pub timed_read32: unsafe extern "C" fn(*mut c_void, u32, u64) -> u32,
    pub write: unsafe extern "C" fn(*mut c_void, u32, u32, u64),
}

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct DynarmicMemoryControl {
    pub sctlr: u32,
    pub ttbr0: u32,
    pub ttbr1: u32,
    pub ttbcr: u32,
    pub dacr: u32,
    pub fcseidr: u32,
    pub cpsr: u32,
    pub cpacr: u32,
}

pub enum DynarmicInstance {}

unsafe extern "C" {
    pub fn dynarmic_create(
        callbacks: *const DynarmicCallbacks,
        rprofile: bool,
    ) -> *mut DynarmicInstance;
    pub fn dynarmic_destroy(inst: *mut DynarmicInstance);
    pub fn dynarmic_set_translation_context(
        inst: *mut DynarmicInstance,
        value: u32,
        vfp_enabled: bool,
    );
    pub fn dynarmic_observe_stores(inst: *mut DynarmicInstance, enabled: bool);
    pub fn dynarmic_read_gprs(inst: *mut DynarmicInstance, values: *mut u32);
    pub fn dynarmic_read_snapshot(
        inst: *mut DynarmicInstance,
        ext_regs: *mut u32,
        cp15: *mut u32,
        cache_ids: *mut u32,
    );
    pub fn dynarmic_write_snapshot(
        inst: *mut DynarmicInstance,
        ext_regs: *const u32,
        cp15: *const u32,
        cache_ids: *const u32,
    );
    pub fn dynarmic_data_pages(inst: *mut DynarmicInstance) -> *mut *mut u8;
    pub fn dynarmic_run(
        inst: *mut DynarmicInstance,
        budget: u64,
        executed: *mut u64,
    ) -> DynarmicExitReason;
    pub fn dynarmic_step(inst: *mut DynarmicInstance, executed: *mut u64) -> DynarmicExitReason;
    pub fn dynarmic_abort_access(inst: *mut DynarmicInstance);
    pub fn dynarmic_memory_control(
        inst: *mut DynarmicInstance,
        control: *mut DynarmicMemoryControl,
    );
    pub fn dynarmic_stop(inst: *mut DynarmicInstance);
    pub fn dynarmic_clear_cache(inst: *mut DynarmicInstance);
    pub fn dynarmic_invalidate_cache(inst: *mut DynarmicInstance, vaddr: u32, length: usize);
    pub fn dynarmic_get_reg(inst: *mut DynarmicInstance, reg: u32) -> u32;
    pub fn dynarmic_set_reg(inst: *mut DynarmicInstance, reg: u32, val: u32);
    pub fn dynarmic_get_cpsr(inst: *mut DynarmicInstance) -> u32;
    pub fn dynarmic_set_cpsr(inst: *mut DynarmicInstance, val: u32);
    pub fn dynarmic_get_fpscr(inst: *mut DynarmicInstance) -> u32;
    pub fn dynarmic_set_fpscr(inst: *mut DynarmicInstance, val: u32);
    pub fn dynarmic_set_cp15(
        inst: *mut DynarmicInstance,
        opc1: u32,
        crn: u32,
        crm: u32,
        opc2: u32,
        val: u32,
    );
    pub fn dynarmic_get_cp15(
        inst: *mut DynarmicInstance,
        opc1: u32,
        crn: u32,
        crm: u32,
        opc2: u32,
    ) -> u32;
    pub fn dynarmic_set_cache_ids(inst: *mut DynarmicInstance, l1d: u32, l1i: u32, l2: u32);
}
