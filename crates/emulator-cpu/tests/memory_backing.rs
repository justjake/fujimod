#![cfg(not(target_os = "emscripten"))]

use fujimod_emulator_cpu::memory::Backing;
use std::sync::Mutex;

static MEMORY_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn large_zero_backing_remains_writable_at_page_edges() {
    let _lock = MEMORY_LOCK.lock().unwrap();
    let size = (1 << 20) + 4096;
    let backing = Backing::zeros(size);
    assert_eq!(backing.len(), size);
    let bytes = unsafe { std::slice::from_raw_parts_mut(backing.as_ptr(), size) };
    assert!(bytes.iter().all(|&byte| byte == 0));
    for (index, value) in [(0, 1), (4095, 2), (4096, 3), (size - 1, 4)] {
        bytes[index] = value;
    }
    assert_eq!(
        (bytes[0], bytes[4095], bytes[4096], bytes[size - 1]),
        (1, 2, 3, 4)
    );
}

#[test]
fn aligned_backing_supports_page_and_large_alignments() {
    let _lock = MEMORY_LOCK.lock().unwrap();
    let size = 3 * 4096;
    for alignment in [4096, 16384, 65536, 2 << 20] {
        let backing = Backing::aligned_zeros(size, alignment).unwrap();
        assert_eq!(backing.len(), size);
        assert_eq!(backing.as_ptr() as usize % alignment, 0);
        let bytes = unsafe { std::slice::from_raw_parts_mut(backing.as_ptr(), size) };
        assert!(bytes.iter().all(|&byte| byte == 0));
        bytes[0] = 0x5a;
        bytes[size - 1] = 0xa5;
        assert_eq!((bytes[0], bytes[size - 1]), (0x5a, 0xa5));
    }
}

#[test]
fn invalid_aligned_backing_inputs_are_rejected() {
    let _lock = MEMORY_LOCK.lock().unwrap();
    for (size, alignment) in [(0, 4096), (1, 4096), (4096, 0), (4096, 2048), (4096, 6144)] {
        assert!(Backing::aligned_zeros(size, alignment).is_err());
    }
    assert!(Backing::aligned_zeros(usize::MAX & !4095, 4096).is_err());
}

#[cfg(windows)]
#[test]
fn windows_drop_releases_original_virtual_allocation() {
    use std::ffi::c_void;

    #[repr(C)]
    struct MemoryInfo {
        base: *mut c_void,
        allocation_base: *mut c_void,
        allocation_protect: u32,
        #[cfg(target_pointer_width = "64")]
        partition_id: u16,
        region_size: usize,
        state: u32,
        protect: u32,
        kind: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        #[link_name = "VirtualQuery"]
        fn virtual_query(address: *const c_void, info: *mut MemoryInfo, length: usize) -> usize;
    }
    fn query(address: *const c_void) -> MemoryInfo {
        let mut info = unsafe { std::mem::zeroed::<MemoryInfo>() };
        let length = std::mem::size_of::<MemoryInfo>();
        assert_eq!(unsafe { virtual_query(address, &mut info, length) }, length);
        info
    }

    let _lock = MEMORY_LOCK.lock().unwrap();
    let backing = Backing::aligned_zeros(8192, 2 << 20).unwrap();
    let info = query(backing.as_ptr().cast());
    assert_eq!(info.state, 0x1000); // MEM_COMMIT
    assert_eq!(info.kind, 0x20000); // MEM_PRIVATE
    let base = info.allocation_base;
    drop(backing);
    assert_eq!(query(base).state, 0x10000); // MEM_FREE
}
