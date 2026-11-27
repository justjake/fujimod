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
}

#[repr(C)]
pub struct DynarmicCallbacks {
    pub user_data: *mut c_void,
    pub observe_memory: unsafe extern "C" fn(*mut c_void, bool, u32, i32, i64),
    pub read_8: unsafe extern "C" fn(*mut c_void, u32) -> u8,
    pub read_16: unsafe extern "C" fn(*mut c_void, u32) -> u16,
    pub read_32: unsafe extern "C" fn(*mut c_void, u32) -> u32,
    pub read_64: unsafe extern "C" fn(*mut c_void, u32) -> u64,
    pub write_8: unsafe extern "C" fn(*mut c_void, u32, u8),
    pub write_16: unsafe extern "C" fn(*mut c_void, u32, u16),
    pub write_32: unsafe extern "C" fn(*mut c_void, u32, u32),
    pub write_64: unsafe extern "C" fn(*mut c_void, u32, u64),
}

pub enum DynarmicInstance {}

unsafe extern "C" {
    pub fn dynarmic_create(callbacks: *const DynarmicCallbacks) -> *mut DynarmicInstance;
    pub fn dynarmic_destroy(inst: *mut DynarmicInstance);
    pub fn dynarmic_step(inst: *mut DynarmicInstance, executed: *mut u64) -> DynarmicExitReason;
    pub fn dynarmic_stop(inst: *mut DynarmicInstance);
    pub fn dynarmic_clear_cache(inst: *mut DynarmicInstance);
    pub fn dynarmic_invalidate_cache(inst: *mut DynarmicInstance, vaddr: u32, length: usize);
    pub fn dynarmic_get_reg(inst: *mut DynarmicInstance, reg: u32) -> u32;
    pub fn dynarmic_set_reg(inst: *mut DynarmicInstance, reg: u32, val: u32);
    pub fn dynarmic_get_cpsr(inst: *mut DynarmicInstance) -> u32;
    pub fn dynarmic_set_cpsr(inst: *mut DynarmicInstance, val: u32);
    pub fn dynarmic_get_fpscr(inst: *mut DynarmicInstance) -> u32;
    pub fn dynarmic_set_fpscr(inst: *mut DynarmicInstance, val: u32);
    pub fn dynarmic_map_pages(
        inst: *mut DynarmicInstance,
        start_page: u32,
        num_pages: u32,
        host_ptr: *mut u8,
    );
    pub fn dynarmic_unmap_pages(inst: *mut DynarmicInstance, start_page: u32, num_pages: u32);
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
