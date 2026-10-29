//! Private SDK ABI. Values follow Hypervisor.framework's macOS ARM64 headers.
use std::ffi::{c_int, c_void};
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Exception {
    pub syndrome: u64,
    pub virtual_address: u64,
    pub physical_address: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Exit {
    pub reason: u32,
    pub exception: Exception,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MachTimebaseInfo {
    pub numer: u32,
    pub denom: u32,
}
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub struct Kevent {
    pub ident: u64,
    pub filter: i16,
    pub flags: u16,
    pub fflags: u32,
    pub data: i64,
    pub udata: *mut c_void,
}
impl Default for Kevent {
    fn default() -> Self {
        Self {
            ident: 0,
            filter: 0,
            flags: 0,
            fflags: 0,
            data: 0,
            udata: std::ptr::null_mut(),
        }
    }
}
pub const PC: u32 = 31;
pub const FPCR: u32 = 32;
pub const FPSR: u32 = 33;
pub const PSTATE: u32 = 34;
pub const MDSCR: u16 = 0x8012;
pub const SCTLR: u16 = 0xc080;
pub const TCR: u16 = 0xc102;
pub const SP0: u16 = 0xc208;
pub const SP1: u16 = 0xe208;
pub const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
pub const EVFILT_TIMER: i16 = -7;
pub const EVFILT_USER: i16 = -10;
pub const EV_ADD: u16 = 0x0001;
pub const EV_DELETE: u16 = 0x0002;
pub const EV_ENABLE: u16 = 0x0004;
pub const EV_ONESHOT: u16 = 0x0010;
pub const EV_CLEAR: u16 = 0x0020;
pub const NOTE_TRIGGER: u32 = 0x0100_0000;
pub const NOTE_ABSOLUTE: u32 = 0x0000_0008;
pub const NOTE_MACHTIME: u32 = 0x0000_0100;
#[link(name = "System")]
unsafe extern "C" {
    pub fn mach_absolute_time() -> u64;
    pub fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
    pub fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
    pub fn kqueue() -> c_int;
    pub fn kevent(
        queue: c_int,
        changes: *const Kevent,
        change_count: c_int,
        events: *mut Kevent,
        event_count: c_int,
        timeout: *const c_void,
    ) -> c_int;
    pub fn close(fd: c_int) -> c_int;
}
#[link(name = "Hypervisor", kind = "framework")]
unsafe extern "C" {
    pub fn hv_vm_create(config: *const c_void) -> u32;
    pub fn hv_vm_destroy() -> u32;
    pub fn hv_vm_map(address: *mut c_void, ipa: u64, size: usize, flags: u64) -> u32;
    pub fn hv_vm_unmap(ipa: u64, size: usize) -> u32;
    pub fn hv_vm_protect(ipa: u64, size: usize, flags: u64) -> u32;
    pub fn hv_vcpu_create(cpu: *mut u64, exit: *mut *const Exit, config: *const c_void) -> u32;
    pub fn hv_vcpu_destroy(cpu: u64) -> u32;
    pub fn hv_vcpu_run(cpu: u64) -> u32;
    pub fn hv_vcpus_exit(cpus: *const u64, count: u32) -> u32;
    pub fn hv_vcpu_get_reg(cpu: u64, reg: u32, out: *mut u64) -> u32;
    pub fn hv_vcpu_set_reg(cpu: u64, reg: u32, value: u64) -> u32;
    pub fn hv_vcpu_get_sys_reg(cpu: u64, reg: u16, out: *mut u64) -> u32;
    pub fn hv_vcpu_set_sys_reg(cpu: u64, reg: u16, value: u64) -> u32;
    pub fn camera_hvf_get_simd(cpu: u64, reg: u32, bytes: *mut u8) -> u32;
    pub fn camera_hvf_set_simd(cpu: u64, reg: u32, bytes: *const u8) -> u32;
    pub fn sys_icache_invalidate(address: *mut c_void, size: usize);
    pub fn hv_vcpu_set_trap_debug_exceptions(cpu: u64, value: bool) -> u32;
    pub fn hv_vcpu_set_trap_debug_reg_accesses(cpu: u64, value: bool) -> u32;
    pub fn hv_vcpu_set_vtimer_mask(cpu: u64, value: bool) -> u32;
}
pub fn check(status: u32, operation: &str) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!("HVF {operation}: {status:#010x}"))
    }
}
