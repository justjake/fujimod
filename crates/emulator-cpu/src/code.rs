//! Code tracking by host backing address, shared by CPU backends.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

struct WatchedPage {
    seen: [u64; 64],
    contents: [u8; 4096],
}

#[derive(Default)]
struct WatchedPages {
    pages: RefCell<HashMap<usize, Box<WatchedPage>>>,
    revision: Cell<u64>,
    watch_revision: Cell<u64>,
    changed: RefCell<VecDeque<usize>>,
}

/// Shared tracking of translated code and translation metadata in host backing memory.
/// Aliased guest mappings share the same watched bytes.
#[derive(Clone, Default)]
pub struct CodeTracker(Rc<WatchedPages>);

impl CodeTracker {
    /// Record the bytes used by a translation. Return the new watch revision
    /// if this is the first registration on any backing page in the span.
    pub fn watch_bytes(&self, pointer: *const u8, bytes: &[u8]) -> Option<u64> {
        if bytes.is_empty() {
            return None;
        }
        let start = pointer.addr();
        let end = start + bytes.len();
        let mut new = false;
        let mut pages = self.0.pages.borrow_mut();
        for page in ((start & !4095)..end).step_by(4096) {
            let watched = pages.entry(page).or_insert_with(|| {
                new = true;
                Box::new(WatchedPage {
                    seen: [0; 64],
                    contents: [0; 4096],
                })
            });
            let first = start.saturating_sub(page);
            let last = (end - page).min(4096);
            let offset = page + first - start;
            watched.contents[first..last].copy_from_slice(&bytes[offset..offset + last - first]);
            let mut index = first;
            while index < last {
                let bit = index % 64;
                let width = (last - index).min(64 - bit);
                watched.seen[index / 64] |= (u64::MAX >> (64 - width)) << bit;
                index += width;
            }
        }
        new.then(|| {
            let revision = self.watch_revision().wrapping_add(1);
            self.0.watch_revision.set(revision);
            revision
        })
    }

    /// Changes when a previously unwatched page is registered.
    pub fn watch_revision(&self) -> u64 {
        self.0.watch_revision.get()
    }

    /// Visit the host addresses of all watched 4 KiB backing pages.
    pub fn watched_pages(&self, visit: impl FnMut(usize)) {
        self.0.pages.borrow().keys().copied().for_each(visit);
    }

    /// Changes when a write alters watched bytes.
    pub fn revision(&self) -> u64 {
        self.0.revision.get()
    }

    /// Whether the backing page contains any watched bytes.
    pub fn watched(&self, pointer: *const u8) -> bool {
        self.0
            .pages
            .borrow()
            .contains_key(&(pointer.addr() & !4095))
    }

    /// Whether any backing page in the span contains watched bytes.
    pub fn watches_pages(&self, pointer: *const u8, count: usize) -> bool {
        let start = pointer.addr();
        let pages = self.0.pages.borrow();
        count != 0
            && ((start & !4095)..start + count)
                .step_by(4096)
                .any(|page| pages.contains_key(&page))
    }

    /// Whether the given backing span contains any watched bytes.
    pub fn overlaps(&self, pointer: *const u8, count: usize) -> bool {
        if count == 0 {
            return false;
        }
        let start = pointer.addr();
        let pages = self.0.pages.borrow();
        let end = start + count;
        ((start & !4095)..end).step_by(4096).any(|page| {
            pages.get(&page).is_some_and(|bytes| {
                (start.saturating_sub(page)..(end - page).min(4096))
                    .any(|byte| bytes.seen[byte / 64] & (1 << (byte % 64)) != 0)
            })
        })
    }

    /// Record a completed write and compare its watched bytes with the saved ones.
    ///
    /// # Safety
    /// The span must be valid for reading `count` initialized bytes without
    /// concurrent writes for the duration of this call.
    pub unsafe fn note_write(&self, pointer: *const u8, count: usize) {
        self.record_write(pointer, count, |watched, index, offset| {
            let byte = unsafe { pointer.add(offset).read() };
            std::mem::replace(&mut watched.contents[index], byte) != byte
        });
    }

    /// Invalidate watched bytes in the footprint of a direct access which
    /// can write, such as a conditional atomic read-modify-write.
    pub fn note_possible_write(&self, pointer: *const u8, count: usize) {
        self.record_write(pointer, count, |_, _, _| true);
    }

    fn record_write(
        &self,
        pointer: *const u8,
        count: usize,
        mut changed_byte: impl FnMut(&mut WatchedPage, usize, usize) -> bool,
    ) {
        if count == 0 {
            return;
        }
        let start = pointer.addr();
        let end = start + count;
        let mut pages = self.0.pages.borrow_mut();
        let mut latest = None;
        for page in ((start & !4095)..end).step_by(4096) {
            let Some(watched) = pages.get_mut(&page) else {
                continue;
            };
            for index in start.saturating_sub(page)..(end - page).min(4096) {
                if watched.seen[index / 64] & (1 << (index % 64)) == 0 {
                    continue;
                }
                if !changed_byte(watched, index, page + index - start) {
                    continue;
                }
                let word = (page + index) & !3;
                if latest == Some(word) {
                    continue;
                }
                latest = Some(word);
                let mut changed = self.0.changed.borrow_mut();
                if changed.len() == 4096 {
                    changed.pop_front();
                }
                changed.push_back(word);
                self.0.revision.set(self.0.revision.get().wrapping_add(1));
            }
        }
    }

    /// Visit changed backing-word addresses since the given revision.
    /// Return false if the bounded history cannot cover that revision.
    pub fn changed_since(&self, from: u64, mut visit: impl FnMut(usize)) -> bool {
        let delta = self.revision().wrapping_sub(from);
        let changed = self.0.changed.borrow();
        if delta > changed.len() as u64 {
            return false;
        }
        changed
            .iter()
            .skip(changed.len() - delta as usize)
            .copied()
            .for_each(&mut visit);
        true
    }
}
