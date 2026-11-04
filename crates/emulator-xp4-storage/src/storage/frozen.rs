//! Retained, aligned immutable source for a NOR execution prefix.
use std::{alloc::{alloc, dealloc, Layout}, cell::UnsafeCell, ptr::NonNull, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImmutablePrefix(usize);
impl ImmutablePrefix {
    pub fn new(bytes: usize) -> Result<Self, String> {
        if bytes == 0 || !bytes.is_multiple_of(4096) || bytes as u64 > super::FLASH_SIZE {
            return Err("Immutable NOR prefix must be positive and page aligned".into());
        }
        Ok(Self(bytes))
    }
    pub fn bytes(self) -> usize { self.0 }
}
struct Allocation { pointer: NonNull<UnsafeCell<u8>>, layout: Layout, len: usize }
// No mutable capability escapes; initialization completes before publication.
unsafe impl Send for Allocation {}
unsafe impl Sync for Allocation {}
impl Drop for Allocation {
    fn drop(&mut self) { unsafe { dealloc(self.pointer.as_ptr().cast(), self.layout) }; }
}
#[derive(Clone)]
pub(super) struct Source(Arc<Allocation>);
impl Source {
    pub fn new(bytes: Vec<u8>) -> Result<Self, String> {
        let layout = Layout::from_size_align(bytes.len().max(1), 65536).map_err(|e| e.to_string())?;
        let pointer = NonNull::new(unsafe { alloc(layout) }).ok_or("NOR allocation failed")?;
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.as_ptr(), bytes.len()) };
        Ok(Self(Arc::new(Allocation { pointer: pointer.cast(), layout, len: bytes.len() })))
    }
    pub fn len(&self) -> usize { self.0.len }
    pub fn as_slice(&self) -> &[u8] {
        // Source is never written after initialization, including by mapped CPUs.
        unsafe { std::slice::from_raw_parts(self.0.pointer.as_ptr().cast(), self.len()) }
    }
}
/// Keeping this capability alive retains the canonical NOR allocation. Consumers
/// may map it only through an immutable mapping contract and must never write it.
#[derive(Clone)]
pub struct ImmutableView { source: Source, len: usize }
impl ImmutableView {
    pub(super) fn new(source: Source, len: usize) -> Self { Self { source, len } }
    pub fn len(&self) -> usize { self.len }
    pub fn is_empty(&self) -> bool { self.len == 0 }
    pub fn as_ptr(&self) -> *const u8 { self.source.0.pointer.as_ptr().cast() }
    pub fn as_slice(&self) -> &[u8] { &self.source.as_slice()[..self.len] }
}
