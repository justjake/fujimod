//! Large, zero-filled WebAssembly reservations without touching new pages.
//!
//! `memory.grow` guarantees zeroed bytes. sbrk reserves disjoint address space
//! from Emscripten's allocator. Existing growth slack is reserved and cleared;
//! pages added by this reservation remain untouched and demand-zero. Dropped reservations are reused and then
//! cleared in full. WebAssembly cannot shrink linear memory.
use std::cell::RefCell;
use std::ffi::c_void;

unsafe extern "C" {
    fn sbrk(increment: isize) -> *mut c_void;
}

thread_local! {
    static FREE: RefCell<Vec<(usize, usize)>> = const { RefCell::new(Vec::new()) };
}

pub fn zeros(size: usize) -> Option<*mut u8> {
    if let Some(pointer) = FREE.with(|free| {
        let mut free = free.borrow_mut();
        free.iter()
            .position(|&(_, bytes)| bytes == size)
            .map(|index| free.swap_remove(index).0 as *mut u8)
    }) {
        // The previous guest may have written anywhere in the reservation.
        unsafe { pointer.write_bytes(0, size) };
        return Some(pointer);
    }
    const ALIGN: usize = 65536;
    let old_memory_end = core::arch::wasm32::memory_size::<0>() * ALIGN;
    let old_break = unsafe { sbrk(0) } as usize;
    if old_break == usize::MAX {
        return None;
    }
    let aligned = old_break.checked_add(ALIGN - 1)? & !(ALIGN - 1);
    let increment = isize::try_from(aligned.checked_sub(old_break)?.checked_add(size)?).ok()?;
    let start = unsafe { sbrk(increment) } as usize;
    if start == usize::MAX {
        return None;
    }
    // Machine is single-threaded/unsendable. No allocator call can intervene
    // between querying and increasing the break; a mismatch cannot be owned.
    assert_eq!(start, old_break);
    // Existing linear memory may contain recycled bytes beyond the old break.
    // Only newly grown pages have WebAssembly's zero guarantee. Reclaim the
    // allocator's growth slack instead of abandoning it before every DRAM.
    let existing = old_memory_end.saturating_sub(aligned).min(size);
    unsafe { (aligned as *mut u8).write_bytes(0, existing) };
    Some(aligned as *mut u8)
}

pub fn release(pointer: *mut u8, size: usize) {
    FREE.with(|free| free.borrow_mut().push((pointer as usize, size)));
}
