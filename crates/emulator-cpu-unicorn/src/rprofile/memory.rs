//! Shared backing identities, translated bytes and alias-coherent code writes.
//! Only the CPU backend uses this execution mechanism. It never supplies a
//! device with program inspection or private RAM predicates.
use super::core::Execution;
use fujimod_emulator_cpu::exclusive;
use fujimod_emulator_cpu::{Cpu, ExecutionPolicy};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
struct View {
    core: usize,
    address: u64,
    pointer: usize,
    bytes: usize,
    physical: Option<u64>,
}
struct Chunks<'a> {
    views: &'a [View],
    core: usize,
    cursor: u64,
    end: u64,
    require_backing: bool,
}
impl<'a> Chunks<'a> {
    fn new(
        views: &'a [View],
        core: usize,
        address: u64,
        bytes: usize,
        require_backing: bool,
    ) -> Result<Self, String> {
        Ok(Self {
            views,
            core,
            cursor: address,
            end: address
                .checked_add(bytes as u64)
                .ok_or("CPU memory range overflow")?,
            require_backing,
        })
    }
}
impl Iterator for Chunks<'_> {
    type Item = Result<(usize, usize), String>;
    fn next(&mut self) -> Option<Self::Item> {
        while self.cursor < self.end {
            if let Some(v) = self.views.iter().find(|v| {
                v.core == self.core
                    && self.cursor >= v.address
                    && self.cursor < v.address + v.bytes as u64
            }) {
                let bytes = (self.end.min(v.address + v.bytes as u64) - self.cursor) as usize;
                let pointer = v.pointer + (self.cursor - v.address) as usize;
                self.cursor += bytes as u64;
                return Some(Ok((pointer, bytes)));
            }
            if self.require_backing {
                let address = self.cursor;
                self.cursor = self.end;
                return Some(Err(format!(
                    "CPU memory has no owned backing at {address:#x}"
                )));
            }
            self.cursor = self
                .views
                .iter()
                .filter(|v| v.core == self.core && v.address > self.cursor)
                .map(|v| v.address)
                .min()
                .unwrap_or(self.end)
                .min(self.end);
        }
        None
    }
}
struct Page {
    bits: Box<[u64; 64]>,
    invalid: bool,
}
#[derive(Clone, Copy)]
struct Invalidation {
    core: usize,
    address: u64,
    backing: usize,
}
#[derive(Default)]
struct State {
    cores: Vec<bool>,
    views: Vec<View>,
    bindings: Vec<View>,
    pages: HashMap<usize, Page>,
    watches: Vec<(usize, u64)>,
    invalidations: Vec<Invalidation>,
    dirty_pages: Vec<usize>,
    active: HashSet<usize>,
    run_policies: HashMap<usize, (ExecutionPolicy, bool)>,
    parked: HashMap<usize, usize>,
    owner_barrier: Option<usize>,
}
/// Thread-safe ownership for memory identities and cross-processor code
/// coherence. It contains no CPU handles; each processor creates its own
/// thread-affine [`Memory`] view inside its owner thread.
#[derive(Clone)]
pub struct SharedMemory {
    state: Arc<Mutex<State>>,
    monitors: exclusive::Shared,
}
impl Default for SharedMemory {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            monitors: Arc::new(Mutex::new(exclusive::Monitors::default())),
        }
    }
}
impl SharedMemory {
    /// Create the thread-affine view used by one processor owner thread.
    pub fn processor(&self) -> Memory {
        Memory {
            shared: self.clone(),
            local: Rc::new(Local::default()),
            monitors: self.monitors.clone(),
        }
    }
    pub fn exclusive_domain(&self) -> exclusive::Shared {
        self.monitors.clone()
    }
}
#[derive(Default)]
struct Local {
    cores: RefCell<HashMap<usize, Weak<Execution>>>,
    active: Cell<Option<usize>>,
}
#[derive(Clone)]
pub struct Memory {
    shared: SharedMemory,
    local: Rc<Local>,
    pub(super) monitors: exclusive::Shared,
}
pub(super) struct RunReservation<'a> {
    memory: &'a Memory,
    core: usize,
}
pub(super) struct OwnerBarrierLease {
    shared: SharedMemory,
    core: usize,
}
impl Drop for OwnerBarrierLease {
    fn drop(&mut self) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.owner_barrier == Some(self.core) {
            state.owner_barrier = None;
        }
    }
}
impl Drop for RunReservation<'_> {
    fn drop(&mut self) {
        self.memory.finish_run(self.core);
    }
}
impl Default for Memory {
    fn default() -> Self {
        SharedMemory::default().processor()
    }
}
impl Memory {
    /// Return the send-safe cross-thread coordinator for this memory space.
    pub fn shared(&self) -> SharedMemory {
        self.shared.clone()
    }
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub(super) fn require_stopped(&self) -> Result<(), String> {
        if !Self::quiescent(&self.state()) {
            return Err("R-profile memory mapping requires all processors to be stopped".into());
        }
        Ok(())
    }
    pub(super) fn acquire_owner_barrier(&self, core: usize) -> Option<OwnerBarrierLease> {
        let mut state = self.state();
        let has_peer = state
            .cores
            .iter()
            .enumerate()
            .any(|(id, live)| id != core && *live);
        let all_parked = state
            .run_policies
            .keys()
            .all(|id| state.active.contains(id) && state.parked.get(id).copied().unwrap_or(0) != 0);
        if !has_peer
            || state.owner_barrier.is_some()
            || !state.run_policies.contains_key(&core)
            || !state.active.contains(&core)
            || state.parked.get(&core).copied().unwrap_or(0) == 0
            || !all_parked
        {
            return None;
        }
        state.owner_barrier = Some(core);
        Some(OwnerBarrierLease {
            shared: self.shared.clone(),
            core,
        })
    }
    fn quiescent(state: &State) -> bool {
        Self::quiescent_except(state, None)
    }
    fn quiescent_except(state: &State, allowed: Option<usize>) -> bool {
        state
            .active
            .iter()
            .all(|core| Some(*core) == allowed || state.parked.get(core).copied().unwrap_or(0) != 0)
    }
    pub(super) fn set_parked(&self, core: usize, parked: bool) -> Result<(), String> {
        let mut state = self.state();
        if !parked && state.owner_barrier.is_some_and(|owner| owner != core) {
            return Err(
                "cannot resume a peer during another processor's synchronization barrier".into(),
            );
        }
        if !state.active.contains(&core) {
            return Err("cannot park an R-profile processor outside a run".into());
        }
        if parked {
            *state.parked.entry(core).or_default() += 1;
        } else {
            let depth = state
                .parked
                .get_mut(&core)
                .ok_or("R-profile processor is not parked")?;
            *depth -= 1;
            if *depth == 0 {
                state.parked.remove(&core);
            }
        }
        Ok(())
    }
    pub(super) fn reserve_run(
        &self,
        core: usize,
        policy: ExecutionPolicy,
        parallel_tcg: bool,
    ) -> Result<RunReservation<'_>, String> {
        let mut state = self.state();
        if state.owner_barrier.is_some_and(|owner| owner != core) {
            return Err("a peer R-profile run is held at an owner synchronization barrier".into());
        }
        if state.run_policies.contains_key(&core) {
            return Err("R-profile processor is already running".into());
        }
        if !state.run_policies.is_empty()
            && (policy != ExecutionPolicy::Interactive
                || !parallel_tcg
                || state
                    .run_policies
                    .values()
                    .any(|(active, parallel)| *active != ExecutionPolicy::Interactive || !parallel))
        {
            return Err(
                "concurrent R-profile processor runs require interactive execution policy and parallel TCG on every processor".into(),
            );
        }
        state.run_policies.insert(core, (policy, parallel_tcg));
        Ok(RunReservation { memory: self, core })
    }
    pub(super) fn run_reserved(&self, core: usize) -> bool {
        self.state().run_policies.contains_key(&core)
    }
    fn finish_run(&self, core: usize) {
        if self.local.active.get() == Some(core) {
            self.end();
        }
        let mut state = self.state();
        state.active.remove(&core);
        state.run_policies.remove(&core);
        state.parked.remove(&core);
    }
    pub(super) fn register(&self, core: Weak<Execution>) -> usize {
        let mut s = self.state();
        let id = s.cores.len();
        s.cores.push(true);
        self.local.cores.borrow_mut().insert(id, core);
        id
    }
    pub(super) fn start(&self, core: usize) -> Result<(), String> {
        let mut state = self.state();
        if !state.run_policies.contains_key(&core) {
            return Err("R-profile processor run was not reserved".into());
        }
        if !state.active.insert(core) {
            return Err("R-profile processor is already running".into());
        }
        state.parked.remove(&core);
        self.local.active.set(Some(core));
        Ok(())
    }
    pub(super) fn end(&self) {
        let active = self.local.active.replace(None);
        let mut state = self.state();
        if let Some(active) = active {
            state.active.remove(&active);
            state.parked.remove(&active);
        }
    }
    pub(super) fn validate_mapping(
        &self,
        pointer: usize,
        bytes: usize,
        physical: Option<u64>,
    ) -> Result<(), String> {
        let Some(physical) = physical else {
            return Ok(());
        };
        for v in &self.state().bindings {
            let Some(other) = v.physical else {
                continue;
            };
            let begin = pointer.max(v.pointer);
            let end = (pointer + bytes).min(v.pointer + v.bytes);
            if begin < end
                && physical + (begin - pointer) as u64 != other + (begin - v.pointer) as u64
            {
                return Err("shared backing has conflicting physical RAM identities".into());
            }
            let begin = physical.max(other);
            let end = (physical + bytes as u64).min(other + v.bytes as u64);
            if begin < end
                && pointer + (begin - physical) as usize != v.pointer + (begin - other) as usize
            {
                return Err("physical RAM identity has different shared backing".into());
            }
        }
        Ok(())
    }
    pub(super) fn declare(
        &self,
        core: usize,
        address: u64,
        pointer: usize,
        bytes: usize,
        physical: Option<u64>,
    ) {
        let mut s = self.state();
        s.bindings.push(View {
            core,
            address,
            pointer,
            bytes,
            physical,
        });
        let capacity = s.bindings.len() + s.cores.len() * 4;
        let len = s.views.len();
        s.views.reserve(capacity.saturating_sub(len));
    }
    pub(super) fn undeclare(&self, core: usize, address: u64, bytes: usize) {
        Self::remove_range(&mut self.state().bindings, core, address, bytes);
    }
    pub(super) fn forget(&self, core: usize) {
        let mut s = self.state();
        s.views.retain(|v| v.core != core);
        s.bindings.retain(|v| v.core != core);
        s.watches.retain(|(id, _)| *id != core);
        let mut dirty = Vec::new();
        s.invalidations.retain(|invalidation| {
            if invalidation.core == core {
                dirty.push(invalidation.backing);
                false
            } else {
                true
            }
        });
        s.cores[core] = false;
        self.local.cores.borrow_mut().remove(&core);
        dirty.sort_unstable();
        dirty.dedup();
        for backing in dirty {
            if !s
                .invalidations
                .iter()
                .any(|invalidation| invalidation.backing == backing)
            {
                if let Some(page) = s.pages.get_mut(&backing) {
                    page.invalid = false;
                }
                s.dirty_pages.retain(|page| *page != backing);
            }
        }
    }
    pub(super) fn map(
        &self,
        core: usize,
        address: u64,
        pointer: usize,
        bytes: usize,
        physical: Option<u64>,
    ) {
        let mut s = self.state();
        let view = View {
            core,
            address,
            pointer,
            bytes,
            physical,
        };
        // A newly mapped alias must watch code already translated elsewhere.
        let State { pages, watches, .. } = &mut *s;
        for &page in pages.keys() {
            Self::aliases(&[view], page, watches);
        }
        s.views.push(view);
        // Translation-time reservation bounds all later code-write queues.
        let capacity = s
            .pages
            .len()
            .saturating_mul(s.views.len())
            .saturating_mul(2);
        let len = s.invalidations.len();
        s.invalidations.reserve(capacity.saturating_sub(len));
        let capacity = s.views.len() * 4;
        let len = s.watches.len();
        s.watches.reserve(capacity.saturating_sub(len));
    }
    pub(super) fn unmap(&self, core: usize, address: u64, bytes: usize) {
        Self::remove_range(&mut self.state().views, core, address, bytes);
    }
    fn remove_range(views: &mut Vec<View>, core: usize, address: u64, bytes: usize) {
        let end = address + bytes as u64;
        let mut i = 0;
        while i < views.len() {
            let v = views[i];
            if v.core != core || v.address >= end || address >= v.address + v.bytes as u64 {
                i += 1;
                continue;
            }
            let left = v.address < address;
            if left {
                views[i].bytes = (address - v.address) as usize;
            }
            if v.address + v.bytes as u64 > end {
                let right = View {
                    address: end,
                    pointer: v.pointer + (end - v.address) as usize,
                    bytes: (v.address + v.bytes as u64 - end) as usize,
                    core,
                    physical: v.physical.map(|p| p + end - v.address),
                };
                if left {
                    views.push(right);
                } else {
                    views[i] = right;
                }
            } else if !left {
                views.swap_remove(i);
                continue;
            }
            i += 1;
        }
    }
    fn aliases(views: &[View], page: usize, out: &mut Vec<(usize, u64)>) {
        for v in views {
            let begin = page.max(v.pointer);
            let end = (page + 4096).min(v.pointer + v.bytes);
            if begin >= end {
                continue;
            }
            let first = (v.address + (begin - v.pointer) as u64) >> 12;
            let last = (v.address + (end - v.pointer - 1) as u64) >> 12;
            for p in first..=last {
                out.push((v.core, p << 12));
            }
        }
    }
    fn invalidation_aliases(views: &[View], backing: usize, out: &mut Vec<Invalidation>) {
        for v in views {
            let begin = backing.max(v.pointer);
            let end = (backing + 4096).min(v.pointer + v.bytes);
            if begin >= end {
                continue;
            }
            let first = (v.address + (begin - v.pointer) as u64) >> 12;
            let last = (v.address + (end - v.pointer - 1) as u64) >> 12;
            for page in first..=last {
                out.push(Invalidation {
                    core: v.core,
                    address: page << 12,
                    backing,
                });
            }
        }
    }
    fn canonical_pointer(views: &[View], pointer: usize, bytes: usize) -> Option<u64> {
        views.iter().find_map(|v| {
            if pointer >= v.pointer && pointer.checked_add(bytes)? <= v.pointer + v.bytes {
                v.physical.map(|p| p + (pointer - v.pointer) as u64)
            } else {
                None
            }
        })
    }
    pub(super) fn canonical(&self, core: usize, address: u64, bytes: usize) -> Result<u64, String> {
        let s = self.state();
        let (pointer, _) = Chunks::new(&s.views, core, address, bytes, true)?
            .next()
            .ok_or("empty R-profile exclusive range")??;
        Self::canonical_pointer(&s.bindings, pointer, bytes)
            .ok_or("R-profile exclusive is outside checked physical RAM views".into())
    }
    pub(super) fn watch_exclusive(
        &self,
        core: usize,
        address: u64,
        bytes: usize,
    ) -> Result<(), String> {
        {
            let mut s = self.state();
            let (pointer, _) = Chunks::new(&s.views, core, address, bytes, true)?
                .next()
                .ok_or("empty R-profile exclusive range")??;
            let State { views, watches, .. } = &mut *s;
            for page in ((pointer & !4095)..=((pointer + bytes - 1) & !4095)).step_by(4096) {
                Self::aliases(views, page, watches);
            }
        }
        self.apply_watches()
    }
    /// The 4 KiB guest pages of `[begin, end)` that hold translated code, or
    /// None when part of the range has no owned memory (code there is not
    /// tracked). A bounded code observer re-translates just these pages.
    pub(super) fn translated_pages(&self, core: usize, begin: u64, end: u64) -> Option<Vec<u64>> {
        let s = self.state();
        let bytes = end.checked_sub(begin)? as usize;
        let mut guest = begin;
        let mut pages = std::collections::BTreeSet::new();
        for chunk in Chunks::new(&s.views, core, begin, bytes, true).ok()? {
            let (pointer, length) = chunk.ok()?;
            let mut host = pointer & !4095;
            while host < pointer + length {
                if s.pages.contains_key(&host) {
                    let low = guest + (host.max(pointer) - pointer) as u64;
                    let high = guest + ((host + 4096).min(pointer + length) - pointer) as u64;
                    pages.extend(((low & !4095)..high).step_by(4096));
                }
                host += 4096;
            }
            guest += length as u64;
        }
        Some(pages.into_iter().collect())
    }
    pub(super) fn note_code(
        &self,
        core: usize,
        address: u64,
        bytes: usize,
    ) -> Result<bool, String> {
        let mut s = self.state();
        let State {
            views,
            pages,
            watches,
            invalidations,
            dirty_pages,
            ..
        } = &mut *s;
        let old = watches.len();
        for chunk in Chunks::new(views, core, address, bytes, true)? {
            let (pointer, bytes) = chunk?;
            let mut cursor = pointer;
            let end = pointer
                .checked_add(bytes)
                .ok_or("CPU memory range overflow")?;
            while cursor < end {
                let base = cursor & !4095;
                let page_end = end.min(base + 4096);
                let begin_offset = cursor - base;
                let end_offset = page_end - base;
                let inserted = match pages.entry(base) {
                    std::collections::hash_map::Entry::Occupied(mut entry) => {
                        Self::mark_code_range(
                            entry.get_mut().bits.as_mut(),
                            begin_offset,
                            end_offset,
                        );
                        false
                    }
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let mut page = Page {
                            bits: Box::new([0; 64]),
                            invalid: false,
                        };
                        Self::mark_code_range(page.bits.as_mut(), begin_offset, end_offset);
                        entry.insert(page);
                        true
                    }
                };
                if inserted {
                    Self::aliases(views, base, watches);
                    let capacity = pages.len() * views.len() * 2;
                    invalidations.reserve(capacity.saturating_sub(invalidations.len()));
                    dirty_pages.reserve(pages.len().saturating_sub(dirty_pages.len()));
                }
                cursor = page_end;
            }
        }
        Ok(watches.len() != old)
    }
    fn mark_code_range(bits: &mut [u64; 64], begin: usize, end: usize) {
        let mut offset = begin;
        while offset < end {
            let word = offset / 64;
            let bit = offset % 64;
            let count = (end - offset).min(64 - bit);
            let mask = if count == 64 {
                u64::MAX
            } else {
                ((1u64 << count) - 1) << bit
            };
            bits[word] |= mask;
            offset += count;
        }
    }
    pub(super) fn apply_watches(&self) -> Result<(), String> {
        loop {
            let next = {
                let local = self.local.cores.borrow();
                let mut s = self.state();
                let position = s.watches.iter().rposition(|(id, _)| local.contains_key(id));
                position.map(|position| {
                    let (id, page) = s.watches.swap_remove(position);
                    (local.get(&id).and_then(Weak::upgrade), page)
                })
            };
            let Some((core, page)) = next else {
                return Ok(());
            };
            if let Some(core) = core {
                core.cpu.watch_stores(page, 4096)?;
            }
        }
    }
    pub(super) fn written(&self, core: usize, address: u64, bytes: usize) -> Result<bool, String> {
        self.record_write(core, address, bytes, true)
    }
    pub(super) fn guest_store(
        &self,
        core: usize,
        address: u64,
        bytes: usize,
    ) -> Result<bool, String> {
        // Unicorn's MMIO slow path calls every covering store hook, including
        // watched hooks. Non-RAM accesses have no translated backing here;
        // the mapped device or the processor's fault path owns their effects.
        self.record_write(core, address, bytes, false)
    }
    fn record_write(
        &self,
        core: usize,
        address: u64,
        bytes: usize,
        require_backing: bool,
    ) -> Result<bool, String> {
        if bytes == 0 {
            return Ok(false);
        }
        let mut s = self.state();
        let State {
            views,
            bindings,
            pages,
            invalidations,
            dirty_pages,
            ..
        } = &mut *s;
        let mut changed = false;
        for chunk in Chunks::new(views, core, address, bytes, require_backing)? {
            let (pointer, count) = chunk?;
            if let Some(physical) = Self::canonical_pointer(bindings, pointer, count) {
                self.shared
                    .monitors
                    .lock()
                    .map_err(|_| "exclusive monitors poisoned")?
                    .written(Some((physical, count)))?;
            }
            changed |=
                Self::record_backing(views, pages, invalidations, dirty_pages, pointer, count);
        }
        Ok(changed)
    }
    pub(super) fn backing_written(
        &self,
        physical: u64,
        pointer: usize,
        bytes: usize,
    ) -> Result<bool, String> {
        if bytes == 0 {
            return Ok(false);
        }
        self.validate_mapping(pointer, bytes, Some(physical))?;
        self.shared
            .monitors
            .lock()
            .map_err(|_| "exclusive monitors poisoned")?
            .written(Some((physical, bytes)))?;
        let mut s = self.state();
        let State {
            views,
            pages,
            invalidations,
            dirty_pages,
            ..
        } = &mut *s;
        Ok(Self::record_backing(
            views,
            pages,
            invalidations,
            dirty_pages,
            pointer,
            bytes,
        ))
    }
    fn record_backing(
        views: &[View],
        pages: &mut HashMap<usize, Page>,
        invalidations: &mut Vec<Invalidation>,
        dirty_pages: &mut Vec<usize>,
        pointer: usize,
        bytes: usize,
    ) -> bool {
        let mut changed = false;
        for base in ((pointer & !4095)..=((pointer + bytes - 1) & !4095)).step_by(4096) {
            let Some(page) = pages.get_mut(&base) else {
                continue;
            };
            if page.invalid {
                continue;
            }
            let touched = (pointer.max(base)..(pointer + bytes).min(base + 4096)).any(|p| {
                let offset = p & 4095;
                page.bits[offset / 64] & (1 << (offset % 64)) != 0
            });
            if !touched {
                continue;
            }
            page.invalid = true;
            page.bits.fill(0);
            changed = true;
            dirty_pages.push(base);
            Self::invalidation_aliases(views, base, invalidations);
        }
        changed
    }
    pub(super) fn flush_pending(&self) -> bool {
        let local = self.local.cores.borrow();
        self.state()
            .invalidations
            .iter()
            .any(|invalidation| local.contains_key(&invalidation.core))
    }
    pub(super) fn prepare_external_write(&self) -> Result<(), String> {
        let state = self.state();
        if !Self::quiescent_except(&state, self.local.active.get()) {
            return Err(
                "code write requires all running processors to reach an owner barrier".into(),
            );
        }
        let active = state.active.clone();
        drop(state);
        for id in active {
            if let Some(core) = self.local.cores.borrow().get(&id).and_then(Weak::upgrade) {
                core.prepare_flush()?;
            }
        }
        self.flush()?;
        Ok(())
    }
    pub(super) fn flush(&self) -> Result<(), String> {
        if let Some(core) = self.local.active.get() {
            let state = self.state();
            if !state.active.contains(&core)
                || state.parked.get(&core).copied().unwrap_or(0) == 0
                    && !Self::quiescent_except(&state, Some(core))
            {
                return Err(
                    "code invalidation requires the local processor to be stopped or at an owner barrier".into(),
                );
            }
        }
        let local = self.local.cores.borrow();
        loop {
            let next = {
                let mut s = self.state();
                let position = s
                    .invalidations
                    .iter()
                    .rposition(|invalidation| local.contains_key(&invalidation.core));
                position.map(|position| {
                    let invalidation = s.invalidations.swap_remove(position);
                    (
                        invalidation,
                        local.get(&invalidation.core).and_then(Weak::upgrade),
                    )
                })
            };
            let Some((invalidation, core)) = next else {
                break;
            };
            if let Some(core) = core {
                core.cpu
                    .invalidate_code(Some((invalidation.address, invalidation.address + 4096)))?;
            }
            let mut s = self.state();
            if !s
                .invalidations
                .iter()
                .any(|queued| queued.backing == invalidation.backing)
            {
                if let Some(page) = s.pages.get_mut(&invalidation.backing) {
                    page.invalid = false;
                }
                s.dirty_pages.retain(|page| *page != invalidation.backing);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopped_mapping_barrier_accepts_parked_cores_only() {
        let shared = SharedMemory::default();
        let a = shared.processor();
        let b = shared.processor();
        let _a_run = a
            .reserve_run(1, ExecutionPolicy::Interactive, true)
            .unwrap();
        let _b_run = b
            .reserve_run(2, ExecutionPolicy::Interactive, true)
            .unwrap();
        a.start(1).unwrap();
        b.start(2).unwrap();

        a.set_parked(1, true).unwrap();
        assert!(a.require_stopped().is_err());
        b.set_parked(2, true).unwrap();
        assert!(a.require_stopped().is_ok());

        // A worker may invalidate its own stopped translation cache while a
        // peer is still executing; shared mapping changes still need a barrier.
        b.set_parked(2, false).unwrap();
        assert!(a.require_stopped().is_err());
        assert!(a.flush().is_ok());

        a.end();
        b.end();
        assert!(a.require_stopped().is_ok());
    }

    #[test]
    fn callback_write_requires_every_peer_to_be_parked() {
        let shared = SharedMemory::default();
        let a = shared.processor();
        let b = shared.processor();
        let _a_run = a
            .reserve_run(1, ExecutionPolicy::Interactive, true)
            .unwrap();
        let _b_run = b
            .reserve_run(2, ExecutionPolicy::Interactive, true)
            .unwrap();
        a.start(1).unwrap();
        b.start(2).unwrap();

        // A synchronous callback can invalidate the source CPU's local cache
        // while it is active, once every other CPU has reached the barrier.
        b.set_parked(2, true).unwrap();
        a.prepare_external_write().unwrap();
        b.set_parked(2, false).unwrap();
        assert!(a.prepare_external_write().is_err());

        a.end();
        b.end();
    }

    #[test]
    fn owner_barrier_lease_holds_parked_peers_until_the_run_ends() {
        let shared = SharedMemory::default();
        let a = shared.processor();
        let b = shared.processor();
        let _a_run = a
            .reserve_run(1, ExecutionPolicy::Interactive, true)
            .unwrap();
        let _b_run = b
            .reserve_run(2, ExecutionPolicy::Interactive, true)
            .unwrap();
        a.start(1).unwrap();
        b.start(2).unwrap();
        a.set_parked(1, true).unwrap();
        assert!(a.acquire_owner_barrier(1).is_none());
        b.set_parked(2, true).unwrap();
        {
            let mut state = shared.state.lock().unwrap();
            state.cores = vec![false, true, true];
        }

        let lease = a
            .acquire_owner_barrier(1)
            .expect("all processors are parked");
        assert!(
            b.reserve_run(3, ExecutionPolicy::Interactive, true)
                .err()
                .unwrap()
                .contains("owner synchronization barrier")
        );
        assert!(
            b.set_parked(2, false)
                .unwrap_err()
                .contains("cannot resume a peer")
        );
        assert_eq!(shared.state.lock().unwrap().parked.get(&2), Some(&1));

        drop(lease);
        b.set_parked(2, false).unwrap();
        assert_eq!(shared.state.lock().unwrap().parked.get(&2), None);
        drop(_b_run);
        let next_b_run = b
            .reserve_run(2, ExecutionPolicy::Interactive, true)
            .expect("a released barrier permits a new peer run");
        drop(next_b_run);
        drop(_a_run);
    }

    #[test]
    fn overlapping_runs_require_interactive_policy_on_every_core() {
        let shared = SharedMemory::default();
        let a = shared.processor();
        let b = shared.processor();

        let a_exact = a.reserve_run(1, ExecutionPolicy::Exact, true).unwrap();
        assert!(
            b.reserve_run(2, ExecutionPolicy::Exact, true)
                .err()
                .unwrap()
                .contains("require interactive execution policy")
        );
        drop(a_exact);

        let a_interactive = a
            .reserve_run(1, ExecutionPolicy::Interactive, true)
            .unwrap();
        assert!(
            b.reserve_run(2, ExecutionPolicy::Exact, true)
                .err()
                .unwrap()
                .contains("require interactive execution policy")
        );
        let b_interactive = b
            .reserve_run(2, ExecutionPolicy::Interactive, true)
            .unwrap();
        a.start(1).unwrap();
        b.start(2).unwrap();
        assert!(a.require_stopped().is_err());
        a.end();
        b.end();
        assert!(a.require_stopped().is_ok());
        drop(b_interactive);
        drop(a_interactive);
    }

    #[test]
    fn overlapping_interactive_runs_require_parallel_tcg_on_every_core() {
        let shared = SharedMemory::default();
        let memory = shared.processor();

        let serialized = memory
            .reserve_run(1, ExecutionPolicy::Interactive, false)
            .unwrap();
        assert!(matches!(
            memory.reserve_run(2, ExecutionPolicy::Interactive, true),
            Err(error) if error.contains("parallel TCG on every processor")
        ));
        drop(serialized);

        let parallel = memory
            .reserve_run(1, ExecutionPolicy::Interactive, true)
            .unwrap();
        assert!(matches!(
            memory.reserve_run(2, ExecutionPolicy::Interactive, false),
            Err(error) if error.contains("parallel TCG on every processor")
        ));
        drop(parallel);

        let first = memory
            .reserve_run(1, ExecutionPolicy::Interactive, true)
            .unwrap();
        let second = memory
            .reserve_run(2, ExecutionPolicy::Interactive, true)
            .unwrap();
        drop(second);
        drop(first);
    }
}
