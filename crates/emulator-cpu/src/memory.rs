//! Retained guest-memory backing, independent of the CPU execution backend.
use std::cell::UnsafeCell;
#[cfg(not(target_os = "emscripten"))]
use std::ffi::c_void;

pub mod pages;

unsafe extern "C" {
    #[cfg(not(target_os = "emscripten"))]
    fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> *mut c_void;
    #[cfg(not(target_os = "emscripten"))]
    fn munmap(addr: *mut c_void, len: usize) -> i32;
}
#[cfg(target_vendor = "apple")]
const MAP_ANON: i32 = 0x1000;
#[cfg(all(not(target_vendor = "apple"), not(target_os = "emscripten")))]
const MAP_ANON: i32 = 0x20;
const ZERO_MAP_THRESHOLD: usize = 1 << 20;

enum Storage {
    Boxed(Box<[UnsafeCell<u8>]>),
    // Anonymous zero-filled pages, faulted in on first touch. A freshly mapped
    // 512 MiB DRAM costs microseconds; a recycled calloc block is re-cleared.
    #[cfg(not(target_os = "emscripten"))]
    Mapped(*mut u8, usize),
    #[cfg(not(target_os = "emscripten"))]
    AlignedMap {
        allocation: *mut u8,
        allocation_size: usize,
        pointer: *mut u8,
        size: usize,
    },
    #[cfg(target_os = "emscripten")]
    Wasm(*mut u8, usize),
}

pub struct Backing {
    storage: Storage,
}
impl Backing {
    /// Sparse, zero-filled backing with explicit alignment for physical maps
    /// shared by different CPU backends. Allocation failure never falls back
    /// to an insufficiently aligned buffer.
    #[cfg(not(target_os = "emscripten"))]
    pub fn aligned_zeros(size: usize, alignment: usize) -> Result<Self, String> {
        if size == 0
            || !alignment.is_power_of_two()
            || alignment < 0x1000
            || !size.is_multiple_of(0x1000)
        {
            return Err("invalid aligned guest backing size/alignment".into());
        }
        let allocation_size = size
            .checked_add(alignment)
            .ok_or("aligned backing size overflow")?;
        let allocation = unsafe {
            mmap(
                std::ptr::null_mut(),
                allocation_size,
                3,
                2 | MAP_ANON,
                -1,
                0,
            )
        };
        if allocation as isize == -1 || allocation.is_null() {
            return Err("aligned guest backing allocation failed".into());
        }
        let pointer = ((allocation as usize + alignment - 1) & !(alignment - 1)) as *mut u8;
        Ok(Self {
            storage: Storage::AlignedMap {
                allocation: allocation.cast(),
                allocation_size,
                pointer,
                size,
            },
        })
    }
    #[cfg(target_os = "emscripten")]
    pub fn aligned_zeros(size: usize, alignment: usize) -> Result<Self, String> {
        if size == 0
            || !alignment.is_power_of_two()
            || !(0x1000..=0x10000).contains(&alignment)
            || !size.is_multiple_of(0x1000)
        {
            return Err("invalid aligned guest backing size/alignment".into());
        }
        let pointer =
            crate::wasm_backing::zeros(size).ok_or("aligned guest backing allocation failed")?;
        Ok(Self {
            storage: Storage::Wasm(pointer, size),
        })
    }
    pub fn from_bytes(bytes: Box<[u8]>) -> Self {
        // UnsafeCell<u8> is repr(transparent). Ownership/allocation layout and
        // slice metadata are unchanged. FFI guest writes then have explicit
        // interior mutability; no shared &[u8] aliases a writable guest buffer.
        let raw = Box::into_raw(bytes) as *mut [UnsafeCell<u8>];
        Self {
            storage: Storage::Boxed(unsafe { Box::from_raw(raw) }),
        }
    }
    pub fn zeros(size: usize) -> Self {
        #[cfg(target_os = "emscripten")]
        if size >= ZERO_MAP_THRESHOLD {
            if let Some(pointer) = crate::wasm_backing::zeros(size) {
                return Self {
                    storage: Storage::Wasm(pointer, size),
                };
            }
        }
        #[cfg(not(target_os = "emscripten"))]
        if size >= ZERO_MAP_THRESHOLD {
            let pointer = unsafe {
                mmap(
                    std::ptr::null_mut(),
                    size,
                    3,            // PROT_READ | PROT_WRITE
                    2 | MAP_ANON, // MAP_PRIVATE | MAP_ANON
                    -1,
                    0,
                )
            };
            if pointer as isize != -1 && !pointer.is_null() {
                return Self {
                    storage: Storage::Mapped(pointer.cast(), size),
                };
            }
        }
        Self::from_bytes(vec![0; size].into_boxed_slice())
    }
    pub fn as_ptr(&self) -> *mut u8 {
        match &self.storage {
            Storage::Boxed(cells) => cells.as_ptr().cast::<u8>().cast_mut(),
            #[cfg(not(target_os = "emscripten"))]
            Storage::Mapped(pointer, _) => *pointer,
            #[cfg(not(target_os = "emscripten"))]
            Storage::AlignedMap { pointer, .. } => *pointer,
            #[cfg(target_os = "emscripten")]
            Storage::Wasm(pointer, _) => *pointer,
        }
    }
    pub fn len(&self) -> usize {
        match &self.storage {
            Storage::Boxed(cells) => cells.len(),
            #[cfg(not(target_os = "emscripten"))]
            Storage::Mapped(_, size) => *size,
            #[cfg(not(target_os = "emscripten"))]
            Storage::AlignedMap { size, .. } => *size,
            #[cfg(target_os = "emscripten")]
            Storage::Wasm(_, size) => *size,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
impl Drop for Backing {
    fn drop(&mut self) {
        #[cfg(target_os = "emscripten")]
        if let Storage::Wasm(pointer, size) = self.storage {
            crate::wasm_backing::release(pointer, size);
        }
        #[cfg(not(target_os = "emscripten"))]
        if let Storage::Mapped(pointer, size) = self.storage {
            unsafe {
                munmap(pointer.cast(), size);
            }
        }
        #[cfg(not(target_os = "emscripten"))]
        if let Storage::AlignedMap {
            allocation,
            allocation_size,
            ..
        } = self.storage
        {
            unsafe {
                munmap(allocation.cast(), allocation_size);
            }
        }
    }
}
