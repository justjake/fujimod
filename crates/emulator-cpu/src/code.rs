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
    /// Record four fetched bytes within one backing page. Return the new
    /// watch revision if this is the first registration on that page.
    pub fn watch_word(&self, pointer: *const u8, bytes: [u8; 4]) -> Option<u64> {
        let page = pointer.addr() & !4095;
        let mut pages = self.0.pages.borrow_mut();
        let new = !pages.contains_key(&page);
        let watched = pages.entry(page).or_insert_with(|| {
            Box::new(WatchedPage {
                seen: [0; 64],
                contents: [0; 4096],
            })
        });
        let byte = pointer.addr() - page;
        for (index, value) in (byte..byte + 4).zip(bytes) {
            watched.seen[index / 64] |= 1 << (index % 64);
            watched.contents[index] = value;
        }
        drop(pages);
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
                let byte = unsafe { pointer.add(page + index - start).read() };
                if std::mem::replace(&mut watched.contents[index], byte) == byte {
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
