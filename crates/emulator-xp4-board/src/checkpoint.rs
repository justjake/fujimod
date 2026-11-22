//! Portable XP4 application state at a stopped whole-cluster boundary.
use super::*;
use fujimod_emulator_cpu::memory::pages::{Image, Tracker};
use fujimod_emulator_devices::checkpoint::Checkpoint;
use serde::{Deserialize, Serialize};

const KIND: &str = "xp4-application-snapshot-v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    identity: String,
    instructions: u64,
    ticks: u64,
    tick_phase: u64,
    device_scan_at: u64,
    per_core: Vec<u64>,
    waiting: Vec<u8>,
    events: Vec<bool>,
    recent: Vec<VecDeque<(u32, u32)>>,
    milestones: BTreeMap<(usize, u32), u64>,
    cbar_reads: u64,
    gic: Gic,
    timer: Option<timer::Timer>,
    irq_entries: Vec<u64>,
    undefined_entries: Vec<u64>,
    prefetch_aborts: Vec<u64>,
    data_aborts: Vec<u64>,
    calls: Vec<Call>,
    pending_calls: Vec<BTreeMap<u32, Vec<usize>>>,
    observe_filter: Vec<u64>,
    exclusive: Vec<Option<(u64, usize)>>,
    cache_dirty: bool,
    pmu: Vec<Option<fujimod_emulator::board::armv7a::Profile>>,
    low_alias: Option<usize>,
    remap_pending: bool,
    remap_ready: bool,
    quiescent: bool,
    next_core: usize,
    memory: Vec<(u64, usize, u32)>,
}
impl Header {
    fn capture(board: &Session, identity: String) -> Self {
        let state = unsafe { &*board.inner.state.get() };
        Self {
            identity, instructions: state.instructions, ticks: state.ticks, tick_phase: state.tick_phase,
            device_scan_at: state.device_scan_at, per_core: state.per_core.clone(),
            waiting: state.waiting.clone(), events: state.events.clone(), recent: state.recent.clone(),
            milestones: state.milestones.clone(), cbar_reads: state.cbar_reads, gic: state.gic.clone(),
            timer: state.timer.clone(), irq_entries: state.irq_entries.clone(),
            undefined_entries: state.undefined_entries.clone(), prefetch_aborts: state.prefetch_aborts.clone(),
            data_aborts: state.data_aborts.clone(), calls: state.calls.clone(),
            pending_calls: state.pending_calls.clone(), observe_filter: state.observe_filter.clone(),
            exclusive: state.exclusive.clone(), cache_dirty: state.cache_dirty, pmu: state.pmu.clone(),
            low_alias: state.low_alias, remap_pending: state.remap_pending, remap_ready: state.remap_ready,
            next_core: board.next_core.get(), quiescent: board.quiescent.get(),
            memory: board.memory_geometry(),
        }
    }

    fn validate(&self, board: &Session, identity: &str) -> Result<(), String> {
        let live = unsafe { &*board.inner.state.get() };
        let cores = board.inner.cpus.len();
        if self.identity != identity || self.memory != board.memory_geometry() || self.next_core >= cores
            || [self.per_core.len(), self.waiting.len(), self.events.len(), self.recent.len(), self.irq_entries.len(),
                self.undefined_entries.len(), self.prefetch_aborts.len(), self.data_aborts.len(),
                self.pending_calls.len(), self.exclusive.len(), self.pmu.len()].into_iter().any(|n| n != cores)
            || self.per_core.iter().try_fold(0u64, |sum, &n| sum.checked_add(n)) != Some(self.instructions)
            || self.waiting.iter().any(|state| *state > 2)
            || self.recent.iter().any(|entries| entries.len() > 32)
            || self.observe_filter.len() != live.observe_filter.len()
            || self.calls.len() > 4096
            || self.calls.iter().any(|call| call.core >= cores || call.args.len() > 10)
            || self.pending_calls.iter().flatten().flat_map(|(_, indices)| indices).any(|&index| index >= self.calls.len())
            || self.low_alias != live.low_alias
            || self.timer.as_ref().map(std::mem::discriminant) != live.timer.as_ref().map(std::mem::discriminant)
            || self.pmu.iter().zip(&live.pmu).any(|(saved, live)| saved.as_ref().map(|p| &p.config) != live.as_ref().map(|p| &p.config))
        {
            return Err("XP4 snapshot execution wiring or state differs".into());
        }
        board.inner.instruction_rate.budget(0, self.tick_phase, 1)?;
        live.gic.validate_state(&self.gic)
    }

    fn restore(self, board: &Session) {
        let state = unsafe { &mut *board.inner.state.get() };
        state.instructions = self.instructions;
        state.ticks = self.ticks;
        state.tick_phase = self.tick_phase;
        state.device_scan_at = self.device_scan_at;
        state.per_core = self.per_core;
        state.waiting = self.waiting;
        state.events = self.events;
        state.recent = self.recent;
        state.milestones = self.milestones;
        state.cbar_reads = self.cbar_reads;
        state.gic = self.gic;
        state.timer = self.timer;
        state.irq_entries = self.irq_entries;
        state.undefined_entries = self.undefined_entries;
        state.prefetch_aborts = self.prefetch_aborts;
        state.data_aborts = self.data_aborts;
        state.calls = self.calls;
        state.pending_calls = self.pending_calls;
        state.observe_filter = self.observe_filter;
        state.exclusive = self.exclusive;
        state.cache_dirty = self.cache_dirty;
        state.dirty_pages.clear();
        state.dirty_all = self.cache_dirty;
        state.pmu = self.pmu;
        state.remap_pending = self.remap_pending;
        state.remap_ready = self.remap_ready;
        board.next_core.set(self.next_core);
        board.quiescent.set(self.quiescent);
    }
}

impl Session {
    fn memory_geometry(&self) -> Vec<(u64, usize, u32)> {
        std::iter::once((0, self.inner.ram.len(), 7)).chain(
            self.inner.memory.iter().map(|window| (window.base, window.data.len(), window.permissions))
        ).collect()
    }

    fn snapshot_backings(&self) -> impl Iterator<Item = &Backing> {
        std::iter::once(&self.inner.ram).chain(self.inner.memory.iter().map(|window| &window.data))
    }

    fn snapshot_boundary(&self) -> Result<(), String> {
        self.inner.host_mutation()?;
        self.inner.maintenance.ready(&self.inner.cpus)?;
        let state = unsafe { &*self.inner.state.get() };
        if self.inner.counted || state.discovery.enabled || state.counted_core.is_some()
            || state.failure.is_some() || state.redirected.is_some() || state.driver_guard.is_some()
            || state.recorder.is_some() || state.memory_watch.is_some() || state.pc_trace.is_some()
            || state.pc_observers.iter().any(|observer| !observer.is_empty()) || !state.xip.is_empty()
        {
            return Err("XP4 portable snapshots require a stopped strict exact board without diagnostic hooks or live flash execution".into());
        }
        Ok(())
    }

    /// Save an exact XP4 application boundary under the supplied boot plan identity.
    pub fn export_snapshot(&self, identity: &str) -> Result<Vec<u8>, String> {
        self.snapshot_boundary()?;
        let state = unsafe { &*self.inner.state.get() };
        let devices = state.devices.as_ref().map(devices::Devices::checkpoint).transpose()?;
        let processors = self.inner.maintenance.portable(&self.inner.cpus)?;
        let memory = self.snapshot_backings().map(|backing| {
            let tracker = Tracker::new(backing.len());
            unsafe { tracker.capture_all(backing.as_ptr()) }.to_bytes()
        }).collect::<Result<Vec<_>, _>>()?;
        Checkpoint::encode(KIND, &Header::capture(self, identity.into()))?
            .with_children(devices.into_iter().collect())
            .with_blobs(processors.into_iter().chain(memory).map(std::sync::Arc::from).collect())
            .to_bytes()
    }

    /// Restore onto a fresh board built from the same boot plan.
    pub fn restore_snapshot(&mut self, identity: &str, bytes: &[u8]) -> Result<(), String> {
        self.snapshot_boundary()?;
        let state = unsafe { &*self.inner.state.get() };
        if state.instructions != 0 || state.ticks != 0 {
            return Err("XP4 snapshot restore requires a fresh board".into());
        }
        let saved = Checkpoint::from_bytes(bytes)?;
        let header: Header = saved.decode(KIND)?;
        header.validate(self, identity)?;
        let cores = self.inner.cpus.len();
        if saved.blobs().len() != cores + header.memory.len()
            || saved.children().len() != usize::from(state.devices.is_some())
        {
            return Err("XP4 snapshot component counts differ".into());
        }
        if let Some((device, data)) = state.devices.as_ref().zip(saved.children().first()) {
            device.validate_checkpoint(data)?;
        }
        let processors = self.inner.maintenance.import_portable(&self.inner.cpus, &saved.blobs()[..cores])?;
        let memory = saved.blobs()[cores..].iter().zip(&header.memory)
            .map(|(data, (_, bytes, _))| Image::from_bytes(data, *bytes)).collect::<Result<Vec<_>, _>>()?;
        let result = (|| {
            for (backing, image) in self.snapshot_backings().zip(memory) {
                unsafe { Tracker::new(backing.len()).restore(&image, backing.as_ptr()) }?;
            }
            self.inner.maintenance.restore_portable(&self.inner.cpus, &processors)?;
            let state = unsafe { &mut *self.inner.state.get() };
            if let Some((device, data)) = state.devices.as_mut().zip(saved.children().first()) {
                device.restore_checkpoint(data)?;
            }
            header.restore(self);
            // Restored NOR bytes replace the executable window outside a device boundary.
            if let Some(view) = &self.inner.executable_nor
                && view.dirty()
            {
                for cpu in &self.inner.cpus {
                    cpu.invalidate_code(None)?;
                }
                unsafe { view.clear_dirty() };
            }
            Ok(())
        })();
        result.map_err(|error| self.inner.retain_failure(error))
    }
}
