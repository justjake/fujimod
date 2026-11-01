//! Host virtual-memory ownership for zero-filled guest backing.
use std::ffi::c_void;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    #[link_name = "VirtualAlloc"]
    fn virtual_alloc(address: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    #[link_name = "VirtualFree"]
    fn virtual_free(address: *mut c_void, size: usize, kind: u32) -> i32;
}

#[cfg(not(windows))]
unsafe extern "C" {
    fn mmap(
        address: *mut c_void,
        size: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, size: usize) -> i32;
}

#[cfg(all(not(windows), target_vendor = "apple"))]
const MAP_ANON: i32 = 0x1000;
#[cfg(all(not(windows), not(target_vendor = "apple")))]
const MAP_ANON: i32 = 0x20;

#[cfg(windows)]
pub(super) fn allocate(size: usize) -> *mut u8 {
    // MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE. Physical pages remain demand-zero.
    unsafe { virtual_alloc(std::ptr::null_mut(), size, 0x3000, 0x04).cast() }
}

#[cfg(not(windows))]
pub(super) fn allocate(size: usize) -> *mut u8 {
    let pointer = unsafe { mmap(std::ptr::null_mut(), size, 3, 2 | MAP_ANON, -1, 0) };
    if pointer as isize == -1 {
        std::ptr::null_mut()
    } else {
        pointer.cast()
    }
}

/// Release the original allocation base, not an aligned interior pointer.
#[cfg(windows)]
pub(super) unsafe fn release(pointer: *mut u8, _size: usize) {
    // MEM_RELEASE requires the reservation base and a zero size.
    unsafe { virtual_free(pointer.cast(), 0, 0x8000) };
}

#[cfg(not(windows))]
pub(super) unsafe fn release(pointer: *mut u8, size: usize) {
    unsafe { munmap(pointer.cast(), size) };
}
