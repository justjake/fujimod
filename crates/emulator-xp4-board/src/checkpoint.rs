//! In-process checkpoint and restore of a whole ARMv7-A cluster.
//!
//! A `Checkpoint` lives in memory and is restored into the same board. It has
//! no byte encoding. Every processor is an attached translation owner, so the
//! processors are captured with the owner-aware architectural snapshot of the
//! CPU crate. Memory, the interrupt controller, the timer, the counters and the
//! device table are copied. Devices without an exact checkpoint use their
//! observational JSON (see `devices::DevicesCheckpoint`); `exact()` says
//! whether any did.
//!
//! What is wiring and is not restored: the instruction observer callback,
//! instruction points, the lazily mapped MMIO pages and the frozen NOR view. The
//! translation-maintenance diagnostics are not restored either: their sequence
//! counters mirror the native queue, which is not rewound.
use super::*;
use crate::devices::DevicesCheckpoint;
use fujimod_emulator::board::armv7a::Profile;

const PAGE: usize = 4096;

/// The bytes of one memory backing, without its all-zero pages.
struct Pages {
    len: usize,
    pages: Vec<(usize, Box<[u8]>)>,
}
impl Pages {
    fn capture(backing: &Backing) -> Self {
        let bytes = unsafe { std::slice::from_raw_parts(backing.as_ptr(), backing.len()) };
        Self {
            len: bytes.len(),
            pages: bytes
                .chunks(PAGE)
                .enumerate()
                .filter(|(_, chunk)| chunk.iter().any(|&byte| byte != 0))
                .map(|(index, chunk)| (index, chunk.to_vec().into_boxed_slice()))
                .collect(),
        }
    }
    fn matches(&self, backing: &Backing) -> bool {
        self.len == backing.len()
    }
    fn restore(&self, backing: &Backing) {
        let bytes = unsafe { std::slice::from_raw_parts_mut(backing.as_ptr(), backing.len()) };
        let mut saved = self.pages.iter().peekable();
        for (index, chunk) in bytes.chunks_mut(PAGE).enumerate() {
            match saved.next_if(|(page, _)| *page == index) {
                Some((_, data)) => {
                    if *chunk != **data {
                        chunk.copy_from_slice(data);
                    }
                }
                None => {
                    if chunk.iter().any(|&byte| byte != 0) {
                        chunk.fill(0);
                    }
                }
            }
        }
    }
    fn bytes(&self) -> usize {
        self.pages.iter().map(|(_, page)| page.len()).sum()
    }
}

/// Plain counters, logs and flags of `RunState` that are machine or run state.
#[derive(Clone)]
struct Saved {
    instructions: u64,
    ticks: u64,
    tick_phase: u64,
    per_core: Vec<u64>,
    waiting: Vec<u8>,
    events: Vec<bool>,
    recent: Vec<VecDeque<(u32, u32)>>,
    milestones: BTreeMap<(usize, u32), u64>,
    discovery: Discovery,
    cbar_reads: u64,
    irq_entries: Vec<u64>,
    undefined_entries: Vec<u64>,
    prefetch_aborts: Vec<u64>,
    data_aborts: Vec<u64>,
    calls: Vec<Call>,
    pending_calls: Vec<BTreeMap<u32, Vec<usize>>>,
    exclusive: Vec<Option<(u64, usize)>>,
    cache_dirty: bool,
    pmu: Vec<Option<Profile>>,
    low_alias: Option<usize>,
    remap_pending: bool,
    remap_ready: bool,
}
impl Saved {
    /// Every field of `RunState` is named, so a new field forces a decision
    /// between saving it and listing it as wiring or host state.
    fn capture(state: &RunState) -> Self {
        let RunState {
            instructions,
            ticks,
            tick_phase,
            device_scan_at: _, // rescanned after a restore
            per_core,
            waiting,
            events,
            recent,
            milestones,
            discovery,
            failure: _, // a checkpoint exists only for a healthy board
            redirected: _,
            pages: _, // lazily mapped MMIO pages are wiring
            cbar_reads,
            gic: _, // kept in the checkpoint beside the state
            timer: _,
            irq_entries,
            undefined_entries,
            prefetch_aborts,
            data_aborts,
            calls,
            pending_calls,
            exclusive,
            recorder: _, // host instrumentation: a checkpoint refuses it
            memory_watch: _,
            pc_observers: _,
            devices: _, // kept in the checkpoint beside the state
            cache_dirty,
            pmu,
            low_alias,
            remap_pending,
            remap_ready,
            driver_guard: _,
        } = state;
        Self {
            instructions: *instructions,
            ticks: *ticks,
            tick_phase: *tick_phase,
            per_core: per_core.clone(),
            waiting: waiting.clone(),
            events: events.clone(),
            recent: recent.clone(),
            milestones: milestones.clone(),
            discovery: discovery.clone(),
            cbar_reads: *cbar_reads,
            irq_entries: irq_entries.clone(),
            undefined_entries: undefined_entries.clone(),
            prefetch_aborts: prefetch_aborts.clone(),
            data_aborts: data_aborts.clone(),
            calls: calls.clone(),
            pending_calls: pending_calls.clone(),
            exclusive: exclusive.clone(),
            cache_dirty: *cache_dirty,
            pmu: pmu.clone(),
            low_alias: *low_alias,
            remap_pending: *remap_pending,
            remap_ready: *remap_ready,
        }
    }
    fn apply(self, state: &mut RunState) {
        let Saved {
            instructions,
            ticks,
            tick_phase,
            per_core,
            waiting,
            events,
            recent,
            milestones,
            discovery,
            cbar_reads,
            irq_entries,
            undefined_entries,
            prefetch_aborts,
            data_aborts,
            calls,
            pending_calls,
            exclusive,
            cache_dirty,
            pmu,
            low_alias,
            remap_pending,
            remap_ready,
        } = self;
        state.instructions = instructions;
        state.ticks = ticks;
        state.tick_phase = tick_phase;
        state.per_core = per_core;
        state.waiting = waiting;
        state.events = events;
        state.recent = recent;
        state.milestones = milestones;
        state.discovery = discovery;
        state.cbar_reads = cbar_reads;
        state.irq_entries = irq_entries;
        state.undefined_entries = undefined_entries;
        state.prefetch_aborts = prefetch_aborts;
        state.data_aborts = data_aborts;
        state.calls = calls;
        state.pending_calls = pending_calls;
        state.exclusive = exclusive;
        state.cache_dirty = cache_dirty;
        state.pmu = pmu;
        state.low_alias = low_alias;
        state.remap_pending = remap_pending;
        state.remap_ready = remap_ready;
        // Devices are rescanned on the next instruction; a failure of a later
        // run is not part of a healthy checkpoint.
        state.device_scan_at = 0;
        state.failure = None;
        state.redirected = None;
    }
}

/// The shape of the board a checkpoint was taken from. The definition text is
/// not kept by a session, so the restore compares what it can see.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Shape {
    cores: usize,
    ram: usize,
    windows: Vec<(u64, usize, u32)>,
    dram_alias: Option<u64>,
    frozen_nor: Option<usize>,
    timer: Option<bool>,
    devices: bool,
}

/// An in-memory checkpoint of one cluster. Not portable: no byte encoding.
pub struct Checkpoint {
    shape: Shape,
    cpus: Vec<Box<dyn cpu::Snapshot>>,
    ram: Pages,
    windows: Vec<Pages>,
    state: Saved,
    gic: Gic,
    timer: Option<timer::Timer>,
    devices: Option<DevicesCheckpoint>,
    next_core: usize,
}
impl Checkpoint {
    pub fn ticks(&self) -> u64 {
        self.state.ticks
    }
    pub fn instructions(&self) -> u64 {
        self.state.instructions
    }
    pub fn per_core(&self) -> &[u64] {
        &self.state.per_core
    }
    /// Bytes of guest memory held by the checkpoint (non-zero pages only).
    pub fn memory_bytes(&self) -> usize {
        self.ram.bytes() + self.windows.iter().map(Pages::bytes).sum::<usize>()
    }
    /// Devices captured through the interim observational JSON.
    pub fn observational_devices(&self) -> Vec<&'static str> {
        self.devices
            .as_ref()
            .map(DevicesCheckpoint::observational)
            .unwrap_or_default()
    }
    /// Devices captured through their exact checkpoint path.
    pub fn exact_devices(&self) -> usize {
        self.devices.as_ref().map_or(0, DevicesCheckpoint::exact)
    }
    /// True when every device used its exact checkpoint path.
    pub fn exact(&self) -> bool {
        self.observational_devices().is_empty()
    }
}

impl Cluster {
    fn shape(&self) -> Shape {
        let state = unsafe { &*self.state.get() };
        Shape {
            cores: self.cpus.len(),
            ram: self.ram.len(),
            windows: self
                .memory
                .iter()
                .map(|window| (window.base, window.data.len(), window.permissions))
                .collect(),
            dram_alias: self.dram_alias,
            frozen_nor: self.frozen_nor.as_ref().map(|view| view.len()),
            timer: state.timer.as_ref().map(|timer| matches!(timer, timer::Timer::Xp4(_))),
            devices: state.devices.is_some(),
        }
    }

    /// Instrumentation that holds host state outside the machine.
    fn checkpoint_instrumentation(&self) -> Result<(), String> {
        let state = unsafe { &*self.state.get() };
        if state.recorder.is_some() {
            return Err("Checkpoint requires the function watch to be ended".into());
        }
        if state.memory_watch.is_some() {
            return Err("Checkpoint requires the memory watch to be ended".into());
        }
        if state.driver_guard.is_some() {
            return Err("Checkpoint requires the guarded routine to be ended".into());
        }
        Ok(())
    }

    /// Move the low-memory window to `target`, as the guest's remap does.
    fn set_low_alias(&self, target: Option<usize>) -> Result<(), String> {
        let state = unsafe { &mut *self.state.get() };
        match (state.low_alias, target) {
            (current, wanted) if current == wanted => Ok(()),
            (Some(index), None) => {
                let low = self.memory[index].data.len();
                for cpu in &self.cpus {
                    cpu.unmap(0, low)?;
                    unsafe {
                        cpu.map_backing(0, low, 7, NonNull::new(self.ram.as_ptr()).unwrap())
                    }?;
                }
                state.low_alias = None;
                Ok(())
            }
            (None, Some(index)) => {
                let window = &self.memory[index];
                for cpu in &self.cpus {
                    cpu.unmap(0, window.data.len())?;
                    unsafe {
                        cpu.map_backing(
                            0,
                            window.data.len(),
                            window.permissions,
                            NonNull::new(window.data.as_ptr()).unwrap(),
                        )
                    }?;
                }
                state.low_alias = Some(index);
                Ok(())
            }
            _ => Err("Low-memory alias differs from the checkpoint".into()),
        }
    }

    fn take_checkpoint(&self, next_core: usize) -> Result<Checkpoint, String> {
        self.host_mutation()?;
        self.checkpoint_instrumentation()?;
        if unsafe { (*self.state.get()).redirected.is_some() } {
            return Err("Checkpoint requires a settled instruction redirect".into());
        }
        let mut cpus = Vec::with_capacity(self.cpus.len());
        for (index, cpu) in self.cpus.iter().enumerate() {
            cpus.push(cpu.snapshot_owned(self.maintenance.owner(index)?, None)?);
        }
        let state = unsafe { &*self.state.get() };
        let devices = state
            .devices
            .as_ref()
            .map(devices::Devices::checkpoint)
            .transpose()?;
        Ok(Checkpoint {
            shape: self.shape(),
            cpus,
            ram: Pages::capture(&self.ram),
            windows: self.memory.iter().map(|w| Pages::capture(&w.data)).collect(),
            state: Saved::capture(state),
            gic: state.gic.clone(),
            timer: state.timer.clone(),
            devices,
            next_core,
        })
    }

    /// Everything that can be refused is refused here, before any mutation.
    fn validate_restore(&self, saved: &Checkpoint) -> Result<(), String> {
        if self.running.get() {
            return Err("Restore requires a paused ARMv7-A board".into());
        }
        self.maintenance
            .ready(&self.cpus)
            .map_err(|error| self.retain_failure(error))?;
        self.checkpoint_instrumentation()?;
        if saved.shape != self.shape() {
            return Err("Checkpoint belongs to a different board".into());
        }
        if saved.cpus.len() != self.cpus.len() {
            return Err("Checkpoint holds a different number of processors".into());
        }
        let state = unsafe { &*self.state.get() };
        if !saved.ram.matches(&self.ram)
            || saved.windows.len() != self.memory.len()
            || saved
                .windows
                .iter()
                .zip(&self.memory)
                .any(|(pages, window)| !pages.matches(&window.data))
        {
            return Err("Checkpoint memory differs from the board".into());
        }
        for (now, then) in state.pmu.iter().zip(&saved.state.pmu) {
            if now.as_ref().map(|profile| &profile.config)
                != then.as_ref().map(|profile| &profile.config)
            {
                return Err("Checkpoint performance-counter setup differs".into());
            }
        }
        let mut gic = state.gic.clone();
        gic.restore(&saved.gic.save())?;
        if let (Some(devices), Some(saved)) = (&state.devices, &saved.devices) {
            devices.validate_checkpoint(saved)?;
        }
        Ok(())
    }

    fn apply_restore(&self, saved: &Checkpoint) -> Result<(), String> {
        self.set_low_alias(saved.state.low_alias)?;
        saved.ram.restore(&self.ram);
        for (pages, window) in saved.windows.iter().zip(&self.memory) {
            pages.restore(&window.data);
        }
        let state = unsafe { &mut *self.state.get() };
        if let (Some(devices), Some(saved)) = (state.devices.as_mut(), &saved.devices) {
            devices.restore_checkpoint(saved)?;
        }
        state.gic = saved.gic.clone();
        state.timer = saved.timer.clone();
        saved.state.clone().apply(state);
        // The saved counters are the truth now; the processors follow.
        for (index, (cpu, snapshot)) in self.cpus.iter().zip(&saved.cpus).enumerate() {
            cpu.restore_owned(self.maintenance.owner(index)?, &**snapshot)?;
        }
        Ok(())
    }
}

impl Session {
    /// Capture the whole board in memory. The board must be paused, healthy,
    /// without pending translation maintenance, and without an active function
    /// watch, memory watch or guarded routine.
    pub fn checkpoint(&self) -> Result<Checkpoint, String> {
        self.inner.take_checkpoint(self.next_core.get())
    }

    /// Return the board to a checkpoint taken from it. Validation happens
    /// before the first change. An error after the first change retains a
    /// failure, so a half-restored board never runs. A board that retained a
    /// failure after the checkpoint is healthy again after a successful
    /// restore.
    pub fn restore(&self, saved: &Checkpoint) -> Result<(), String> {
        let c = &self.inner;
        c.validate_restore(saved)?;
        if let Err(error) = c.apply_restore(saved) {
            return Err(c.retain_failure(format!("Restore failed part way: {error}")));
        }
        // The failure, if any, was of the discarded future.
        unsafe {
            (*c.state.get()).failure = None;
        }
        self.next_core.set(saved.next_core);
        self.quiescent.set(false);
        Ok(())
    }
}
