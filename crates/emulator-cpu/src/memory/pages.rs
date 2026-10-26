//! Immutable page versions with conservative engine/DMA dirty tracking.
//! Keyframes never alias mutable guest backing. Only flagged pages are read
//! after an initial capture; equal or zero candidates retain shared pages.
use crate::BackingObserver;
mod checkpoint;
use std::{
    cell::RefCell,
    ptr::NonNull,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, OnceLock},
};

pub const PAGE_BYTES: usize = 4096;
type Page = [u8; PAGE_BYTES];
#[derive(Clone)]
pub struct Image {
    len: usize,
    pages: Arc<[Arc<Page>]>,
}
/// Stable bitmap allocation retained until every observing processor is gone.
/// Native processor threads and the owner may flag pages concurrently.
pub struct Tracker {
    len: usize,
    dirty: Box<[AtomicU64]>,
    base: RefCell<Option<Image>>,
}
fn zero() -> &'static Arc<Page> {
    static ZERO: OnceLock<Arc<Page>> = OnceLock::new();
    ZERO.get_or_init(|| Arc::new([0; PAGE_BYTES]))
}
impl Image {
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn pages(&self) -> usize {
        self.pages.len()
    }
    pub fn shared_pages(&self, other: &Self) -> usize {
        self.pages
            .iter()
            .zip(other.pages.iter())
            .filter(|(a, b)| Arc::ptr_eq(a, b))
            .count()
    }
}
impl Tracker {
    pub fn new(len: usize) -> Self {
        Self {
            len,
            dirty: (0..len.div_ceil(PAGE_BYTES * 64))
                .map(|_| AtomicU64::new(0))
                .collect(),
            base: RefCell::new(None),
        }
    }
    pub fn observer(&self, backing: NonNull<u8>) -> Option<BackingObserver> {
        (self.len != 0).then(|| BackingObserver {
            backing,
            bytes: self.len,
            bitmap: NonNull::new(self.dirty.as_ptr() as *mut AtomicU64).unwrap(),
            page_shift: 12,
        })
    }
    pub fn mark(&self, offset: usize, bytes: usize) -> Result<(), String> {
        if offset.checked_add(bytes).is_none_or(|end| end > self.len) {
            return Err("dirty range escapes backing".into());
        }
        if bytes != 0 {
            for page in offset / PAGE_BYTES..=(offset + bytes - 1) / PAGE_BYTES {
                self.dirty[page / 64].fetch_or(1 << (page % 64), Ordering::Relaxed);
            }
        }
        Ok(())
    }
    fn dirty(&self, page: usize) -> bool {
        self.dirty[page / 64].load(Ordering::Relaxed) & (1 << (page % 64)) != 0
    }
    fn set_base(&self, image: &Image) {
        for flag in self.dirty.iter() {
            flag.store(0, Ordering::Relaxed);
        }
        *self.base.borrow_mut() = Some(image.clone());
    }
    /// # Safety
    /// Every candidate (dirty or initial) page is readable at `pointer`;
    /// clean pages are never accessed. All CPUs are stopped. Before any
    /// processor resumes, re-arm its backing observers to flush cached TLBs.
    /// Every host/DMA write must call `mark` in the same physical domain.
    pub unsafe fn capture(&self, pointer: *const u8) -> Image {
        let previous = self.base.borrow();
        let mut pages = Vec::with_capacity(self.len.div_ceil(PAGE_BYTES));
        let zero_page = zero();
        for offset in (0..self.len).step_by(PAGE_BYTES) {
            let index = offset / PAGE_BYTES;
            let size = PAGE_BYTES.min(self.len - offset);
            let old = previous.as_ref().and_then(|p| p.pages.get(index));
            if let Some(old) = old.filter(|_| !self.dirty(index)) {
                pages.push(old.clone());
                continue;
            }
            let bytes = unsafe { std::slice::from_raw_parts(pointer.add(offset), size) };
            let page = if let Some(old) = old.filter(|p| p[..size] == *bytes) {
                old.clone()
            } else if bytes == &zero_page[..size] {
                zero_page.clone()
            } else {
                let mut page = [0; PAGE_BYTES];
                page[..size].copy_from_slice(bytes);
                Arc::new(page)
            };
            pages.push(page);
        }
        let image = Image {
            len: self.len,
            pages: pages.into(),
        };
        drop(previous);
        self.set_base(&image);
        image
    }
    /// # Safety
    /// Exclusive paused access to writable backing. Notify code caches and
    /// monitors of the returned ranges, then re-arm all engine observers.
    pub unsafe fn restore(
        &self,
        image: &Image,
        pointer: *mut u8,
    ) -> Result<Vec<(usize, usize)>, String> {
        if self.len != image.len {
            return Err("keyframe memory extent differs".into());
        }
        let previous = self.base.borrow();
        let mut changed: Vec<(usize, usize)> = Vec::new();
        for (index, page) in image.pages.iter().enumerate() {
            let old = previous.as_ref().and_then(|p| p.pages.get(index));
            if !self.dirty(index) && old.is_some_and(|old| Arc::ptr_eq(old, page)) {
                continue;
            }
            let offset = index * PAGE_BYTES;
            let size = PAGE_BYTES.min(self.len - offset);
            let current = unsafe { std::slice::from_raw_parts(pointer.add(offset), size) };
            if current == &page[..size] {
                continue;
            }
            unsafe { std::ptr::copy_nonoverlapping(page.as_ptr(), pointer.add(offset), size) };
            if let Some((start, bytes)) = changed.last_mut()
                && *start + *bytes == offset
            {
                *bytes += size;
            } else {
                changed.push((offset, size));
            }
        }
        drop(previous);
        self.set_base(image);
        Ok(changed)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dirty_versions_share_pages_restore_branches_and_track_partial_pages() {
        let mut ram = vec![0; PAGE_BYTES * 3 + 7];
        ram[PAGE_BYTES] = 42;
        let tracker = Tracker::new(ram.len());
        let first = unsafe { tracker.capture(ram.as_ptr()) };
        let same = unsafe { tracker.capture(ram.as_ptr()) };
        assert_eq!(first.shared_pages(&same), 4);
        assert!(Arc::ptr_eq(&first.pages[0], &first.pages[3]));
        ram[PAGE_BYTES * 2] = 99;
        ram[PAGE_BYTES * 3 + 6] = 11;
        tracker.mark(PAGE_BYTES * 2, PAGE_BYTES + 7).unwrap();
        let next = unsafe { tracker.capture(ram.as_ptr()) };
        assert_eq!(first.shared_pages(&next), 2);
        assert_eq!(
            unsafe { tracker.restore(&first, ram.as_mut_ptr()) }.unwrap(),
            [(PAGE_BYTES * 2, PAGE_BYTES + 7)]
        );
        ram[PAGE_BYTES] = 8;
        tracker.mark(PAGE_BYTES, 1).unwrap();
        assert_eq!(
            unsafe { tracker.restore(&next, ram.as_mut_ptr()) }.unwrap(),
            [(PAGE_BYTES, PAGE_BYTES * 2 + 7)]
        );
        assert_eq!(ram[PAGE_BYTES], 42);
        assert_eq!(ram[PAGE_BYTES * 2], 99);
        assert_eq!(ram.last(), Some(&11));
        assert!(
            unsafe { tracker.restore(&next, ram.as_mut_ptr()) }
                .unwrap()
                .is_empty()
        );
        assert!(tracker.mark(ram.len(), 1).is_err());
        let empty = Tracker::new(0);
        let image = unsafe { empty.capture(std::ptr::null()) };
        assert!(
            unsafe { empty.restore(&image, std::ptr::null_mut()) }
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn clean_pages_are_not_read_when_capturing_the_next_version() {
        let mut ram = vec![0; PAGE_BYTES * 2];
        ram[0] = 1;
        let tracker = Tracker::new(ram.len());
        let first = unsafe { tracker.capture(ram.as_ptr()) };
        // A sentinel pointer proves a completely clean capture never reads
        // backing: no candidate page requires a readable pointer in this case.
        let same = unsafe { tracker.capture(std::ptr::NonNull::<u8>::dangling().as_ptr()) };
        assert_eq!(first.shared_pages(&same), 2);
    }
}
