//! Code tracking by host backing address, shared by CPU backends.

use std::ptr;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering::*},
};

const PAGE: usize = 4096;
const WORD: usize = 4;
const BUCKETS: usize = 4096;
const QUEUE_SIZE: usize = 4096;
// Each atomic word stores four byte values, four watched bits, and four bits
// indicating whether the saved values are still known.
const WATCHED_SHIFT: usize = 32;
const KNOWN_SHIFT: usize = WATCHED_SHIFT + WORD;

struct Node<T> {
    value: T,
    next: *mut Node<T>,
}

/// Nodes are published once and never removed while the list is shared.
struct List<T>(AtomicPtr<Node<T>>);

// A published node is immutable and its lifetime is tied to the list.
unsafe impl<T: Send> Send for List<T> {}
unsafe impl<T: Send + Sync> Sync for List<T> {}

impl<T> Default for List<T> {
    fn default() -> Self {
        Self(AtomicPtr::new(ptr::null_mut()))
    }
}

impl<T> List<T> {
    fn iter(&self, head: *mut Node<T>) -> impl Iterator<Item = &T> {
        std::iter::successors(self.node(head), |node| self.node(node.next)).map(|node| &node.value)
    }

    fn node(&self, node: *mut Node<T>) -> Option<&Node<T>> {
        // All non-null links are owned by this list and live as long as it does.
        unsafe { node.as_ref() }
    }

    fn insert(&self, value: T, matches: impl Fn(&T) -> bool) -> (&T, bool) {
        let mut node = Box::new(Node {
            value,
            next: ptr::null_mut(),
        });
        let mut head = self.0.load(Acquire);
        loop {
            if let Some(existing) = self.iter(head).find(|value| matches(value)) {
                return (existing, false);
            }
            node.next = head;
            let raw = Box::into_raw(node);
            match self.0.compare_exchange(head, raw, AcqRel, Acquire) {
                Ok(_) => return (&self.node(raw).unwrap().value, true),
                Err(current) => {
                    // A failed CAS leaves the allocation private.
                    node = unsafe { Box::from_raw(raw) };
                    head = current;
                }
            }
        }
    }
}

impl<T> Drop for List<T> {
    fn drop(&mut self) {
        let mut node = *self.0.get_mut();
        while !node.is_null() {
            let owned = unsafe { Box::from_raw(node) };
            node = owned.next;
        }
    }
}

struct WatchedPage {
    address: usize,
    /// One bit per backing word, with an exact byte mask and byte values in
    /// each marked word. Publishing a mask and its values is one atomic update.
    watched: [AtomicU64; PAGE / WORD / 64],
    words: [AtomicU64; PAGE / WORD],
}

fn word_spans(first: usize, end: usize) -> impl Iterator<Item = (usize, usize, usize)> {
    (first / WORD..end.div_ceil(WORD)).map(move |word| {
        let base = word * WORD;
        (word, first.saturating_sub(base), (end - base).min(WORD))
    })
}

fn byte_mask(first: usize, end: usize) -> u64 {
    ((1 << end) - 1) ^ ((1 << first) - 1)
}

impl WatchedPage {
    fn watch(&self, first: usize, bytes: &[u8]) {
        for (word, lo, hi) in word_spans(first, first + bytes.len()) {
            let mask = byte_mask(lo, hi);
            let mut values = [0; WORD];
            let offset = word * WORD + lo - first;
            values[lo..hi].copy_from_slice(&bytes[offset..offset + hi - lo]);
            let values = u64::from(u32::from_le_bytes(values));
            let value_mask = u64::from(u32::MAX >> (32 - (hi - lo) * 8)) << (lo * 8);
            let entry = &self.words[word];
            let update = |old| {
                (old & !value_mask) | values | (mask << WATCHED_SHIFT) | (mask << KNOWN_SHIFT)
            };
            let mut old = entry.load(Relaxed);
            while update(old) != old {
                match entry.compare_exchange_weak(old, update(old), AcqRel, Acquire) {
                    Ok(_) => break,
                    Err(current) => old = current,
                }
            }
            let bit = 1 << (word % 64);
            let bitmap = &self.watched[word / 64];
            if bitmap.load(Acquire) & bit == 0 {
                bitmap.fetch_or(bit, Release);
            }
        }
    }

    fn marked(&self, first: usize, end: usize) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
        word_spans(first, end)
            .filter(|&(word, _, _)| self.watched[word / 64].load(Acquire) & (1 << (word % 64)) != 0)
    }
}

struct Queue {
    slots: Box<[AtomicUsize]>,
    ready: [AtomicU64; QUEUE_SIZE / 64],
    next: AtomicUsize,
    pending: AtomicBool,
    overflow: AtomicBool,
}

impl Queue {
    fn push(&self, word: usize) {
        let index = self.next.fetch_add(1, Relaxed) % QUEUE_SIZE;
        // Store word+1 so zero can denote an empty slot.
        if self.slots[index]
            .compare_exchange(0, word + 1, AcqRel, Relaxed)
            .is_ok()
        {
            self.ready[index / 64].fetch_or(1 << (index % 64), Release);
        } else {
            self.overflow.store(true, Release);
        }
        self.pending.store(true, Release);
    }
}

/// A CPU's single-consumer inbox. Producers never wait for any consumer.
/// A collision requests a full flush instead of discarding a live slot.
pub struct Invalidations(Arc<Queue>);

impl Invalidations {
    /// Take this CPU's pending backing-word addresses. Returns `None` if
    /// there is no work, or `Some(false)` if a full cache flush is required.
    pub fn drain(&mut self, mut visit: impl FnMut(usize)) -> Option<bool> {
        if !self.0.pending.swap(false, AcqRel) {
            return None;
        }
        for (group, ready) in self.0.ready.iter().enumerate() {
            let mut bits = ready.swap(0, AcqRel);
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let word = self.0.slots[group * 64 + bit].swap(0, AcqRel);
                debug_assert_ne!(word, 0);
                visit(word - 1);
            }
        }
        Some(!self.0.overflow.swap(false, AcqRel))
    }
}

struct WatchedPages {
    pages: Box<[List<WatchedPage>]>,
    watch_revision: AtomicU64,
    subscribers: List<Weak<Queue>>,
}

impl Default for WatchedPages {
    fn default() -> Self {
        Self {
            pages: (0..BUCKETS).map(|_| List::default()).collect(),
            watch_revision: AtomicU64::new(0),
            subscribers: List::default(),
        }
    }
}

/// Shared tracking of translated code and translation metadata in host backing memory.
/// Aliased guest mappings share the same watched bytes.
#[derive(Clone, Default)]
pub struct CodeTracker(Arc<WatchedPages>);

impl CodeTracker {
    pub fn subscribe(&self) -> Invalidations {
        let queue = Arc::new(Queue {
            slots: (0..QUEUE_SIZE).map(|_| AtomicUsize::new(0)).collect(),
            ready: [const { AtomicU64::new(0) }; QUEUE_SIZE / 64],
            next: AtomicUsize::new(0),
            pending: AtomicBool::new(false),
            overflow: AtomicBool::new(false),
        });
        self.0.subscribers.insert(Arc::downgrade(&queue), |_| false);
        Invalidations(queue)
    }

    fn bucket(&self, page: usize) -> &List<WatchedPage> {
        let index = page / PAGE;
        &self.0.pages[(index ^ (index >> 12)) % BUCKETS]
    }

    fn page(&self, page: usize) -> Option<&WatchedPage> {
        let bucket = self.bucket(page);
        bucket
            .iter(bucket.0.load(Acquire))
            .find(|p| p.address == page)
    }

    /// Record the bytes used by a translation. Return the new watch revision
    /// if any backing page in the span was first registered by this call.
    pub fn watch_bytes(&self, pointer: *const u8, bytes: &[u8]) -> Option<u64> {
        let start = pointer.addr();
        let mut added = false;
        for (page, first, end) in spans(start, bytes.len()) {
            let (watched, new) = if let Some(watched) = self.page(page) {
                (watched, false)
            } else {
                self.bucket(page).insert(
                    WatchedPage {
                        address: page,
                        watched: [const { AtomicU64::new(0) }; PAGE / WORD / 64],
                        words: [const { AtomicU64::new(0) }; PAGE / WORD],
                    },
                    |p| p.address == page,
                )
            };
            let offset = page + first - start;
            watched.watch(first, &bytes[offset..offset + end - first]);
            added |= new;
        }
        added.then(|| self.0.watch_revision.fetch_add(1, Release).wrapping_add(1))
    }

    /// Changes when a previously unwatched page is registered.
    pub fn watch_revision(&self) -> u64 {
        self.0.watch_revision.load(Acquire)
    }

    pub fn watched(&self, pointer: *const u8) -> bool {
        self.page(pointer.addr() & !(PAGE - 1)).is_some()
    }

    pub fn watches_pages(&self, pointer: *const u8, count: usize) -> bool {
        spans(pointer.addr(), count).any(|(page, _, _)| self.page(page).is_some())
    }

    pub fn overlaps(&self, pointer: *const u8, count: usize) -> bool {
        spans(pointer.addr(), count).any(|(page, first, end)| {
            self.page(page).is_some_and(|watched| {
                watched.marked(first, end).any(|(word, lo, hi)| {
                    (watched.words[word].load(Acquire) >> WATCHED_SHIFT) & byte_mask(lo, hi) != 0
                })
            })
        })
    }

    /// Record a completed write and compare its watched bytes with the saved ones.
    ///
    /// # Safety
    /// The span must be valid for reading `count` initialized bytes without
    /// concurrent writes for the duration of this call.
    pub unsafe fn note_write(&self, pointer: *const u8, count: usize) {
        self.record_write(pointer, count, |word, entry, lo, hi| {
            let mut old = entry.load(Acquire);
            let mut changed = false;
            loop {
                let mut new = old;
                let mask = (old >> WATCHED_SHIFT) & byte_mask(lo, hi);
                for byte in lo..hi {
                    if mask & (1 << byte) == 0 {
                        continue;
                    }
                    let offset = word + byte - pointer.addr();
                    let value = unsafe { pointer.add(offset).read() };
                    new = (new & !(0xff << (byte * 8))) | (u64::from(value) << (byte * 8));
                }
                new |= mask << KNOWN_SHIFT;
                changed |= old != new;
                if old == new {
                    return changed;
                }
                match entry.compare_exchange_weak(old, new, AcqRel, Acquire) {
                    Ok(_) => return changed,
                    Err(current) => old = current,
                }
            }
        });
    }

    /// Invalidate watched bytes in a footprint which can write, such as a
    /// conditional atomic read-modify-write. This does not read backing RAM.
    pub fn note_possible_write(&self, pointer: *const u8, count: usize) {
        self.record_write(pointer, count, |_, entry, lo, hi| {
            let mask = byte_mask(lo, hi);
            (entry.fetch_and(!(mask << KNOWN_SHIFT), AcqRel) >> WATCHED_SHIFT) & mask != 0
        });
    }

    fn record_write(
        &self,
        pointer: *const u8,
        count: usize,
        mut changed: impl FnMut(usize, &AtomicU64, usize, usize) -> bool,
    ) {
        for (page, first, end) in spans(pointer.addr(), count) {
            let Some(watched) = self.page(page) else {
                continue;
            };
            for (word, lo, hi) in watched.marked(first, end) {
                let address = page + word * WORD;
                if changed(address, &watched.words[word], lo, hi) {
                    let subscribers = &self.0.subscribers;
                    for inbox in subscribers.iter(subscribers.0.load(Acquire)) {
                        if let Some(queue) = inbox.upgrade() {
                            queue.push(address);
                        }
                    }
                }
            }
        }
    }
}

fn spans(start: usize, count: usize) -> impl Iterator<Item = (usize, usize, usize)> {
    let end = start + count;
    ((start & !(PAGE - 1))..if count == 0 { 0 } else { end })
        .step_by(PAGE)
        .map(move |page| (page, start.saturating_sub(page), (end - page).min(PAGE)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_publishers_deliver_each_word_to_each_core() {
        let tracker = CodeTracker::default();
        let mut first = tracker.subscribe();
        let mut second = tracker.subscribe();
        let base = 0x1000;
        let workers = 8;
        let words = 64;
        std::thread::scope(|scope| {
            for worker in 0..workers {
                let tracker = tracker.clone();
                scope.spawn(move || {
                    let address = base + worker * words * WORD;
                    let backing = ptr::without_provenance(address);
                    let _ = tracker.watch_bytes(backing, &[0; 64 * WORD]);
                    tracker.note_possible_write(backing, words * WORD);
                });
            }
        });
        for queue in [&mut first, &mut second] {
            let mut addresses = Vec::new();
            assert_eq!(queue.drain(|address| addresses.push(address)), Some(true));
            addresses.sort_unstable();
            assert!(
                addresses
                    .into_iter()
                    .eq((base..base + workers * words * WORD).step_by(WORD))
            );
        }
    }

    #[test]
    fn slow_consumer_flushes_without_overflowing_another_core() {
        let tracker = CodeTracker::default();
        let mut slow = tracker.subscribe();
        let mut fast = tracker.subscribe();
        let backing = ptr::without_provenance(0x1000);
        let _ = tracker.watch_bytes(backing, &[0; WORD]);
        for _ in 0..=QUEUE_SIZE {
            tracker.note_possible_write(backing, WORD);
            assert_eq!(
                fast.drain(|word| assert_eq!(word, backing.addr())),
                Some(true)
            );
        }
        assert_eq!(slow.drain(|_| {}), Some(false));
        assert_eq!(slow.drain(|_| unreachable!()), None);
        tracker.note_possible_write(backing, WORD);
        assert_eq!(
            slow.drain(|word| assert_eq!(word, backing.addr())),
            Some(true)
        );
    }
}
