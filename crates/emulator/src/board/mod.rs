//! Board execution and device scheduling above the CPU backend boundary.
pub mod aprofile;
pub mod armv7a;
pub mod bus;
mod callback_timing;
mod code_shadow;
mod mprofile_blocks;
mod thumb_accounting;
mod poll_loops;
#[cfg(test)]
mod mprofile_blocks_tests;
#[cfg(test)]
mod code_shadow_tests;
#[cfg(test)]
mod store_hook_tests;
#[cfg(test)]
mod settle_access_tests;
pub mod continuation;
pub mod decoder;
#[cfg(test)]
mod decoder_tests;
#[cfg(test)]
mod accounting_equivalence_tests;
#[cfg(test)]
mod restart_tests;
pub mod discovery;
pub mod factory;
pub mod harness;
mod host;
mod interactive;
pub mod keyframe;
mod loops;
pub mod memory;
#[cfg(test)]
mod memory_tests;
pub mod mprofile;
pub mod observation;
pub mod preservation;
#[cfg(test)]
mod observation_tests;
#[cfg(test)]
mod preservation_tests;
mod observe;
pub mod write_watch;
mod schedule;
mod state;
#[cfg(test)]
mod schedule_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod workers_tests;
use bus::Bus;
pub use fujimod_emulator_cpu::GuestProfilePhase;
use fujimod_emulator_cpu::timeline::{Ratio, Target, Timeline};
use fujimod_emulator_cpu::{
    Cpu, ExecutionPolicy, Exit, Hook, HookId, HookRange, Interrupt, Mmio, Register, Run,
};
use fujimod_emulator_devices::{Host, Record};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::{Rc, Weak};

pub use state::{BoardState, FrameState, ProcessorState};

mod interrupts;
pub use interrupts::{InterruptMap, InterruptRegisters};
struct CoreInterruptMap {
    device: usize,
    inputs: Box<dyn fujimod_emulator_devices::bus::CoreInterrupts>,
}
/// Whether the board observes every instruction entry of this processor
/// (`before`), instead of settling its entries in batches.
fn observes_every_instruction(cpu: &dyn Cpu) -> bool {
    let features = cpu.features();
    !(cpu.model() != fujimod_emulator_cpu::Model::CortexM4
        && features.architectural_exits
        && features.instruction_progress
        && features.tighten_instruction_limit)
}
fn device_workers(policy: ExecutionPolicy, preference: usize) -> usize {
    match policy {
        ExecutionPolicy::Exact => 0,
        ExecutionPolicy::Interactive => preference,
    }
}
fn prepare_device_workers(policy: ExecutionPolicy, preference: usize) -> Result<(), String> {
    let count = device_workers(policy, preference);
    if fujimod_emulator_devices::work::worker_count() != count {
        fujimod_emulator_devices::work::configure(count)?;
    }
    Ok(())
}

fn engine_run_boundary(
    timeline: &Timeline,
    instruction_limit: u64,
    until: Option<u64>,
    lag: u64,
) -> Result<fujimod_perf::EngineEndReason, String> {
    use fujimod_perf::EngineEndReason;

    let ratio = timeline.ratio();
    let now = timeline.now();
    let phase = timeline.phase();
    let event = timeline.next_event();
    let event_budget = event
        .map(|event| {
            ratio
                .budget(now, phase, event.at)
                .map(|budget| budget.saturating_add(lag))
        })
        .transpose()?
        .unwrap_or(u64::MAX);
    let until_budget = until
        .map(|until| {
            ratio
                .budget(now, phase, until)
                .map(|budget| budget.saturating_add(lag))
        })
        .transpose()?
        .unwrap_or(u64::MAX);

    if event_budget <= instruction_limit && event_budget <= until_budget {
        return Ok(match event.map(|event| event.target) {
            Some(Target::DeviceEvent(_) | Target::DeviceDeadline(_)) => {
                EngineEndReason::DeviceDeadline
            }
            _ => EngineEndReason::OtherExit,
        });
    }
    if instruction_limit <= until_budget {
        Ok(EngineEndReason::QuantumOrInstructionBudget)
    } else {
        Ok(EngineEndReason::OtherExit)
    }
}

fn engine_end_reason(
    exit: Option<Exit>,
    instructions: u64,
    instruction_limit: u64,
    boundary: fujimod_perf::EngineEndReason,
    watch_or_observer_stop: bool,
    failed: bool,
    interrupted_by_scheduler: bool,
) -> fujimod_perf::EngineEndReason {
    use fujimod_perf::EngineEndReason;

    if watch_or_observer_stop {
        return EngineEndReason::WatchOrObserverStop;
    }
    if !failed {
        match exit {
            Some(Exit::Stopped) if instructions >= instruction_limit => {
                return match boundary {
                    EngineEndReason::QuantumOrInstructionBudget
                    | EngineEndReason::DeviceDeadline => boundary,
                    _ => EngineEndReason::OtherExit,
                };
            }
            Some(Exit::Stopped) if !interrupted_by_scheduler => {
                return EngineEndReason::WatchOrObserverStop;
            }
            _ => (),
        }
    }
    EngineEndReason::OtherExit
}

#[derive(Clone, Copy)]
enum StoreObserverReason {
    ExplicitWriteWatchOverlap,
    MProfileAliasWrite,
    LiveExclusiveReservation,
    RestartAttempt,
}

#[derive(Clone, Default)]
struct StoreObserverCounters {
    callbacks_total: u64,
    completed_callbacks: u64,
    early_restart_attempts: u64,
    failed_before_classification: u64,
    callbacks_with_any_observed_reason: u64,
    callbacks_with_multiple_observed_reasons: u64,
    no_recorded_reason: u64,
    explicit_write_watch_overlaps: u64,
    mprofile_alias_writes: u64,
    code_shadow_store_checks: u64,
    live_exclusive_reservation_before_write: u64,
}

impl StoreObserverCounters {
    fn observe_reason(&mut self, reason: StoreObserverReason) {
        let counter = match reason {
            StoreObserverReason::ExplicitWriteWatchOverlap => {
                &mut self.explicit_write_watch_overlaps
            }
            StoreObserverReason::MProfileAliasWrite => &mut self.mprofile_alias_writes,
            StoreObserverReason::LiveExclusiveReservation => {
                &mut self.live_exclusive_reservation_before_write
            }
            StoreObserverReason::RestartAttempt => &mut self.early_restart_attempts,
        };
        *counter = counter.saturating_add(1);
    }

    fn finish_observation(&mut self, reasons: u8, complete: bool) {
        if reasons != 0 {
            self.callbacks_with_any_observed_reason =
                self.callbacks_with_any_observed_reason.saturating_add(1);
        }
        if reasons > 1 {
            self.callbacks_with_multiple_observed_reasons = self
                .callbacks_with_multiple_observed_reasons
                .saturating_add(1);
        }
        if complete {
            self.completed_callbacks = self.completed_callbacks.saturating_add(1);
            if reasons == 0 {
                self.no_recorded_reason = self.no_recorded_reason.saturating_add(1);
            }
        }
    }

    fn json(&self) -> Value {
        serde_json::json!({
            "callbacks_total": self.callbacks_total,
            "completed_callbacks": self.completed_callbacks,
            "early_restart_attempts": self.early_restart_attempts,
            "failed_before_classification": self.failed_before_classification,
            "callbacks_with_any_observed_reason": self.callbacks_with_any_observed_reason,
            "callbacks_with_multiple_observed_reasons": self.callbacks_with_multiple_observed_reasons,
            "no_recorded_reason": self.no_recorded_reason,
            "explicit_write_watch_overlaps": self.explicit_write_watch_overlaps,
            "mprofile_alias_writes": self.mprofile_alias_writes,
            "code_shadow_store_checks": self.code_shadow_store_checks,
            "live_exclusive_reservation_before_write": self.live_exclusive_reservation_before_write,
            "reason_counts_are_nonexclusive": true,
        })
    }
}

struct State {
    continuation: Rc<RefCell<continuation::Stops>>,
    host_pause: Rc<Cell<bool>>,
    execution_policy: ExecutionPolicy,
    // Host computation preference; absent from fingerprints and keyframes.
    device_workers: usize,
    // Host run boundary, not a device deadline or persistent modeled state.
    clock_limit: Option<u64>,
    power_off: bool,
    memory: Rc<memory::Memory>,
    bus: Bus,
    timeline: Timeline,
    interrupts: Option<InterruptMap>,
    core_interrupts: Vec<Option<CoreInterruptMap>>,
    interrupt_changed: Vec<bool>,
    released: Vec<bool>,
    release_state_changed: bool,
    waiting: Vec<bool>,
    /// Released cores that wait in WFI until an SGI is pending for them (see factory).
    parked_until_sgi: Vec<bool>,
    waiting_event: Vec<bool>,
    events: Vec<bool>,
    pins: Vec<(bool, bool)>,
    interrupt_delivery: Vec<bool>,
    instructions: Vec<u64>,
    epoch_instructions: Vec<u64>,
    accounted: Vec<u64>,
    deadlines: Vec<Option<u64>>,
    generations: Vec<u64>,
    error: Option<String>,
    records: Vec<Record>,
    irq_observations: Option<(usize, Vec<Value>)>,
    console: Vec<String>,
    frames: std::collections::BTreeMap<usize, fujimod_emulator_devices::graphics::scanout::Frame>,
    outputs: std::collections::BTreeMap<usize, Value>,
    mprofile: Vec<Option<mprofile::Profile>>,
    aprofile: Vec<Option<aprofile::Profile>>,
    armv7a: Vec<Option<armv7a::Profile>>,
    // Observational, outside keyframes: IRQ and disabled-VFP entries the
    // board performed on ARMv7-A cores.
    armv7a_entries: Vec<fujimod_emulator_cpu::armv7a::Entries>,
    entry_points: Vec<Vec<Weak<observation::EntryPoints>>>,
    implementation_cluster: aprofile::cortex_a53::Registers,
    unmodeled_system: std::collections::BTreeMap<(usize, u16, u64), (u64, u64)>,
    processor_deadlines: Vec<Option<u64>>,
    processor_generations: Vec<u64>,
    reset_contexts: Vec<Option<Box<dyn fujimod_emulator_cpu::Snapshot>>>,
    exclusive: fujimod_emulator_cpu::exclusive::Shared,
    // Consecutive resumes of this processor share unchanged external backing.
    // A peer run or host/DMA write requires resynchronizing Unicorn's code cache.
    code_run: Option<usize>,
    // Host-only: a device wrote RAM during the running slice, none of it RAM
    // the running M-profile processor maps. `code_run` is cleared when the
    // slice ends instead of at the write, so that processor's translations
    // and page copies, which the write cannot have touched, are kept.
    code_write_deferred: bool,
    // Host-only: proves an M-profile processor's translations still match
    // memory after a peer ran, so its code cache survives the peer's slice.
    code_shadow: Vec<Option<code_shadow::CodeShadow>>,
    // Host-only: block execution of M-profile processors.
    blocks: Vec<Option<mprofile_blocks::Blocks>>,
    // Host-only: entries the instruction observer saw in the current engine
    // run, as the engine's own counter counts them.
    run_entries: Vec<u64>,
    // Host-only: poll loop fast-forward of AArch64 processors.
    polls: poll_loops::Polls,
    // Host-only: lets an instruction observer skip interrupt bookkeeping
    // that provably has nothing to do.
    settled: Settled,
    active_slice: Option<schedule::Slice>,
    parallel: Option<interactive::Epoch>,
    decoders: decoder::Decoders,
    memory_access: bool,
    mprofile_loops: loops::MProfileLoops,
    access_histogram: Option<access_histogram::SharedStatistics>,
    store_observer: Option<Vec<StoreObserverCounters>>,
    callback_service_time: Option<callback_timing::SharedStatistics>,
}
/// Host-only record that pushing interrupt pins and refreshing processor
/// deadlines is a fixed point.
///
/// An instruction observer repeats both before every instruction. Between two
/// instructions of one run, their inputs change only through a device event,
/// an MMIO or system access, an architectural entry the observer itself
/// performs, or guest time reaching a generic timer compare. Every such
/// change clears this record; while it stands, both calls would recompute the
/// values already stored, so the observer skips them.
struct Settled {
    /// The last full pin push left every value as it found it, so repeating
    /// it at the same timer outputs changes nothing.
    pass: bool,
    /// Both calls are a fixed point until guest time reaches `until`.
    armed: bool,
    until: u64,
    /// While armed: the M-profile processor whose observer armed this found
    /// no exception ready. Its pending scan finds the same until this record
    /// is cleared.
    idle: Option<usize>,
    /// Recompute every `verify`th skipped call and refuse any difference;
    /// zero never verifies. Debug builds verify every skip.
    verify: u32,
    skipped: u32,
    /// Observer entries that skipped, and that ran, the bookkeeping.
    skips: u64,
    full: u64,
}
impl Settled {
    fn new() -> Self {
        Self {
            pass: false,
            armed: false,
            until: 0,
            idle: None,
            verify: std::env::var("FUJIMOD_VERIFY_SETTLED")
                .ok()
                .and_then(|period| period.parse().ok())
                .unwrap_or(u32::from(cfg!(debug_assertions))),
            skipped: 0,
            skips: 0,
            full: 0,
        }
    }
    #[inline]
    fn clear(&mut self) {
        self.pass = false;
        self.armed = false;
        self.idle = None;
    }
    /// Whether to recompute this skipped call.
    #[inline]
    fn verifies(&mut self) -> bool {
        if self.verify == 0 {
            return false;
        }
        self.skipped = self.skipped.wrapping_add(1);
        self.skipped.is_multiple_of(self.verify)
    }
}
struct Binding {
    state: Rc<RefCell<State>>,
    callback_service_time: Option<callback_timing::SharedStatistics>,
    cpus: Weak<Vec<Rc<dyn Cpu>>>,
    mmio_callback_counts: Rc<RefCell<Vec<u64>>>,
    core: usize,
    base: u64,
    // This backend's clock is owned by the ordered board. Its descriptors
    // are evaluated after the access reaches the bus timeline.
    published_reads: Vec<fujimod_emulator_cpu::FastMmioRead>,
    published_hits: Cell<u64>,
}

fn callback_service_sample(
    binding: &Binding,
    cpus: &[Rc<dyn Cpu>],
    kind: callback_timing::CallbackKind,
) -> Option<callback_timing::SampledCallback> {
    let statistics = binding.callback_service_time.as_ref()?;
    if cpus[binding.core].model() != fujimod_emulator_cpu::Model::CortexM4 {
        return None;
    }
    callback_timing::count(statistics, binding.core, kind)
}

/// Devices and callbacks have one owner. Interactive native processors run
/// concurrently between barriers; exact execution uses ordered slices.
pub struct Board {
    cpus: Rc<Vec<Rc<dyn Cpu>>>,
    state: Rc<RefCell<State>>,
    access_histogram: Option<access_histogram::SharedStatistics>,
    run_wakeup: fujimod_emulator_cpu::RunWakeup,
    // Native hooks hold raw pointers into each box; boxing keeps them stable as the Vec grows.
    #[allow(clippy::vec_box)]
    bindings: Vec<Box<Binding>>,
    hooks: Vec<(usize, HookId)>,
    /// The owner of every hook the execution core holds or may hold for
    /// itself, in construction order: the synchronization barrier, the
    /// instruction observer and the board store hook. Which of them are
    /// installed at an instant, and whether block execution replaced the
    /// observer, is a host choice; snapshots and keyframes bind to this list
    /// and to the hooks other owners installed (`hook_wiring`).
    execution_hooks: Vec<usize>,
    /// Per core, the synchronization barrier's hook.
    barrier_hooks: Vec<Option<HookId>>,
    /// Per core, the instruction observer's hook (`before`), when installed.
    observer_hooks: Vec<Option<HookId>>,
    /// Per core, the hooks that block execution installed.
    block_hooks: Vec<Vec<HookId>>,
    /// Per core, the board store hook (`written`) when it is always installed.
    store_hooks: Vec<Option<HookId>>,
    /// Host run control: M-profile processors may run in block execution.
    /// Off unless the board's owner asks for it (`set_block_execution`): a
    /// processor in block execution has no instruction observer, so the
    /// M-profile loop windows (`loops`) do not run for it.
    block_execution: Cell<bool>,
    /// Host run control: poll loops of M-profile processors in block
    /// execution are fast-forwarded. `FUJIMOD_POLL_LOOPS=0` starts a board
    /// with it off.
    poll_loops: Cell<bool>,
    /// Per core: its board store hook is installed only while it has work,
    /// and the installed hook.
    store_hooks_on_demand: Vec<Option<Option<HookId>>>,
    /// Host-only: how often an on-demand store hook was installed.
    store_hook_installs: Cell<u64>,
    /// Native MMIO maps as (core, address, bytes, physical base).
    mmio: Vec<(usize, u64, usize, u64)>,
    mmio_callback_counts: Rc<RefCell<Vec<u64>>>,
    interactive_windows: Cell<u64>,
    interactive_host_duration_ns: Cell<u128>,
    next_core: Cell<usize>,
    slice_left: Cell<usize>,
    quantum: Cell<Option<usize>>,
    // CPUs must be destroyed before the VM, and the VM before borrowed RAM.
    retained_engines: Option<factory::Engines>,
    physical_memory: Rc<memory::Memory>,
    host_pause: Rc<Cell<bool>>,
    // Passive host reporting; excluded from modeled state and checkpoints.
    progress_observer: Option<Box<dyn FnMut(u64)>>,
    guest_profile_active: Cell<bool>,
    isolated_core: Cell<Option<usize>>,
    // Host run control and its outcome; excluded from modeled state.
    idle_stop: Cell<bool>,
    idle_stopped: Cell<bool>,
    continuation: Option<Box<continuation::StoreWatch>>,
    /// Processor address views of RAM, from the checked graph.
    mappings: Vec<factory::MemoryMap>,
    read_set: RefCell<Option<continuation::ReadSet>>,
    // Native hooks hold raw pointers into each box; boxing keeps them stable as the Vec grows.
    #[allow(clippy::vec_box)]
    watches: Vec<Box<observe::Watch>>,
    entries: Rc<RefCell<observe::Entries>>,
    // Native hooks hold raw pointers into each box; boxing keeps them stable as the Vec grows.
    #[allow(clippy::vec_box)]
    discovery_stub_contexts: Vec<Box<discovery::CalleeStubContext>>,
    translation_cache_size_bytes: u32,
}
impl Board {
    /// Per processor translation cache size selected when the board was built.
    pub fn translation_cache_size_bytes(&self) -> u32 {
        self.translation_cache_size_bytes
    }
    /// Host parallelism used for Interactive device computations. Defaults to
    /// zero on every target; Exact stays inline for any preference.
    pub fn device_worker_preference(&self) -> usize {
        self.state.borrow().device_workers
    }
    pub fn device_worker_count(&self) -> usize {
        device_workers(self.execution_policy(), self.device_worker_preference())
    }
    /// Take device work that paused guest execution for an external async host
    /// operation. Each request is paired with its owning device index.
    pub fn take_host_requests(
        &mut self,
    ) -> Result<Vec<(usize, fujimod_emulator_devices::bus::HostRequest)>, String> {
        let mut state = self
            .state
            .try_borrow_mut()
            .map_err(|_| "host requests require a stopped board")?;
        if state.active_slice.is_some() || state.parallel.is_some() || state.memory_access {
            return Err("host requests require a stopped board".into());
        }
        Ok(state.bus.take_host_requests())
    }
    /// Deliver an async host result to the device that published its request.
    pub fn complete_host_request(
        &mut self,
        device: usize,
        id: u64,
        result: &[u8],
    ) -> Result<(), String> {
        let mut state = self
            .state
            .try_borrow_mut()
            .map_err(|_| "host result requires a stopped board")?;
        if state.active_slice.is_some() || state.parallel.is_some() || state.memory_access {
            return Err("host result requires a stopped board".into());
        }
        state.with_host(&self.cpus, |bus, host| {
            bus.complete_host_request(host, device, id, result)
        })?;
        self.host_pause.set(false);
        state.refresh_deadlines()?;
        state.refresh_processors()
    }
    /// Drain derived work before changing a stopped owner's executor. Joining
    /// computation never publishes its DMA payload or interrupts.
    pub fn set_device_workers(&mut self, count: usize) -> Result<(), String> {
        if !matches!(count, 0 | 2) {
            return Err("device workers must be 0 or 2".into());
        }
        let mut state = self
            .state
            .try_borrow_mut()
            .map_err(|_| "device workers require a stopped board")?;
        if state.active_slice.is_some() || state.parallel.is_some() || state.memory_access {
            return Err("device workers require a stopped board".into());
        }
        prepare_device_workers(state.execution_policy, count)?;
        state.device_workers = count;
        Ok(())
    }
    pub fn execution_policy(&self) -> ExecutionPolicy {
        self.state.borrow().execution_policy
    }
    /// Change policy only at a whole-board pause. CPU snapshots retain their
    /// own policy; the board keyframe retains this scheduler selection too.
    pub fn set_execution_policy(&mut self, policy: ExecutionPolicy) -> Result<(), String> {
        let mut state = self
            .state
            .try_borrow_mut()
            .map_err(|_| "policy requires a stopped board")?;
        if state.active_slice.is_some() || state.parallel.is_some() || state.memory_access {
            return Err("policy requires a stopped board".into());
        }
        let previous = state.execution_policy;
        if previous == policy {
            return prepare_device_workers(policy, state.device_workers);
        }
        for (index, cpu) in self.cpus.iter().enumerate() {
            if let Err(error) = cpu.set_execution_policy(policy) {
                for cpu in &self.cpus[..index] {
                    cpu.set_execution_policy(previous)?;
                }
                return Err(error);
            }
        }
        if let Err(error) = prepare_device_workers(policy, state.device_workers) {
            for cpu in self.cpus.iter() {
                cpu.set_execution_policy(previous)?;
            }
            return Err(error);
        }
        state.execution_policy = policy;
        for shadow in state.code_shadow.iter_mut().flatten() {
            shadow.invalidate();
        }
        state.settled.clear();
        self.slice_left.set(0);
        Ok(())
    }
    pub fn new(
        cpus: Vec<Box<dyn Cpu>>,
        bus: Bus,
        ratio: Ratio,
        interrupts: Option<InterruptMap>,
        released: Vec<bool>,
        memory: memory::Memory,
    ) -> Result<Self, String> {
        Self::new_at(cpus, bus, ratio, interrupts, released, memory, 0, None)
    }

    /// Construct fresh processors and devices on a continuing host timeline.
    pub(crate) fn new_at(
        cpus: Vec<Box<dyn Cpu>>,
        bus: Bus,
        ratio: Ratio,
        interrupts: Option<InterruptMap>,
        released: Vec<bool>,
        memory: memory::Memory,
        start_tick: u64,
        previous_instructions: Option<Vec<u64>>,
    ) -> Result<Self, String> {
        if cpus.is_empty() || cpus.len() != released.len() || !released.iter().any(|r| *r) {
            return Err("invalid released processor set".into());
        }
        let instructions = previous_instructions.unwrap_or_else(|| vec![0; cpus.len()]);
        if instructions.len() != cpus.len()
            || instructions.iter().try_fold(0u64, |sum, n| sum.checked_add(*n)).is_none()
        {
            return Err("invalid prior processor instruction counts".into());
        }
        if let Some(g) = &interrupts {
            g.validate(cpus.len())?;
        }
        let epoch_instructions = instructions.clone();
        let n = cpus.len();
        let reset_contexts = cpus
            .iter()
            .map(|cpu| {
                if cpu.model() == fujimod_emulator_cpu::Model::CortexM4 {
                    cpu.snapshot(None).map(Some)
                } else {
                    Ok(None)
                }
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut domains = cpus.iter().filter_map(|c| c.exclusive_domain());
        let exclusive = domains.next().unwrap_or_default();
        if domains.any(|d| !std::sync::Arc::ptr_eq(&d, &exclusive)) {
            return Err("board processors have different physical exclusive domains".into());
        }
        let count = bus.len();
        let memory = Rc::new(memory);
        let mmio_callback_counts = Rc::new(RefCell::new(vec![0; n]));
        let register_access_histogram = access_histogram::new();
        let execution_counters_enabled = std::env::var_os("FUJIMOD_EXECUTION_COUNTERS").is_some();
        let store_observer =
            execution_counters_enabled.then(|| vec![StoreObserverCounters::default(); n]);
        let callback_service_time = execution_counters_enabled.then(|| callback_timing::new(n));
        let continuation = Rc::new(RefCell::new(continuation::Stops::default()));
        let host_pause = Rc::new(Cell::new(false));
        let state = Rc::new(RefCell::new(State {
            continuation,
            host_pause: host_pause.clone(),
            execution_policy: ExecutionPolicy::Exact,
            device_workers: 0,
            clock_limit: None,
            power_off: false,
            memory: memory.clone(),
            bus,
            timeline: Timeline::new_at(ratio, n, start_tick)?,
            interrupts,
            core_interrupts: (0..n).map(|_| None).collect(),
            interrupt_changed: vec![false; n],
            released,
            release_state_changed: false,
            waiting: vec![false; n],
            parked_until_sgi: vec![false; n],
            waiting_event: vec![false; n],
            events: vec![false; n],
            pins: vec![(false, false); n],
            interrupt_delivery: vec![true; n],
            instructions,
            epoch_instructions,
            accounted: vec![0; n],
            deadlines: vec![None; count],
            generations: vec![0; count],
            error: None,
            records: vec![],
            irq_observations: None,
            console: vec![],
            frames: Default::default(),
            outputs: Default::default(),
            mprofile: (0..n).map(|_| None).collect(),
            aprofile: (0..n).map(|_| None).collect(),
            armv7a: (0..n).map(|_| None).collect(),
            armv7a_entries: vec![Default::default(); n],
            entry_points: (0..n).map(|_| Vec::new()).collect(),
            implementation_cluster: Default::default(),
            unmodeled_system: Default::default(),
            processor_deadlines: vec![None; n],
            processor_generations: vec![0; n],
            reset_contexts,
            exclusive,
            code_run: None,
            code_write_deferred: false,
            code_shadow: (0..n).map(|_| None).collect(),
            blocks: (0..n).map(|_| None).collect(),
            run_entries: vec![0; n],
            polls: poll_loops::Polls::new(n),
            settled: Settled::new(),
            active_slice: None,
            parallel: None,
            decoders: decoder::Decoders::default(),
            memory_access: false,
            mprofile_loops: loops::MProfileLoops::new(n),
            access_histogram: register_access_histogram.clone(),
            store_observer: store_observer.clone(),
            callback_service_time: callback_service_time.clone(),
        }));
        let mut board = Self {
            run_wakeup: Default::default(),
            host_pause,
            progress_observer: None,
            guest_profile_active: Cell::new(false),
            isolated_core: Cell::new(None),
            idle_stop: Cell::new(false),
            idle_stopped: Cell::new(false),
            continuation: None,
            mappings: vec![],
            read_set: RefCell::new(None),
            watches: vec![],
            entries: Default::default(),
            discovery_stub_contexts: vec![],
            translation_cache_size_bytes: 0,
            cpus: Rc::new(cpus.into_iter().map(Rc::from).collect()),
            state,
            access_histogram: register_access_histogram,
            bindings: vec![],
            hooks: vec![],
            execution_hooks: vec![],
            barrier_hooks: vec![None; n],
            observer_hooks: vec![None; n],
            block_hooks: vec![vec![]; n],
            store_hooks: vec![None; n],
            block_execution: Cell::new(false),
            poll_loops: Cell::new(
                std::env::var_os("FUJIMOD_POLL_LOOPS").is_none_or(|v| v != "0"),
            ),
            store_hooks_on_demand: vec![None; n],
            store_hook_installs: Cell::new(0),
            mmio: vec![],
            mmio_callback_counts,
            interactive_windows: Cell::new(0),
            interactive_host_duration_ns: Cell::new(0),
            next_core: Cell::new(0),
            slice_left: Cell::new(0),
            quantum: Cell::new(None),
            retained_engines: None,
            physical_memory: memory,
        };
        for core in 0..n {
            if board.cpus[core].features().asynchronous_runs {
                board.cpus[core].set_run_wakeup(board.run_wakeup.clone())?;
            }
            let binding = Box::new(Binding {
                state: board.state.clone(),
                callback_service_time: callback_service_time.clone(),
                cpus: Rc::downgrade(&board.cpus),
                mmio_callback_counts: board.mmio_callback_counts.clone(),
                core,
                base: 0,
                published_reads: vec![],
                published_hits: Cell::new(0),
            });
            let data = (&*binding as *const Binding).cast_mut().cast();
            let features = board.cpus[core].features();
            if features.asynchronous_runs {
                let hook = unsafe {
                    board.cpus[core].install_hook(Hook::Synchronization {
                        callback: synchronization_barrier,
                        data,
                    })?
                };
                board.hooks.push((core, hook));
                board.execution_hooks.push(core);
                board.barrier_hooks[core] = Some(hook);
            }
            if observes_every_instruction(&*board.cpus[core]) {
                let hook = unsafe {
                    board.cpus[core].install_hook(Hook::Instruction {
                        callback: before,
                        data,
                        range: HookRange::All,
                    })?
                };
                board.hooks.push((core, hook));
                board.execution_hooks.push(core);
                board.observer_hooks[core] = Some(hook);
            }
            board.bindings.push(binding);
            if !board.cpus[core].features().shared_physical_maps
                && board.cpus.iter().any(|c| {
                    c.features().shared_physical_maps
                        || c.model() == fujimod_emulator_cpu::Model::CortexM4
                })
            {
                // Without an M-profile controller (no alias stores to
                // detect), the board store hook only shows the processor's
                // stores to a live logical exclusive reservation and to a
                // write watch. An all-store hook sends every store of the
                // processor through the engine's slow path, so it is
                // installed on demand (`sync_store_hook`).
                board.execution_hooks.push(core);
                if board.cpus[core].model() != fujimod_emulator_cpu::Model::CortexM4 {
                    board.store_hooks_on_demand[core] = Some(None);
                } else {
                    let hook = unsafe {
                        board.cpus[core].install_hook(Hook::Memory {
                            callback: written,
                            data,
                            read: false,
                            write: true,
                            range: HookRange::All,
                        })?
                    };
                    board.hooks.push((core, hook));
                    board.store_hooks[core] = Some(hook);
                }
            }
        }
        board.state.borrow_mut().refresh_deadlines()?;
        Ok(board)
    }
    pub fn cpu(&self, index: usize) -> Result<&dyn Cpu, String> {
        self.cpus
            .get(index)
            .map(|c| &**c)
            .ok_or("absent processor".into())
    }
    pub fn instruction_counts(&self) -> Vec<u64> {
        self.state.borrow().instructions.clone()
    }
    /// Existing CPU backend counters, including the R-profile set when used.
    /// These are observations and never enter guest state.
    pub fn exit_counters(
        &self,
    ) -> Vec<Result<std::collections::BTreeMap<&'static str, u64>, String>> {
        self.cpus.iter().map(|cpu| cpu.exit_counters()).collect()
    }
    /// Whether the processors constructed for this epoch have executed.
    pub fn epoch_is_unexecuted(&self) -> bool {
        let state = self.state.borrow();
        state.instructions == state.epoch_instructions
    }
    /// Serviced board MMIO callbacks, indexed by processor.
    pub fn mmio_trap_callback_counts(&self) -> Vec<u64> {
        self.mmio_callback_counts.borrow().clone()
    }
    /// Interactive run windows completed by the board scheduler.
    pub fn interactive_window_count(&self) -> u64 {
        self.interactive_windows.get()
    }
    /// Mean host duration of completed interactive windows, in nanoseconds.
    pub fn mean_interactive_window_host_duration_ns(&self) -> Option<u64> {
        let count = self.interactive_windows.get();
        (count != 0).then(|| {
            u64::try_from(self.interactive_host_duration_ns.get() / u128::from(count))
                .unwrap_or(u64::MAX)
        })
    }
    /// Processor address views of RAM, as continuation watches take them.
    pub fn mappings(&self) -> &[factory::MemoryMap] {
        &self.mappings
    }
    /// Processor address views dispatched through the device bus, as
    /// (core, address, bytes, physical base).
    pub fn mmio_mappings(&self) -> &[(usize, u64, usize, u64)] {
        &self.mmio
    }
    pub fn now(&self) -> u64 {
        self.state.borrow().timeline.now()
    }
    /// Whether a connected device has removed the board supply.
    pub fn is_powered_off(&self) -> bool {
        self.state.borrow().power_off
    }
    /// Opt in to ending a run once a device deadline passes with every
    /// released core still waiting, instead of skipping further idle time.
    pub fn set_idle_stop(&self, enabled: bool) {
        self.idle_stop.set(enabled);
    }
    /// Whether the last run ended with every released core waiting and either
    /// no pending deadline or, under `set_idle_stop`, a deadline that woke none.
    pub fn idle_stopped(&self) -> bool {
        self.idle_stopped.get()
    }
    /// Advance only the unpowered interval. Old CPU and device events cannot run.
    pub fn advance_unpowered(&mut self, at: u64) -> Result<(), String> {
        let mut state = self.state.try_borrow_mut().map_err(|_| "board is executing")?;
        if !state.power_off || state.active_slice.is_some() || state.parallel.is_some() {
            return Err("unpowered advance requires a stopped, unpowered board".into());
        }
        state.timeline.advance_unpowered(at)?;
        state.deadlines.fill(None);
        state.processor_deadlines.fill(None);
        Ok(())
    }
    /// Begin block-level guest observations while all processors are stopped.
    pub fn start_guest_profile(&self) -> Result<(), String> {
        if self.guest_profile_active.get() {
            return Err("guest profile is already active".into());
        }
        let mut started = Vec::<usize>::new();
        for (index, cpu) in self.cpus.iter().enumerate() {
            if let Err(error) = cpu.start_guest_profile() {
                for started_index in started {
                    let _ = self.cpus[started_index].finish_guest_profile();
                }
                return Err(format!("processor {index} guest profile: {error}"));
            }
            started.push(index);
        }
        self.guest_profile_active.set(true);
        Ok(())
    }
    /// Finish block-level guest observations after a board run.
    pub fn guest_profile(&self) -> Value {
        let cores: Vec<_> = self
            .cpus
            .iter()
            .enumerate()
            .filter_map(|(core, cpu)| match cpu.finish_guest_profile() {
                Ok(Some(blocks)) => Some(serde_json::json!({"core": core, "blocks": blocks})),
                Ok(None) => None,
                Err(error) => Some(serde_json::json!({"core": core, "error": error})),
            })
            .collect();
        self.guest_profile_active.set(false);
        serde_json::json!({
            "version": 1,
            "phase_semantics": {
                "idle": "no physical key or touch input is active",
                "key": "a physical key or touch input is active"
            },
            "cores": cores
        })
    }
    /// Set the phase used for subsequent optional guest profile entries.
    pub fn set_guest_profile_phase(&self, phase: GuestProfilePhase) -> Result<(), String> {
        if !self.guest_profile_active.get() {
            return Ok(());
        }
        for cpu in self.cpus.iter() {
            cpu.guest_profile_phase(phase)?;
        }
        Ok(())
    }
    /// Observation only: every released processor is waiting for an input.
    /// Device deadlines may still wake a processor in a later run.
    pub fn quiescent(&self) -> bool {
        let state = self.state.borrow();
        state
            .released
            .iter()
            .zip(&state.waiting)
            .all(|(&released, &waiting)| !released || waiting)
    }
    pub fn read_register(&self, core: usize, register: Register) -> Result<u64, String> {
        scalar(self.cpu(core)?, register)
    }
    pub fn write_register(
        &self,
        core: usize,
        register: Register,
        value: u64,
    ) -> Result<(), String> {
        set(self.cpu(core)?, register, value)
    }
    pub fn read_memory(&self, core: usize, address: u64, bytes: &mut [u8]) -> Result<(), String> {
        self.track_cpu_read(core, address, bytes.len())?;
        self.cpu(core)?.read_memory(address, bytes)
    }
    pub fn read_physical(&self, address: u64, bytes: &mut [u8]) -> Result<(), String> {
        self.track_physical_read(address, bytes.len())?;
        self.physical_memory.read_host(address, bytes)
    }
    /// Compare a paused physical memory range against previously observed bytes.
    pub fn physical_bytes_equal(&self, address: u64, expected: &[u8]) -> Result<bool, String> {
        self.track_physical_read(address, expected.len())?;
        self.physical_memory.host_bytes_equal(address, expected)
    }
    /// Read physical memory without adding it to an active predicate's read
    /// set: the bytes are observed, but stores to them do not end a wait.
    pub fn inspect_physical(&self, address: u64, bytes: &mut [u8]) -> Result<(), String> {
        self.physical_memory.read_host(address, bytes)
    }
    /// Inspect a declared backing address independently of bus mappings.
    pub fn inspect_backing(&self, address: u64, bytes: &mut [u8]) -> Result<(), String> {
        self.physical_memory.read_backing(address, bytes)
    }
    /// Validate a complete backing range without reading or modifying it.
    pub fn validate_backing(&self, address: u64, size: usize) -> Result<(), String> {
        self.physical_memory
            .backing_chunks(address, size)
            .map(|_| ())
    }
    /// Restore backing bytes into an unexecuted epoch without changing bus mappings.
    pub fn restore_backing(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
        if !self.epoch_is_unexecuted() {
            return Err("backing restore requires an unexecuted epoch".into());
        }
        let mut state = self.state.borrow_mut();
        state.code_run = None;
        self.physical_memory
            .write_backing(&self.cpus, &state.exclusive, address, bytes)
    }
    pub fn write_physical(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
        let mut state = self.state.borrow_mut();
        state.code_run = None;
        self.physical_memory.watch_transfer(
            write_watch::WriteSource::Host,
            address,
            bytes,
            state.timeline.now(),
        );
        self.physical_memory
            .write_host(&self.cpus, &state.exclusive, address, bytes)
    }
    pub fn write_memory(&self, core: usize, address: u64, bytes: &[u8]) -> Result<(), String> {
        self.cpu(core)?.write_memory(address, bytes)?;
        let mut state = self.state.borrow_mut();
        state.code_run = None;
        notify_memory(
            &self.cpus,
            &state.mprofile,
            &state.exclusive,
            core,
            address,
            bytes.len(),
        )
    }
    pub fn inputs_changed(&mut self, source: usize) -> Result<(), String> {
        let mut state = self.state.borrow_mut();
        state.bus.inputs_changed(source)?;
        state.refresh_deadlines()
    }
    /// Change a stopped device's external inputs through its concrete
    /// model at the current device time (`f`'s second argument), then
    /// publish the change as `inputs_changed` does.
    pub fn with_device_inputs<T: fujimod_emulator_devices::Device, R>(
        &mut self,
        id: usize,
        f: impl FnOnce(&mut T, u64) -> Result<R, String>,
    ) -> Result<R, String> {
        let result = {
            let mut s = self.state.borrow_mut();
            let now = s.timeline.now();
            f(s.bus.device_mut::<T>(id)?, now)?
        };
        self.inputs_changed(id)?;
        Ok(result)
    }
    /// Observe a device through its concrete model without changing it.
    pub fn with_device<T: fujimod_emulator_devices::Device, R>(
        &self,
        id: usize,
        f: impl FnOnce(&T) -> R,
    ) -> Result<R, String> {
        Ok(f(self.state.borrow().bus.device::<T>(id)?))
    }
    pub fn configure_core_interrupts(
        &mut self,
        core: usize,
        device: usize,
        inputs: Box<dyn fujimod_emulator_devices::bus::CoreInterrupts>,
    ) -> Result<(), String> {
        if self.cpu(core)?.model() != fujimod_emulator_cpu::Model::CortexR5F {
            return Err("per-core interrupt device requires an R-profile processor".into());
        }
        let unexecuted = self.epoch_is_unexecuted();
        let mut s = self.state.borrow_mut();
        if !unexecuted
            || s.core_interrupts[core].is_some()
            || s.core_interrupts
                .iter()
                .flatten()
                .any(|c| c.device == device)
            || s.interrupts.as_ref().is_some_and(|g| core < g.cores)
            || device >= s.bus.len()
        {
            return Err("invalid or already connected per-core interrupt device".into());
        }
        s.core_interrupts[core] = Some(CoreInterruptMap { device, inputs });
        s.bus.inputs_changed(device)?;
        s.refresh_deadlines()
    }
    /// Explicit external interrupt input. Applying its effect remains guest
    /// work at the controller's immediate deadline, rather than a host read.
    pub fn raise_irq(&mut self, core: usize, line: u32) -> Result<(), String> {
        let mut s = self.state.borrow_mut();
        s.with_host(&self.cpus, |_, host| host.raise_irq(core, line))?;
        s.refresh_deadlines()
    }
    pub fn set_core_irq(&mut self, core: usize, line: u32, high: bool) -> Result<(), String> {
        let mut s = self.state.borrow_mut();
        s.with_host(&self.cpus, |_, host| host.set_core_irq(core, line, high))?;
        s.refresh_deadlines()
    }
    pub fn configure_mprofile(
        &mut self,
        core: usize,
        config: mprofile::Config,
        views: Vec<mprofile::MemoryView>,
    ) -> Result<(), String> {
        if self.cpu(core)?.model() != fujimod_emulator_cpu::Model::CortexM4 {
            return Err("M-profile controller requires a Cortex-M4 execution model".into());
        }
        if self.state.borrow().instructions.iter().any(|n| *n != 0) {
            return Err("cannot install a processor controller after execution".into());
        }
        if self.state.borrow().mprofile[core].is_some() {
            return Err("processor controller is already installed".into());
        }
        let profile = mprofile::Profile::new(config, self.state.borrow().exclusive.clone(), views)?;
        self.map_mmio(core, 0xe0000000, 0x100000, 0xe0000000)?;
        let mut state = self.state.borrow_mut();
        state.mprofile[core] = Some(profile);
        state.code_shadow[core] = Some(Default::default());
        state.blocks[core] = Some(Default::default());
        Ok(())
    }
    pub fn configure_aprofile(
        &mut self,
        core: usize,
        config: aprofile::Config,
    ) -> Result<(), String> {
        if self.cpu(core)?.model() != fujimod_emulator_cpu::Model::ArmV8A {
            return Err("A-profile controller requires an AArch64 processor".into());
        }
        let mut state = self.state.borrow_mut();
        if state.instructions.iter().any(|n| *n != 0) || state.aprofile[core].is_some() {
            return Err("A-profile controller must be installed once before execution".into());
        }
        if state.interrupts.as_ref().is_none_or(|g| core >= g.cores) {
            return Err("A-profile timers require this core's interrupt controller".into());
        }
        state.aprofile[core] = Some(aprofile::Profile::new(config)?);
        state.refresh_processors()
    }
    pub fn map_mmio(
        &mut self,
        core: usize,
        address: u64,
        bytes: usize,
        physical: u64,
    ) -> Result<(), String> {
        let cpu = self.cpus.get(core).ok_or("absent processor")?;
        let fast_reads = self
            .state
            .borrow()
            .bus
            .fast_reads(physical, bytes)
            .into_iter()
            .map(|(offset, value)| match value {
                fujimod_emulator_devices::bus::FastReadValue::AtomicU16(value) => {
                    fujimod_emulator_cpu::FastMmioRead::atomic_u16(offset, value)
                }
                fujimod_emulator_devices::bus::FastReadValue::AtomicU32(value) => {
                    fujimod_emulator_cpu::FastMmioRead::atomic_u32(offset, value)
                }
                fujimod_emulator_devices::bus::FastReadValue::ClockedU32(read) => {
                    fujimod_emulator_cpu::FastMmioRead::clocked_u32(offset, read)
                }
                fujimod_emulator_devices::bus::FastReadValue::ConsumingU32(read) => {
                    fujimod_emulator_cpu::FastMmioRead::consuming_u32(offset, read)
                }
            })
            .collect::<Vec<_>>();
        self.state
            .borrow_mut()
            .mprofile_loops
            .add_reads(core, address, &fast_reads);
        let mut engine_reads = fast_reads;
        let published_reads = if cpu.model() == fujimod_emulator_cpu::Model::CortexM4 {
            // Each synchronous M-profile access settles its own mixed-width
            // block in the board. Only then is its device tick known.
            std::mem::take(&mut engine_reads)
        } else {
            Vec::new()
        };
        let binding = Box::new(Binding {
            state: self.state.clone(),
            callback_service_time: self.state.borrow().callback_service_time.clone(),
            cpus: Rc::downgrade(&self.cpus),
            mmio_callback_counts: self.mmio_callback_counts.clone(),
            core,
            base: physical,
            published_reads,
            published_hits: Cell::new(0),
        });
        let data = (&*binding as *const Binding).cast_mut().cast();
        unsafe {
            cpu.map_mmio_fast(address, bytes, Mmio { read, write, data }, engine_reads)?;
        }
        self.bindings.push(binding);
        self.mmio.push((core, address, bytes, physical));
        Ok(())
    }
    /// Observe guest ticks every 100,000 executed instructions, at existing scheduler
    /// boundaries. The listener receives no board access and does not change guest
    /// scheduling or stop criteria.
    pub fn set_progress_observer(&mut self, observer: Option<Box<dyn FnMut(u64)>>) {
        self.progress_observer = observer;
    }
    /// Execute to a guest tick. Exact uses deterministic round-robin slices;
    /// Interactive workers finish blocks and join at device/owner barriers.
    pub fn run_until(&mut self, until: u64, quantum: usize) -> Result<(), String> {
        if self.is_powered_off() { return Err("cannot run an unpowered board".into()); }
        let result = self.run_inner(Some(until), None, None, quantum);
        self.state.borrow_mut().clock_limit = None;
        if let Err(error) = &result {
            self.state.borrow_mut().fail(error.clone());
        }
        result
    }
    pub fn run_for_instructions(&mut self, count: u64, quantum: usize) -> Result<(), String> {
        self.run_with_limits(count, quantum, None, None)
    }
    /// Exact thresholds preserve partial slices and stop at hard instruction
    /// limits. Interactive thresholds are block boundaries and may overshoot
    /// by an admitted block per runnable core; guest time stops at `until`.
    pub fn run_with_limits(
        &mut self,
        count: u64,
        quantum: usize,
        pause_after: Option<u64>,
        until: Option<u64>,
    ) -> Result<(), String> {
        if count == 0 {
            return Ok(());
        }
        let start = self.state.borrow().total_instructions()?;
        let end = start
            .checked_add(count)
            .ok_or("board instruction limit overflow")?;
        // Reusing a threshold still makes one quantum's progress, bounded
        // by the hard allowance. A positive request never becomes a no-op.
        let pause_after = pause_after.map(|pause| {
            if pause <= start {
                start.saturating_add((quantum as u64).min(count))
            } else {
                pause
            }
        });
        self.run_window(end, quantum, pause_after, until)
    }
    /// Continue one host instruction window after passive instruction stops.
    /// The caller normalizes its soft threshold once for the whole window.
    pub fn run_window(
        &mut self,
        end: u64,
        quantum: usize,
        pause_after: Option<u64>,
        until: Option<u64>,
    ) -> Result<(), String> {
        if self.is_powered_off() { return Err("cannot run an unpowered board".into()); }
        let start = self.state.borrow().total_instructions()?;
        if end < start && self.execution_policy() == ExecutionPolicy::Exact {
            return Err("board instruction window moved backward".into());
        }
        if end <= start {
            return Ok(());
        }
        let result = self.run_inner(until, Some(end), pause_after, quantum);
        self.state.borrow_mut().clock_limit = None;
        if let Err(error) = &result {
            self.state.borrow_mut().fail(error.clone());
        }
        result
    }
    fn run_inner(
        &mut self,
        until: Option<u64>,
        instruction_limit: Option<u64>,
        pause_after: Option<u64>,
        quantum: usize,
    ) -> Result<(), String> {
        if self.is_powered_off() {
            return Err("cannot run an unpowered board".into());
        }
        prepare_device_workers(self.execution_policy(), self.device_worker_preference())?;
        if quantum == 0 {
            return Err("zero processor quantum".into());
        }
        if self.quantum.get().is_some_and(|q| q != quantum) {
            return Err("processor quantum is fixed for a board run".into());
        }
        self.quantum.set(Some(quantum));
        if !self.state.borrow().bus.has_host_requests() {
            self.host_pause.set(false);
        }
        self.state.borrow_mut().code_run = None;
        self.state.borrow_mut().clock_limit = until;
        {
            let state = self.state.borrow();
            for (cpu, profile) in self.cpus.iter().zip(&state.armv7a) {
                if profile.is_some() {
                    cpu.select_armv7a_affinity_policy(state.execution_policy)?;
                }
            }
        }
        if self.execution_policy() != ExecutionPolicy::Exact {
            // Concurrent windows do not pass the per-slice check.
            for core in 0..self.cpus.len() {
                self.sync_store_hook(core)?;
            }
        }
        self.idle_stopped.set(false);
        let mut idle_mark = None;
        let mut completed_slice = None;
        let mut next_progress = 0;
        loop {
            if let Some(observer) = &mut self.progress_observer {
                let (instructions, now) = {
                    let s = self.state.borrow();
                    (s.total_instructions()?, s.timeline.now())
                };
                if instructions >= next_progress {
                    observer(now);
                    next_progress = instructions.saturating_add(100_000);
                }
            }
            let mut s = self.state.borrow_mut();
            s.apply_memory(&self.cpus)?;
            s.refresh_processors()?;
            s.park();
            s.due(&self.cpus)?;
            s.push_pins(&self.cpus)?;
            // Clock inputs advance with each processor's own execution and
            // idle time, including for parked peers. Host counter inspection
            // never drives an update.
            for (core, cpu) in self.cpus.iter().enumerate() {
                if cpu.features().guest_clock {
                    cpu.set_guest_clock(s.timeline.processor_clock(core)?)?;
                }
            }
            if let Some(error) = s.error.take() {
                return Err(error);
            }
            if s.continuation.borrow().fired
                || self.host_pause.get()
                || self.isolated_core.get().is_some_and(|core| s.waiting[core])
            {
                return Ok(());
            }
            let remaining = instruction_limit
                .map(|limit| {
                    s.total_instructions().and_then(|n| {
                        if s.execution_policy == ExecutionPolicy::Interactive {
                            Ok(limit.saturating_sub(n))
                        } else {
                            limit
                                .checked_sub(n)
                                .ok_or("board instruction limit exceeded".into())
                        }
                    })
                })
                .transpose()?
                .unwrap_or(u64::MAX);
            // A run a loop window stopped continues to its counted end, as
            // ordinary execution would have, before the clock limit applies.
            let resume = s.mprofile_loops.take_resume().filter(|&(core, end)| {
                end > s.instructions[core] && s.released[core] && !s.waiting[core]
            });
            if (resume.is_none() && until.is_some_and(|until| s.timeline.now() >= until))
                || remaining == 0
            {
                return Ok(());
            }
            let n = self.cpus.len();
            if resume.is_none()
                && pause_after.is_some_and(|pause| s.total_instructions().is_ok_and(|n| n >= pause))
                && (s.execution_policy == ExecutionPolicy::Interactive
                    || completed_slice
                        .is_some_and(|core| (core + 1..n).all(|c| !s.released[c] || s.waiting[c])))
            {
                return Ok(());
            }
            if s.execution_policy == ExecutionPolicy::Interactive {
                let cores: Vec<_> = (0..n)
                    .map(|offset| (self.next_core.get() + offset) % n)
                    .filter(|&core| s.released[core] && !s.waiting[core])
                    .collect();
                if cores.len() > 1
                    && cores
                        .iter()
                        .all(|&core| self.cpus[core].features().asynchronous_runs)
                {
                    let entered = s.total_instructions()?;
                    let pause_room =
                        pause_after.map_or(u64::MAX, |pause| pause.saturating_sub(entered).max(1));
                    let room = remaining
                        .min(pause_room)
                        .div_ceil(cores.len() as u64)
                        .max(1);
                    let cap = s.timeline.ratio().budget(
                        s.timeline.now(),
                        s.timeline.phase(),
                        until.unwrap_or(u64::MAX),
                    )?;
                    let instruction_limit = room.min(cap).min(usize::MAX as u64);
                    let boundary = engine_run_boundary(&s.timeline, instruction_limit, until, 0)?;
                    let budget = s.timeline.budget(instruction_limit)? as usize;
                    if budget == 0 {
                        return Err("scheduler made no progress at a device deadline".into());
                    }
                    drop(s);
                    self.interactive_window(&cores, budget, quantum, boundary)?;
                    completed_slice = None;
                    continue;
                }
            }
            let start = self.next_core.get();
            let core = resume.map(|(core, _)| core).or_else(|| {
                (0..n)
                    .map(|i| (start + i) % n)
                    .find(|&c| s.released[c] && !s.waiting[c])
            });
            let Some(core) = core else {
                if until.is_none() && s.timeline.next_event().is_none() {
                    self.idle_stopped.set(true);
                    return Ok(());
                }
                // Opt-in: a device deadline already passed while every
                // released core stayed in its architectural wait. Return
                // instead of skipping guest time to further deadlines.
                if self.idle_stop.get() {
                    let executed = s.total_instructions()?;
                    if idle_mark == Some(executed) {
                        self.idle_stopped.set(true);
                        return Ok(());
                    }
                    idle_mark = Some(executed);
                }
                s.timeline.advance_waiting(true, until)?;
                continue;
            };
            if core != start || self.slice_left.get() == 0 {
                self.slice_left.set(quantum);
            }
            self.next_core.set(core);
            let slice =
                schedule::Slice::new(core, s.instructions[core], self.slice_left.get(), quantum)?;
            // Concurrent Interactive processors already took the window above.
            // A serialized backend still owes runnable peers a bounded turn.
            let peer = s.runnable_peer(core);
            let solo_pause = if peer {
                u64::MAX
            } else if let Some(pause) = pause_after {
                let distance = pause.saturating_sub(s.total_instructions()?).max(1);
                let room = self.slice_left.get() as u64;
                room.saturating_add(
                    distance
                        .saturating_sub(room)
                        .div_ceil(quantum as u64)
                        .saturating_mul(quantum as u64),
                )
            } else {
                u64::MAX
            };
            let instruction_limit = (if peer {
                self.slice_left.get() as u64
            } else {
                solo_pause
            })
                .min(remaining)
                .min(usize::MAX as u64);
            let lag = s
                .timeline
                .lags()
                .get(core)
                .copied()
                .ok_or_else(|| format!("timeline has no processor {core}"))?;
            let boundary = engine_run_boundary(&s.timeline, instruction_limit, until, lag)?;
            let budget = match resume {
                Some((_, end)) => (end - s.instructions[core]).min(remaining) as usize,
                None => s
                    .timeline
                    .processor_budget(core, instruction_limit, until)?
                    as usize,
            };
            if budget == 0 {
                return Err("scheduler made no progress at a device deadline".into());
            }
            s.accounted[core] = 0;
            if self.cpus[core].model() == fujimod_emulator_cpu::Model::CortexM4
                && s.mprofile[core].is_none()
            {
                return Err("M-profile processor requires its architectural controller".into());
            }
            if s.mprofile[core].as_ref().is_some_and(|p| p.needs_reset) {
                s.settled.clear();
                s.code_run = None;
                if let Some(shadow) = s.code_shadow[core].as_mut() {
                    shadow.invalidate();
                }
                self.cpus[core].restore(
                    s.reset_contexts[core]
                        .as_deref()
                        .ok_or("M-profile reset context is missing")?,
                )?;
                s.mprofile[core]
                    .as_mut()
                    .unwrap()
                    .reset(&*self.cpus[core])?;
            }
            if s.mprofile[core].is_some() {
                // A peer's guest stores are invisible to this engine, as are
                // host and DMA writes between its slices. After any of them,
                // keep the translations only while the shadow can prove
                // their bytes. A write during this processor's own slice, an
                // alias store, a decoder change or a reset discards them.
                // This holds under either execution policy: a slice on this
                // path runs alone (a concurrent window never holds a
                // processor whose slices come here, and it clears `code_run`
                // when it ends), and the instruction observer runs before
                // every instruction of this engine whatever the policy.
                let peer_ran = s.code_run != Some(core);
                let shadow = s.code_shadow[core].as_mut();
                let discard = match &shadow {
                    Some(shadow) => shadow.changed() || peer_ran && shadow.unproven(),
                    None => peer_ran,
                };
                if discard {
                    self.cpus[core].invalidate_code(None)?;
                }
                if let Some(shadow) = shadow {
                    if discard {
                        shadow.discarded();
                    } else if peer_ran {
                        shadow.peer_ran();
                    }
                    shadow.begin_slice();
                }
            }
            s.code_run = Some(core);
            s.code_write_deferred = false;
            let pins_settled = s.mprofile[core].is_some()
                && self.block_execution.get()
                && s.settled.pass;
            s.settled.clear();
            if pins_settled {
                // The scheduler just refreshed deadlines and pushed pins.
                // Start this ordered slice with that fixed point, but make
                // the first observer check the controller for an exception.
                s.settled.pass = true;
                s.arm_settled()?;
            }
            s.active_slice = Some(slice);
            let entered = s.instructions[core];
            s.mprofile_loops.begin_run(core, entered + budget as u64);
            drop(s);
            self.sync_store_hook(core)?;
            let (blocks, _thumb_slice) = self.sync_block_mode(core, budget as u64)?;
            self.sync_poll_loops(core, entered, budget as u64);
            let cpu = &self.cpus[core];
            // The exact budget ends at the first device/DMA/processor event
            // or runnable-peer turn; those boundaries end the stable window.
            cpu.set_loop_window(self.state.borrow().loop_window())?;
            let pc = scalar(&**cpu, Register::Pc)?;
            let mut engine_guard = fujimod_perf::engine_run(
                u8::try_from(core).unwrap_or(u8::MAX),
                false,
            );
            // Block execution has no engine counter: the board debits entries.
            let mut limit = budget;
            self.state.borrow_mut().run_entries[core] = 0;
            let mut result = cpu.run(Run {
                begin: if cpu.model() == fujimod_emulator_cpu::Model::CortexM4
                    || matches!(
                        cpu.model(),
                        fujimod_emulator_cpu::Model::CortexR5F
                            | fujimod_emulator_cpu::Model::CortexA7
                    ) && scalar(&**cpu, Register::Pstate)? & 0x20 != 0
                {
                    pc | 1
                } else {
                    pc
                },
                until: None,
                watchdog_us: 0,
                instructions: (!blocks).then_some(limit),
            });
            // An observer stopped the run for host bookkeeping: it found code
            // changed behind the engine, or (block execution) it changed the
            // PC. Resume the same slice at once: the stop is not a scheduling
            // boundary, so nothing else may run in between.
            while result.is_ok() {
                let resync = self.code_resync(core)?;
                let mut s = self.state.borrow_mut();
                let resume = s.blocks[core]
                    .as_mut()
                    .is_some_and(|blocks| std::mem::take(&mut blocks.resume));
                if !(resync || resume) || s.error.is_some() {
                    break;
                }
                if !blocks {
                    // The engine counted every entry that reached the
                    // observer, the stopped one too; that one runs again.
                    let consumed = s.run_entries[core].saturating_sub(1);
                    limit = limit.saturating_sub(consumed as usize);
                    s.run_entries[core] = 0;
                    if limit == 0 {
                        break;
                    }
                }
                drop(s);
                result = cpu.run(Run {
                    begin: scalar(&**cpu, Register::Pc)? | 1,
                    until: None,
                    watchdog_us: 0,
                    instructions: (!blocks).then_some(limit),
                });
            }
            engine_guard.stop();
            engine_guard.set_instructions(cpu.instruction_progress().unwrap_or(0));
            let mut s = self.state.borrow_mut();
            s.active_slice = None;
            s.mprofile_loops.end_run();
            use fujimod_emulator_cpu::{ExecutionError, Fault};
            if blocks && matches!(&result, Err(ExecutionError::Memory {
                fault: Fault::ReadUnmapped | Fault::WriteUnmapped | Fault::ReadProtected | Fault::WriteProtected, ..
            })) {
                s.settle_interrupted_block(&**cpu, core, scalar(&**cpu, Register::Pc)? & !1, true)?;
            } else {
                s.settle_block(core, None)?;
            }
            s.end_code_slice(&**cpu, core);
            if std::mem::take(&mut s.code_write_deferred) {
                s.code_run = None;
            }
            s.sync(&**cpu, core)?;
            if let Some(error) = s.error.take() {
                return Err(error);
            }
            // Unicorn reports disabled VFP/Advanced SIMD as an invalid
            // instruction; the Cortex-A7 takes the undefined exception.
            let result = match result {
                Err(fujimod_emulator_cpu::ExecutionError::InvalidInstruction(_))
                    if cpu.model() == fujimod_emulator_cpu::Model::CortexA7
                        && fujimod_emulator_cpu::armv7a::enter_disabled_vfp(&**cpu)? =>
                {
                    s.armv7a_entries[core].undefined += 1;
                    Ok(fujimod_emulator_cpu::Exit::Stopped)
                }
                other => other,
            };
            let exit = result.map_err(|error| {
                if cpu.model() == fujimod_emulator_cpu::Model::ArmV8A {
                    let stop_pc = scalar(&**cpu, Register::Pc).unwrap_or(pc);
                    let mut bytes = [0; 4];
                    let word = cpu
                        .read_virtual(
                            stop_pc,
                            fujimod_emulator_cpu::Access::Fetch,
                            &mut bytes,
                        )
                        .map(|()| format!("{:#010x}", u32::from_le_bytes(bytes)))
                        .unwrap_or_else(|_| "unavailable".into());
                    let x0 = scalar(&**cpu, Register::X(0))
                        .map(|value| format!("{value:#x}"))
                        .unwrap_or_else(|_| "unavailable".into());
                    let lr = scalar(&**cpu, Register::X(30))
                        .map(|value| format!("{value:#x}"))
                        .unwrap_or_else(|_| "unavailable".into());
                    format!(
                        "CPU{core} {} at PC {stop_pc:#x}, LR {lr}, instruction {word}, X0 {x0}: {error}",
                        error.kind()
                    )
                } else {
                    format!("CPU{core}: {error}")
                }
            })?;
            s.apply_memory(&self.cpus)?;
            s.finish_exit(&**cpu, core, pc, exit)?;
            let entered = s.instructions[core]
                .checked_sub(entered)
                .ok_or("processor entry count moved backward")?;
            let reason = engine_end_reason(
                Some(exit),
                entered,
                budget as u64,
                boundary,
                s.continuation.borrow().fired || self.host_pause.get(),
                false,
                false,
            );
            engine_guard.set_instructions(entered);
            engine_guard.finish(reason);
            if entered > budget as u64 && s.execution_policy == ExecutionPolicy::Exact {
                return Err("CPU exceeded its requested instruction budget".into());
            }
            self.slice_left
                .set(slice.room(s.instructions[core]) as usize);
            completed_slice = None;
            if s.execution_policy == ExecutionPolicy::Interactive
                || self.slice_left.get() == 0
                || s.waiting[core]
                || !s.released[core]
            {
                completed_slice = Some(core);
                self.next_core.set((core + 1) % n);
                self.slice_left.set(0);
            }
        }
    }
    /// Install or remove a processor's on-demand board store hook before it
    /// runs. The hook has work while a logical exclusive reservation is live
    /// (the processor's stores must reach the shared monitor), while a write
    /// watch is armed, and outside exact execution, where peers run without
    /// this check between their slices. Reservations change only while their
    /// owner runs, so the state found here holds for the processor's slice.
    /// Without the hook the monitor's write epoch does not count the
    /// processor's stores; no reservation is live to observe them.
    fn sync_store_hook(&mut self, core: usize) -> Result<(), String> {
        let Some(installed) = self.store_hooks_on_demand[core] else {
            return Ok(());
        };
        let wanted = {
            let s = self.state.borrow();
            s.execution_policy != ExecutionPolicy::Exact
                || s.memory.watching_writes()
                || s.exclusive
                    .lock()
                    .map_err(|_| "exclusive monitors poisoned")?
                    .active()
        };
        match (wanted, installed) {
            (true, None) => {
                let data = (&*self.bindings[core] as *const Binding).cast_mut().cast();
                // The binding box stays at this address until Board drops its CPU hooks.
                let hook = unsafe {
                    self.cpus[core].install_hook(Hook::Memory {
                        callback: written,
                        data,
                        read: false,
                        write: true,
                        range: HookRange::All,
                    })?
                };
                self.hooks.push((core, hook));
                self.store_hooks_on_demand[core] = Some(Some(hook));
                self.store_hook_installs.set(self.store_hook_installs.get() + 1);
            }
            (false, Some(hook)) => {
                self.cpus[core].remove_hook(hook)?;
                self.hooks.retain(|&(owner, other)| !(owner == core && other == hook));
                self.store_hooks_on_demand[core] = Some(None);
            }
            _ => (),
        }
        Ok(())
    }
    /// The hook wiring a snapshot or keyframe binds to: the execution core's
    /// own hooks by owner, as constructed, then every hook another owner
    /// installed (PC witnesses, write watches, discovery stubs) by identity.
    /// It does not change when the execution core installs or removes its own
    /// hooks between runs.
    pub(super) fn hook_wiring(&self) -> Vec<(usize, Option<HookId>)> {
        let own = |core: usize, hook: HookId| {
            self.barrier_hooks[core] == Some(hook)
                || self.observer_hooks[core] == Some(hook)
                || self.store_hooks[core] == Some(hook)
                || self.store_hooks_on_demand[core] == Some(Some(hook))
                || self.block_hooks[core].contains(&hook)
        };
        self.execution_hooks
            .iter()
            .map(|&core| (core, None))
            .chain(
                self.hooks
                    .iter()
                    .filter(|&&(core, hook)| !own(core, hook))
                    .map(|&(core, hook)| (core, Some(hook))),
            )
            .collect()
    }
    /// Allow or forbid block execution of M-profile processors. Both ways run
    /// the same guest instructions and reach the same state. Block execution
    /// replaces the instruction observer, and with it the M-profile loop
    /// windows, so a board chooses one: blocks pay off where the processors
    /// compute, loop windows where they wait in delay and poll loops.
    pub fn set_block_execution(&self, on: bool) {
        self.block_execution.set(on);
    }
    /// Before an AArch64 processor runs: whether this run may fast-forward a
    /// poll loop (`poll_loops`), and where its entry budget ends. It needs
    /// exact ordered execution, an engine whose entry count is exact inside
    /// a callback and whose limit a callback can lower, and nothing else
    /// that observes the processor's instructions or stores.
    fn sync_poll_loops(&self, core: usize, entered: u64, budget: u64) {
        let cpu = &self.cpus[core];
        let features = cpu.features();
        let mut s = self.state.borrow_mut();
        let active = self.poll_loops.get()
            && cpu.model() == fujimod_emulator_cpu::Model::ArmV8A
            && features.instruction_progress
            && features.tighten_instruction_limit
            && s.execution_policy == ExecutionPolicy::Exact
            && s.parallel.is_none()
            && s.entry_points[core].is_empty()
            && !s.memory.watching_writes()
            && self.watches.is_empty()
            && self.discovery_stub_contexts.is_empty()
            && self.isolated_core.get().is_none()
            && self.read_set.try_borrow().is_ok_and(|r| r.is_none())
            && !self.continuation.as_ref().is_some_and(|watch| watch.active);
        s.polls.active[core] = active;
        s.polls.last[core] = None;
        s.polls.run_end[core] = entered.saturating_add(budget);
    }
    /// Allow or forbid the fast-forward of poll loops (`mprofile_blocks::Poll`,
    /// `poll_loops`).
    /// Both ways give the same guest results; this exists to compare them.
    pub fn set_poll_loops(&self, on: bool) {
        self.poll_loops.set(on);
    }
    /// Choose, before a processor runs, between the per-instruction observer
    /// and block execution (`mprofile_blocks`), and set the engine up for the
    /// choice. Block execution needs an M-profile controller running in an
    /// ordered slice, and no observer the board's execution core did not install
    /// itself (PC witnesses, instruction points, discovery stubs, write
    /// watches): those read counters at their own instructions. Returns
    /// whether the processor runs in block execution, with `budget` entries.
    fn sync_block_mode(
        &mut self,
        core: usize,
        budget: u64,
    ) -> Result<(bool, Option<thumb_accounting::Slice>), String> {
        let (active, wanted) = {
            let s = self.state.borrow();
            let Some(blocks) = s.blocks[core].as_ref() else {
                return Ok((false, None));
            };
            // The hooks the execution core keeps on this processor: its store
            // hook, and the instruction observer or the block-mode hooks.
            let own = 1 + if blocks.active {
                self.block_hooks[core].len()
            } else {
                usize::from(self.observer_hooks[core].is_some())
            };
            let wanted = self.block_execution.get()
                && s.entry_points[core].is_empty()
                && !s.memory.watching_writes()
                && self.cpus[core].features().translation_filter
                && self.cpus[core].hook_count() == Some(own);
            (blocks.active, wanted)
        };
        if active != wanted {
            let cpu = &self.cpus[core];
            let data = (&*self.bindings[core] as *const Binding).cast_mut().cast();
            if wanted {
                if let Some(hook) = self.observer_hooks[core].take() {
                    cpu.remove_hook(hook)?;
                    self.hooks.retain(|&(owner, other)| !(owner == core && other == hook));
                }
                // The binding box stays at this address until Board drops its CPU hooks.
                unsafe {
                    // With a translation filter installed the engine restarts
                    // a hooked store or device access as a block of its own.
                    // In Thumb accounting mode it also classifies register-only blocks.
                    cpu.translation_hooks(Some(fujimod_emulator_cpu::TranslationHooks {
                        classify: if cpu.thumb_recovery_accounting().is_some() {
                            block_classify_transfers
                        } else {
                            block_classify
                        },
                        restart: block_restarted,
                        data,
                    }))?;
                    let hook = cpu.install_hook(Hook::Block {
                        callback: block_entered,
                        data,
                        range: HookRange::All,
                    })?;
                    self.block_hooks[core].push(hook);
                    // A device access must end its block. An unwatched hook
                    // over the device pages makes the engine restart it as a
                    // block of its own, for loads and (once the store hook
                    // sees only watched pages) for stores.
                    for &(owner, address, bytes, _) in &self.mmio {
                        if owner == core && bytes != 0 {
                            let hook = cpu.install_memory_restart_boundary(
                                HookRange::Inclusive {
                                    begin: address,
                                    end: address + bytes as u64 - 1,
                                },
                            )?;
                            self.block_hooks[core].push(hook);
                        }
                    }
                }
                self.hooks
                    .extend(self.block_hooks[core].iter().map(|&hook| (core, hook)));
            } else {
                for hook in std::mem::take(&mut self.block_hooks[core]) {
                    cpu.remove_hook(hook)?;
                    self.hooks.retain(|&(owner, other)| !(owner == core && other == hook));
                }
                unsafe {
                    cpu.translation_hooks(None)?;
                    let hook = cpu.install_hook(Hook::Instruction {
                        callback: before,
                        data,
                        range: HookRange::All,
                    })?;
                    self.hooks.push((core, hook));
                    self.observer_hooks[core] = Some(hook);
                }
            }
            cpu.invalidate_code(None)?;
        }
        // In block execution, let the store hook see only watched pages when
        // nothing needs every store of this slice: no aliased view, no
        // reservation whose granule this engine does not watch, and an
        // engine with watched hooks. Otherwise, and outside block execution,
        // the hook sees every store.
        let fast = {
            let s = self.state.borrow();
            match s.mprofile[core].as_ref() {
                Some(profile) => {
                    wanted
                        && self.store_hooks[core].is_some()
                        && self.cpus[core].features().watched_stores
                        && !profile.aliased()
                        && !profile.monitors_need_every_store()?
                }
                None => false,
            }
        };
        let was_fast = self.state.borrow().blocks[core].as_ref().unwrap().fast_stores;
        let mut flushed = active != wanted;
        if fast != was_fast {
            let hook = self.store_hooks[core].ok_or("M-profile store hook is missing")?;
            self.cpus[core].hook_watched(hook, fast)?;
            let watching = self.state.borrow().blocks[core].as_ref().unwrap().watching;
            if fast && !watching {
                // The first store watches on this engine: every translation
                // must pass an observer again, so that the page it came from
                // is watched before it runs. Later, entered pages stay
                // watched in every mode.
                self.cpus[core].invalidate_code(None)?;
                flushed = true;
            }
        }
        let mut s = self.state.borrow_mut();
        if flushed && let Some(shadow) = s.code_shadow[core].as_mut() {
            shadow.discarded();
            shadow.begin_slice();
        }
        if fast && let Some(profile) = s.mprofile[core].as_mut() {
            profile.watch_reservations();
        }
        let inline = fast && s.execution_policy == ExecutionPolicy::Interactive && s.settled.verify == 0;
        let blocks = s.blocks[core].as_mut().unwrap();
        blocks.active = wanted;
        blocks.fast_stores = fast;
        blocks.watching |= fast;
        if flushed {
            blocks.last_watched = None;
        }
        blocks.room = budget;
        blocks.pending = None;
        blocks.resume = false;
        // Anything may have happened since the last run.
        blocks.poll = None;
        blocks.poll_enabled = wanted && self.poll_loops.get();
        Ok((wanted, blocks.inline.begin(&*self.cpus[core], inline)))
    }
    /// Discard a processor's translations when its instruction observer
    /// asked for it. Returns whether the stopped slice must resume.
    fn code_resync(&self, core: usize) -> Result<bool, String> {
        let mut s = self.state.borrow_mut();
        let Some(shadow) = s.code_shadow[core].as_mut() else {
            return Ok(false);
        };
        if !shadow.resync(&*self.cpus[core])? {
            return Ok(false);
        }
        s.settled.clear();
        Ok(true)
    }
    pub fn state(&self) -> Value {
        let bus = self.state.borrow().bus.state();
        self.state_with_bus(bus)
    }
    fn state_with_bus(&self, bus: Value) -> Value {
        let s = self.state.borrow();
        self.track_all_devices(&s.bus);
        let mut value = json!({"execution_mode":s.bus.mode().name(),"execution_policy":s.execution_policy.name(),"counts_as_boot":s.bus.mode()==bus::Mode::Strict,
            "timeline":s.timeline.save(),"instructions":s.instructions,"epoch_instructions":s.epoch_instructions,"power_off":s.power_off,"released":s.released,"waiting":s.waiting,"waiting_event":s.waiting_event,"events":s.events,"next_core":self.next_core.get(),"slice_left":self.slice_left.get(),"quantum":self.quantum.get(),"failure":s.error,
            "memory_decoders":s.decoders.state(),"dma_mappings":s.memory.state(),"gic":s.interrupts.as_ref().map(|g|g.gic.save()),"core_interrupt_devices":s.core_interrupts.iter().map(|c|c.as_ref().map(|c|c.device)).collect::<Vec<_>>(),"outputs":s.outputs,"implementation_cluster":s.implementation_cluster,"unmodeled_system":s.unmodeled_system.iter().map(|(&(core,register,pc),&(reads,writes))|json!({"core":core,"register":register,"pc":pc,"reads":reads,"writes":writes})).collect::<Vec<_>>(),"armv7a":s.armv7a,"aprofile":s.aprofile.iter().map(|p|p.as_ref().map(|p|p.state(s.timeline.now()).unwrap_or_else(|e|json!({"error":e})))).collect::<Vec<_>>(),"mprofile":s.mprofile.iter().map(|p|p.as_ref().map(|p|p.state(s.timeline.now()))).collect::<Vec<_>>()});
        value["bus"] = bus;
        value["discovery_callee_stubs"] = self.discovery_callee_stubs();
        value
    }
    /// Caller-supplied substitutions installed for explicit discovery runs.
    pub fn discovery_callee_stubs(&self) -> Value {
        json!(
            self.discovery_stub_contexts
                .iter()
                .map(|context| context.state())
                .collect::<Vec<_>>()
        )
    }
    /// Surface callback failures instead of treating a failed substitution as
    /// a successful discovery run.
    pub fn check_discovery_callee_stubs(&self) -> Result<(), String> {
        for context in &self.discovery_stub_contexts {
            if let Some(error) = context
                .failure
                .lock()
                .map_err(|_| "discovery callee stub status is poisoned")?
                .as_ref()
            {
                return Err(format!(
                    "discovery callee stub {} at {:#x}: {error}",
                    context.spec.name, context.spec.address
                ));
            }
        }
        Ok(())
    }
    /// Observe the latest scanouts without draining the board's output queue.
    pub fn display_frames(&self) -> Vec<fujimod_emulator_devices::graphics::scanout::Frame> {
        self.state.borrow().frames.values().cloned().collect()
    }
    pub fn take_frames(&mut self) -> Vec<fujimod_emulator_devices::graphics::scanout::Frame> {
        std::mem::take(&mut self.state.borrow_mut().frames)
            .into_values()
            .collect()
    }
    pub fn take_console(&mut self) -> Vec<String> {
        std::mem::take(&mut self.state.borrow_mut().console)
    }
    pub fn finish(&mut self) -> Result<(), String> {
        self.state.borrow_mut().bus.finish()
    }
    pub fn set_scene(&mut self, scene: &Value) -> Result<(), String> {
        let mut s = self.state.borrow_mut();
        s.bus.set_scene(scene)?;
        s.refresh_deadlines()
    }
}
impl Drop for Board {
    fn drop(&mut self) {
        // Host-only execution counters, on request.
        if std::env::var_os("FUJIMOD_EXECUTION_COUNTERS").is_some()
            && let Ok(s) = self.state.try_borrow()
        {
            eprintln!(
                "FUJIMOD execution counters: observer entries settled={} full={} store-hook-installs={}",
                s.settled.skips,
                s.settled.full,
                self.store_hook_installs.get()
            );
            for b in &self.bindings {
                if b.published_hits.get() != 0 {
                    eprintln!("FUJIMOD execution counters: CPU{} MMIO {:#x} published-reads={}",
                        b.core, b.base, b.published_hits.get());
                }
            }
            for (core, shadow) in s.code_shadow.iter().enumerate() {
                if let Some(shadow) = shadow {
                    eprintln!("FUJIMOD execution counters: CPU{core} code {}", shadow.counters());
                }
            }
            for (core, blocks) in s.blocks.iter().enumerate() {
                if let Some(blocks) = blocks {
                    eprintln!(
                        "FUJIMOD execution counters: CPU{core} callbacks instruction={} block={} restart={} store={} mmio={} instructions={}",
                        blocks.instruction_callbacks,
                        blocks.block_callbacks,
                        blocks.restart_callbacks,
                        blocks.store_callbacks,
                        blocks.mmio_callbacks,
                        s.instructions[core],
                    );
                    eprintln!(
                        "FUJIMOD execution counters: CPU{core} blocks observed={} admitted-entries={} shortened={} (unsettled, instruction, run, peer, deadline: {:?}) fast-stores={} watched-pages={} poll-fast-forwards={} poll-reads-left-out={} inline-entries={}",
                        blocks.observed,
                        blocks.admitted,
                        blocks.shortened,
                        blocks.shortened_by,
                        blocks.fast_stores,
                        blocks.watched.len(),
                        blocks.poll_skips,
                        blocks.poll_reads,
                        blocks.inline.entries
                    );
                }
            }
            for (core, skips) in s.polls.skips.iter().enumerate() {
                if *skips != 0 || s.polls.refused[core] != 0 {
                    eprintln!(
                        "FUJIMOD execution counters: CPU{core} poll-fast-forwards={skips} poll-reads-left-out={} rounds-refused={} last-refused(pc, entries, link)={:x?}",
                        s.polls.reads[core],
                        s.polls.refused[core],
                        s.polls.refused_at[core]
                    );
                }
            }
            for (core, cpu) in self.cpus.iter().enumerate() {
                if let Ok(counters) = cpu.exit_counters() {
                    let row = counters
                        .iter()
                        .filter(|(_, n)| **n != 0)
                        .map(|(name, n)| format!("{name}={n}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    eprintln!("FUJIMOD execution counters: CPU{core} engine {row}");
                }
            }
        }
        if let Some(histogram) = &self.access_histogram {
            access_histogram::report(histogram);
        }
        fujimod_emulator_cpu_unicorn::aprofile::store_histogram::report();
        let _ = self.continuation_end();
        let _ = self.release_store_watch();
        // The board exclusively owns processors; bindings hold only Weak
        // handles. CPUs drop before their still-valid callback contexts.
        for (core, hook) in self.hooks.drain(..) {
            let _ = self.cpus[core].remove_hook(hook);
        }
        for (core, base, bytes, _) in self.mmio.drain(..) {
            let _ = self.cpus[core].unmap(base, bytes);
        }
    }
}
/// Host-only diagnostic: with `FUJIMOD_ACCESS_HISTOGRAM` set, count device
/// accesses by processor, address, PC and direction, sample service time, and
/// print the most frequent accesses when a board is dropped.
mod access_histogram {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;
    use std::time::Instant;
    type Counts = HashMap<(usize, u64, u64, bool), u64>;
    type RegisterKey = (usize, u64, bool);
    pub(super) const SERVICE_TIME_SAMPLE_DENOMINATOR: u64 = 1024;

    #[derive(Default)]
    struct RegisterMetrics {
        calls: u64,
        service_samples: u64,
        sampled_service_ns: u64,
        sample_sequence: u64,
    }

    #[derive(Default)]
    pub(super) struct Statistics {
        counts: Counts,
        registers: HashMap<RegisterKey, RegisterMetrics>,
    }

    pub(super) type SharedStatistics = Rc<RefCell<Statistics>>;

    pub(super) fn new() -> Option<SharedStatistics> {
        std::env::var_os("FUJIMOD_ACCESS_HISTOGRAM")
            .map(|_| Rc::new(RefCell::new(Statistics::default())))
    }

    #[cfg(test)]
    pub(super) fn new_for_test() -> SharedStatistics {
        Rc::new(RefCell::new(Statistics::default()))
    }

    pub(super) struct SampledAccess {
        statistics: SharedStatistics,
        key: RegisterKey,
        started: Instant,
    }

    fn sample_this_access(key: RegisterKey, sequence: u64) -> bool {
        // Hash each register's occurrence independently so hot addresses do not
        // align with a fixed global sampling stride.
        let (core, address, write) = key;
        let mut value = address
            ^ (core as u64).wrapping_mul(0x9e3779b97f4a7c15)
            ^ (write as u64).wrapping_mul(0xbf58476d1ce4e5b9)
            ^ sequence.wrapping_mul(0x94d049bb133111eb);
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^= value >> 31;
        value.is_multiple_of(SERVICE_TIME_SAMPLE_DENOMINATOR)
    }

    impl Drop for SampledAccess {
        fn drop(&mut self) {
            let elapsed_ns = u64::try_from(self.started.elapsed().as_nanos()).unwrap_or(u64::MAX);
            let mut statistics = self.statistics.borrow_mut();
            let register = statistics.registers.entry(self.key).or_default();
            register.service_samples = register.service_samples.saturating_add(1);
            register.sampled_service_ns = register.sampled_service_ns.saturating_add(elapsed_ns);
        }
    }

    #[inline]
    pub(super) fn count(
        statistics: &SharedStatistics,
        core: usize,
        address: u64,
        pc: u64,
        write: bool,
    ) -> Option<SampledAccess> {
        let key = (core, address, write);
        let should_sample = {
            let mut statistics = statistics.borrow_mut();
            let calls = statistics.counts.entry((core, address, pc, write)).or_default();
            *calls = calls.saturating_add(1);
            let register = statistics.registers.entry(key).or_default();
            register.calls = register.calls.saturating_add(1);
            register.sample_sequence = register.sample_sequence.wrapping_add(1);
            sample_this_access(key, register.sample_sequence)
        };
        should_sample.then(|| SampledAccess {
            statistics: statistics.clone(),
            key,
            started: Instant::now(),
        })
    }

    pub(super) fn snapshot(
        statistics: &SharedStatistics,
    ) -> Vec<(usize, u64, bool, u64, u64, u64)> {
        statistics
            .borrow()
            .registers
            .iter()
            .map(|(&(core, address, write), metrics)| {
                (
                    core,
                    address,
                    write,
                    metrics.calls,
                    metrics.service_samples,
                    metrics.sampled_service_ns,
                )
            })
            .collect()
    }

    pub(super) fn report(statistics: &SharedStatistics) {
        let mut rows: Vec<_> = std::mem::take(&mut statistics.borrow_mut().counts)
            .into_iter()
            .collect();
        rows.sort_by_key(|&(key, n)| (std::cmp::Reverse(n), key));
        for ((core, address, pc, write), n) in rows.into_iter().take(32) {
            eprintln!(
                "FUJIMOD access histogram: CPU{core} {} {address:#x} pc={pc:#x} count={n}",
                if write { "write" } else { "read" }
            );
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn statistics_are_owned_by_each_board() {
            let first = new_for_test();
            let second = new_for_test();
            drop(count(&first, 0, 0x1000, 0x10, false));
            drop(count(&second, 1, 0x2000, 0x20, true));

            let first = snapshot(&first);
            let second = snapshot(&second);
            assert_eq!(
                first
                    .iter()
                    .map(|row| (row.0, row.1, row.2, row.3))
                    .collect::<Vec<_>>(),
                vec![(0, 0x1000, false, 1)]
            );
            assert_eq!(
                second
                    .iter()
                    .map(|row| (row.0, row.1, row.2, row.3))
                    .collect::<Vec<_>>(),
                vec![(1, 0x2000, true, 1)]
            );
        }
    }
}
fn scalar(cpu: &dyn Cpu, r: Register) -> Result<u64, String> {
    let mut bytes = [0; 8];
    let n = cpu.read_register(r, &mut bytes)?;
    if !matches!(n, 4 | 8) {
        return Err("invalid scalar register width".into());
    }
    Ok(u64::from_le_bytes(bytes))
}
fn set(cpu: &dyn Cpu, r: Register, v: u64) -> Result<(), String> {
    let mut bytes = [0; 8];
    let n = cpu.read_register(r, &mut bytes)?;
    if n == 4 && v > u32::MAX as u64 {
        return Err("value exceeds architectural register width".into());
    }
    cpu.write_register(r, &v.to_le_bytes()[..n])
}
impl State {
    fn observe_store_reason(&mut self, core: usize, reason: StoreObserverReason) {
        if let Some(counters) = self
            .store_observer
            .as_mut()
            .and_then(|counters| counters.get_mut(core))
        {
            counters.observe_reason(reason);
        }
    }

    fn finish_store_observation(&mut self, core: usize, reasons: u8, complete: bool) {
        if let Some(counters) = self
            .store_observer
            .as_mut()
            .and_then(|counters| counters.get_mut(core))
        {
            counters.finish_observation(reasons, complete);
        }
    }

    fn fail_store_observation_before_classification(&mut self, core: usize) {
        if let Some(counters) = self
            .store_observer
            .as_mut()
            .and_then(|counters| counters.get_mut(core))
        {
            counters.failed_before_classification =
                counters.failed_before_classification.saturating_add(1);
        }
    }

    fn finish_exit(
        &mut self,
        cpu: &dyn Cpu,
        core: usize,
        pc: u64,
        exit: Exit,
    ) -> Result<(), String> {
        self.polls.last[core] = None;
        match exit {
            Exit::BeforeInstruction(word) => {
                let pc = scalar(cpu, Register::Pc)?;
                match cpu.model() {
                    fujimod_emulator_cpu::Model::ArmV8A => {
                        self.aarch64_instruction(cpu, core, pc, word)?;
                    }
                    fujimod_emulator_cpu::Model::CortexA7 => {
                        // Architectural counters follow the core's own position.
                        let now = self.timeline.processor_clock(core)?.now;
                        let profile = self.armv7a[core]
                            .as_mut()
                            .ok_or("ARMv7-A framework instruction without a profile")?;
                        match profile.before_counted(cpu, pc, now, word)? {
                            // A failed condition retires the instruction unexecuted.
                            armv7a::Entry::Execute => set(cpu, Register::Pc, pc.wrapping_add(4))?,
                            armv7a::Entry::Completed => (),
                            armv7a::Entry::Hint(hint) => self.hint(core, hint),
                        }
                    }
                    _ => return Err("unsupported framework instruction exit".into()),
                }
                self.account(core, 1)?;
                self.instructions[core] = self.instructions[core]
                    .checked_add(1).ok_or("instruction counter overflow")?;
            }
            Exit::Stopped => (),
            Exit::Hint(hint) => self.hint(core, hint),
            Exit::Trap(t) if t.syndrome >> 26 == 1 => {
                set(
                    cpu,
                    Register::Pc,
                    scalar(cpu, Register::Pc)?.wrapping_add(4),
                )?;
                self.hint(
                    core,
                    if t.syndrome & 1 != 0 {
                        fujimod_emulator_cpu::Hint::WaitForEvent
                    } else {
                        fujimod_emulator_cpu::Hint::WaitForInterrupt
                    },
                );
            }
            Exit::Trap(t) if t.syndrome >> 26 == 0x18 => {
                self.system_register(cpu, core, t.syndrome)?
            }
            other => return Err(format!("unhandled CPU{core} exit at {pc:#x}: {other:?}")),
        }
        Ok(())
    }
    /// Account the admitted block of a processor in block execution: all of
    /// it, or the `entered` entries of it when the engine restarted an
    /// instruction. The unentered rest returns to the run's budget.
    fn settle_block(&mut self, core: usize, entered: Option<u64>) -> Result<(), String> {
        let Some(blocks) = self.blocks[core].as_mut() else {
            return Ok(());
        };
        let inline = blocks.inline.take()?;
        let admitted = blocks.pending.take().map_or(0, |(_, _, entries, _)| entries);
        if inline != 0 && entered.is_some() {
            return Err("restarted observed block after translated Thumb entries".into());
        }
        let observed = entered.unwrap_or(admitted).min(admitted);
        blocks.room += admitted - observed;
        blocks.room = blocks.room.checked_sub(inline).ok_or("Thumb block exceeded run budget")?;
        let entered = observed + inline;
        if entered == 0 {
            return Ok(());
        }
        blocks.admitted += entered;
        self.account(core, entered)?;
        self.instructions[core] = self.instructions[core]
            .checked_add(entered)
            .ok_or("instruction counter overflow")?;
        Ok(())
    }
    /// Stop inside either the last translated block or the observed block
    /// preceding the grant. A faulting data access has entered its instruction.
    /// An isolated retry will enter it again, so only its prefix counts here.
    fn settle_interrupted_block(
        &mut self,
        cpu: &dyn Cpu,
        core: usize,
        pc: u64,
        include_instruction: bool,
    ) -> Result<(), String> {
        let Some(blocks) = self.blocks[core].as_ref() else { return Ok(()) };
        let charged = blocks.inline.charged_block();
        let pending = blocks.pending.map(|(address, bytes, entries, _)| (address, bytes, entries));
        let Some((address, bytes, entries)) = charged.or(pending) else { return Ok(()) };
        let offset = pc.checked_sub(address).filter(|offset| *offset < u64::from(bytes))
            .ok_or("interrupted Thumb instruction is outside its block")?;
        let prefix = if offset == 0 {
            0
        } else {
            let mut buffer = Vec::new();
            mprofile_blocks::count_entries(self.block_code(cpu, core, address, offset as usize, &mut buffer)?)?
        };
        let entered = prefix + u64::from(include_instruction);
        if charged.is_some() {
            let tail = entries.checked_sub(entered).ok_or("invalid interrupted Thumb instruction count")?;
            self.blocks[core].as_mut().unwrap().inline.refund_tail(tail)?;
            self.settle_block(core, None)
        } else {
            self.settle_block(core, Some(entered))
        }
    }

    /// The Thumb code of a block, from host memory when the code shadow has
    /// the page, else through the engine.
    fn block_code<'a>(
        &'a self,
        cpu: &dyn Cpu,
        core: usize,
        address: u64,
        bytes: usize,
        buffer: &'a mut Vec<u8>,
    ) -> Result<&'a [u8], String> {
        if let Some(code) = self.code_shadow[core]
            .as_ref()
            .and_then(|shadow| shadow.code(address, bytes))
        {
            return Ok(code);
        }
        buffer.resize(bytes, 0);
        cpu.read_memory(address, buffer)?;
        Ok(buffer)
    }
    /// Whether a whole page at `address` is checked shared RAM of an
    /// M-profile processor.
    fn code_page_checked(profile: &Option<mprofile::Profile>, address: u64) -> bool {
        profile
            .as_ref()
            .is_some_and(|profile| profile.physical(address, 4096).is_ok())
    }
    /// Once store watches exist on a processor's engine, the engine detects
    /// stores into translated code only on watched pages. Watch the page an
    /// instruction is entered from, and the next (a translation may end
    /// there), before it runs.
    #[inline]
    fn watch_code(&mut self, cpu: &dyn Cpu, core: usize, pc: u64) -> Result<(), String> {
        let Some(blocks) = self.blocks[core].as_mut().filter(|blocks| blocks.watching) else {
            return Ok(());
        };
        let page = pc & !0xfff;
        if blocks.last_watched == Some(page) {
            return Ok(());
        }
        blocks.last_watched = Some(page);
        for page in [Some(page), page.checked_add(0x1000)].into_iter().flatten() {
            if page <= u32::MAX as u64 - 0xfff && blocks.watched.insert(page) {
                cpu.watch_stores(page, 0x1000)?;
            }
        }
        Ok(())
    }
    /// Close a slice of the code shadow at the processor's stop PC.
    fn end_code_slice(&mut self, cpu: &dyn Cpu, core: usize) {
        if let Ok(pc) = scalar(cpu, Register::Pc)
            && let Err(error) = self.watch_code(cpu, core, pc & !1)
        {
            self.fail(error);
        }
        let Self {
            code_shadow,
            code_run,
            mprofile,
            ..
        } = self;
        if let Some(shadow) = code_shadow[core].as_mut() {
            let profile = &mprofile[core];
            shadow.end_slice(cpu, scalar(cpu, Register::Pc).ok().map(|pc| pc & !1), |address| {
                Self::code_page_checked(profile, address)
            });
            // The copies just taken include any bytes a device, the host or
            // an alias store wrote during this slice, which the engine did
            // not track. Those translations can only be discarded.
            if *code_run != Some(core) {
                shadow.invalidate();
            }
        }
    }
    /// Prove the code about to be entered at `pc`, or stop the slice so the
    /// owner discards the translations first. Returns whether to stop.
    fn enter_code(&mut self, cpu: &dyn Cpu, core: usize, pc: u64) -> Result<bool, String> {
        self.watch_code(cpu, core, pc)?;
        let Self {
            code_shadow,
            mprofile,
            ..
        } = self;
        let Some(shadow) = code_shadow[core].as_mut() else {
            return Ok(false);
        };
        if shadow.changed() {
            // A store of this processor found its page changed behind the
            // engine. Nothing stale may be entered.
            shadow.request_resync(None);
            cpu.stop()?;
            return Ok(true);
        }
        let profile = &mprofile[core];
        match shadow.touch(cpu, pc & !1, |address| Self::code_page_checked(profile, address)) {
            code_shadow::Touch::Proven => Ok(false),
            code_shadow::Touch::Unchecked => {
                shadow.entered_unchecked();
                Ok(false)
            }
            code_shadow::Touch::Changed(page) => {
                shadow.request_resync(Some(page));
                cpu.stop()?;
                Ok(true)
            }
        }
    }
    fn apply_memory(&mut self, cpus: &[Rc<dyn Cpu>]) -> Result<(), String> {
        if self.decoders.pending() {
            self.code_run = None;
            for shadow in self.code_shadow.iter_mut().flatten() {
                shadow.invalidate();
            }
        }
        self.decoders.apply(cpus, &self.memory)
    }
    fn runnable_peer(&self, core: usize) -> bool {
        self.released
            .iter()
            .enumerate()
            .any(|(c, &released)| c != core && released && !self.waiting[c])
    }
    fn scheduler_room(&self, core: usize) -> u64 {
        if self.parallel.is_some() {
            return u64::MAX;
        }
        if !self.runnable_peer(core) {
            return u64::MAX;
        }
        self.active_slice
            .filter(|slice| slice.core == core)
            .map_or(u64::MAX, |slice| slice.room(self.instructions[core]))
    }
    fn total_instructions(&self) -> Result<u64, String> {
        self.instructions.iter().try_fold(0u64, |sum, n| {
            sum.checked_add(*n)
                .ok_or("board instruction count overflow".into())
        })
    }
    fn hint(&mut self, core: usize, hint: fujimod_emulator_cpu::Hint) {
        use fujimod_emulator_cpu::Hint;
        self.settled.clear();
        self.polls.last[core] = None;
        match hint {
            Hint::WaitForInterrupt => {
                self.waiting[core] = true;
                self.waiting_event[core] = false;
            }
            Hint::WaitForEvent => {
                self.waiting[core] = !self.events[core];
                self.waiting_event[core] = self.waiting[core];
                self.events[core] = false;
            }
            Hint::SendEvent | Hint::SendEventLocal => {
                for c in 0..self.events.len() {
                    if hint == Hint::SendEventLocal && c != core {
                        continue;
                    }
                    self.events[c] = true;
                    if self.waiting_event[c] {
                        self.waiting[c] = false;
                        self.waiting_event[c] = false;
                        // An event that completes a waiting WFE is consumed by it.
                        self.events[c] = false;
                    }
                }
            }
        }
    }
    fn fail(&mut self, reason: String) {
        if self.error.is_none() {
            self.error = Some(reason);
        }
    }
    fn sync(&mut self, cpu: &dyn Cpu, core: usize) -> Result<(), String> {
        if let Some(progress) = cpu.instruction_progress() {
            let delta = progress.checked_sub(self.accounted[core]).ok_or_else(|| {
                format!("CPU{core} entry count moved backward: {progress} < {} (policy {}, parallel {})",
                    self.accounted[core], self.execution_policy.name(), self.parallel.is_some())
            })?;
            match self.parallel.as_mut() {
                Some(epoch) => {
                    let elapsed = epoch.advance(core, delta)?;
                    let boundary = self.timeline.ratio().budget(
                        self.timeline.now(),
                        self.timeline.phase(),
                        self.clock_limit.unwrap_or(u64::MAX),
                    )?;
                    let elapsed = self.timeline.budget(elapsed.min(boundary))?;
                    self.timeline.advance_instructions(elapsed)?;
                }
                None if self.execution_policy == ExecutionPolicy::Interactive => {
                    // Interactive runs may overshoot by a block; the frontier
                    // stops at the next deadline or run boundary.
                    let room =
                        self.timeline
                            .processor_budget(core, u64::MAX, self.clock_limit)?;
                    self.account(core, delta.min(room))?;
                }
                None => self.account(core, delta)?,
            }
            self.instructions[core] = self.instructions[core]
                .checked_add(delta)
                .ok_or("instruction counter overflow")?;
            self.accounted[core] = progress;
        } else if cpu.features().instruction_progress {
            return Err("processor entry count is unavailable".into());
        }
        Ok(())
    }
    /// Account one processor's entries on its own position; runnable peers
    /// fall behind the frontier it advances.
    fn account(&mut self, core: usize, entries: u64) -> Result<(), String> {
        let (released, waiting) = (&self.released, &self.waiting);
        self.timeline
            .advance_processor(core, entries, |c| released[c] && !waiting[c])
    }
    /// Stopped and waiting processors idle at the frontier.
    fn park(&mut self) {
        let (released, waiting) = (&self.released, &self.waiting);
        self.timeline.park(|c| released[c] && !waiting[c]);
    }
    fn refresh_deadlines(&mut self) -> Result<(), String> {
        for (id, at) in self.bus.changed_deadlines(self.timeline.now()) {
            if at == self.deadlines[id] {
                continue;
            }
            self.generations[id] = self.generations[id]
                .checked_add(1)
                .ok_or("device deadline generation exhausted")?;
            // A deadline that just fired was removed by pop_due.
            if std::mem::replace(&mut self.deadlines[id], at).is_some() {
                self.timeline.cancel_target(Target::DeviceDeadline(id));
            }
            if let Some(at) = at {
                self.timeline
                    .schedule(Target::DeviceDeadline(id), at, self.generations[id])?;
            }
        }
        Ok(())
    }
    fn refresh_processors(&mut self) -> Result<(), String> {
        for core in 0..self.mprofile.len() {
            let at = if self.released[core] {
                self.mprofile[core]
                    .as_ref()
                    .map(|p| p.next_deadline())
                    .transpose()?
                    .flatten()
                    .into_iter()
                    .chain(
                        self.aprofile[core]
                            .as_ref()
                            .map(|p| p.next_deadline(self.timeline.now()))
                            .transpose()?
                            .flatten(),
                    )
                    .min()
            } else {
                None
            };
            if at == self.processor_deadlines[core] {
                continue;
            }
            self.processor_generations[core] = self.processor_generations[core]
                .checked_add(1)
                .ok_or("processor deadline generation exhausted")?;
            self.processor_deadlines[core] = at;
            self.timeline.cancel_target(Target::ProcessorDeadline(core));
            if let Some(at) = at {
                self.timeline.schedule(
                    Target::ProcessorDeadline(core),
                    at,
                    self.processor_generations[core],
                )?;
            }
        }
        self.release_state_changed = false;
        Ok(())
    }
    fn due(&mut self, cpus: &[Rc<dyn Cpu>]) -> Result<(), String> {
        if self.decoders.pending() {
            return Ok(());
        }
        // Retain a newly created earlier boundary even after its event is
        // consumed. Unfinished peers may catch up only to this cohort's cap.
        if let Some(epoch) = self.parallel.as_mut() {
            epoch.constrain(&self.timeline, self.clock_limit)?;
        }
        let mut count = 0;
        while let Some(event) = self.timeline.pop_due() {
            self.settled.clear();
            count += 1;
            if count > 1_000_000 {
                return Err("device events fail to advance time".into());
            }
            let (id, token) = match event.target {
                Target::DeviceEvent(id) => (id, Some(event.token)),
                Target::DeviceDeadline(id) => {
                    if self.generations.get(id) != Some(&event.token) {
                        continue;
                    }
                    self.deadlines[id] = None;
                    (id, None)
                }
                Target::ProcessorDeadline(core) => {
                    if self.processor_generations.get(core) != Some(&event.token) {
                        continue;
                    }
                    self.processor_deadlines[core] = None;
                    if let Some(p) = self.mprofile[core].as_mut() {
                        p.tick(self.timeline.now())?;
                    } else if self.aprofile[core].is_none() {
                        return Err("processor deadline has no architectural controller".into());
                    }
                    self.refresh_processors()?;
                    continue;
                }
            };
            self.with_host(cpus, |bus, host| bus.event(host, id, token))?;
            self.refresh_deadlines()?;
            if self.release_state_changed {
                self.refresh_processors()?;
            }
        }
        Ok(())
    }
    fn with_host<T>(
        &mut self,
        cpus: &[Rc<dyn Cpu>],
        f: impl FnOnce(&mut Bus, &mut Services<'_>) -> Result<T, String>,
    ) -> Result<T, String> {
        let Self {
            execution_policy,
            device_workers,
            host_pause,
            power_off,
            bus,
            memory,
            timeline,
            interrupts,
            core_interrupts,
            interrupt_changed,
            released,
            release_state_changed,
            waiting,
            error,
            records,
            irq_observations,
            console,
            frames,
            outputs,
            mprofile,
            exclusive,
            code_run,
            code_write_deferred,
            decoders,
            active_slice,
            memory_access,
            settled,
            ..
        } = self;
        settled.clear();
        let mut host = Services {
            execution_policy: *execution_policy,
            device_workers: *device_workers,
            host_pause,
            power_off,
            memory,
            timeline,
            interrupts,
            core_interrupts,
            interrupt_changed,
            released,
            release_state_changed,
            waiting,
            error,
            records,
            irq_observations,
            console,
            frames,
            outputs,
            mprofile,
            exclusive,
            code_run,
            code_write_deferred,
            cpus,
            decoders,
            active_core: active_slice.map(|s| s.core),
            memory_access: *memory_access,
        };
        let result = f(bus, &mut host);
        if self::continuation::fired(&self.continuation)
            && let Some(core) = host.active_core
        {
            continuation::finish_instruction(&*cpus[core])?;
        }
        if bus.needs_frontier_pcs() {
            let pcs = cpus
                .iter()
                .map(|cpu| scalar(&**cpu, Register::Pc))
                .collect::<Result<Vec<_>, _>>()?;
            bus.capture_frontier_pcs(&pcs);
        }
        let result = result?;
        if let Some(error) = host.error.take() {
            return Err(error);
        }
        for (core, changed) in host.interrupt_changed.iter_mut().enumerate() {
            if std::mem::take(changed) {
                bus.inputs_changed(host.core_interrupts[core].as_ref().unwrap().device)?;
            }
        }
        Ok(result)
    }
    fn push_pins(&mut self, cpus: &[Rc<dyn Cpu>]) -> Result<bool, String> {
        let mut changed = false;
        // Whether this push left every value it touches as it found it. A
        // concurrent window changes these values from other threads, so only
        // an ordered slice, under either execution policy, can be a fixed point.
        let mut settled = self.parallel.is_none();
        self.settled.pass = false;
        for (core, controller) in self.core_interrupts.iter().enumerate() {
            if let Some(controller) = controller {
                // Device-owned controller inputs are not tracked here.
                settled = false;
                let high = self.interrupt_delivery[core] && controller.inputs.irq();
                if self.pins[core] != (high, false) {
                    if cpus[core].features().architectural_exits {
                        cpus[core].set_interrupt(Interrupt::Irq, high)?;
                    }
                    self.pins[core] = (high, false);
                    changed = true;
                }
                if high {
                    self.waiting[core] = false;
                }
            }
        }
        for (core, p) in self.aprofile.iter().enumerate() {
            if let Some(p) = p {
                for (line, level) in p.outputs(self.timeline.now())? {
                    self.interrupts
                        .as_mut()
                        .ok_or("architectural timer has no interrupt controller")?
                        .gic
                        .set_ppi(core, line as usize, level)?;
                }
            }
        }
        if let Some(g) = &self.interrupts {
            for core in 0..g.cores {
                let pins = g.gic.pins(core)?;
                if pins != self.pins[core] {
                    if cpus[core].features().architectural_exits {
                        cpus[core].set_interrupt(Interrupt::Irq, pins.0)?;
                        cpus[core].set_interrupt(Interrupt::Fiq, pins.1)?;
                    }
                    self.pins[core] = pins;
                    changed = true;
                }
                if self.parked_until_sgi[core] && g.gic.sgi_pending(core)? {
                    self.parked_until_sgi[core] = false;
                    self.waiting[core] = false;
                    changed = true;
                }
                if pins.0 || pins.1 {
                    settled &= !self.waiting[core] && !self.waiting_event[core] && self.events[core];
                    self.waiting[core] = false;
                    if self.waiting_event[core] {
                        self.waiting_event[core] = false;
                        self.events[core] = false;
                    } else {
                        self.events[core] = true;
                    }
                }
            }
        }
        for (core, p) in self.mprofile.iter_mut().enumerate() {
            if let Some(p) = p
                && self.released[core]
                && p.pending()
            {
                settled &= !self.waiting[core];
                self.waiting[core] = false;
            }
        }
        self.settled.pass = settled && !changed;
        Ok(changed)
    }
    /// Whether the instruction observer may skip its pin push and deadline
    /// refresh: both are a fixed point and guest time is before the next
    /// timer output change.
    #[inline]
    fn settled(&mut self) -> bool {
        if self.settled.armed && self.timeline.now() >= self.settled.until {
            self.settled.clear();
        }
        self.settled.armed
    }
    /// After a full pin push that changed nothing and a full deadline
    /// refresh: record until when both stay a fixed point.
    fn arm_settled(&mut self) -> Result<(), String> {
        if !self.settled.pass {
            return Ok(());
        }
        let now = self.timeline.now();
        let mut until = u64::MAX;
        for p in self.aprofile.iter().flatten() {
            if let Some(at) = p.next_deadline(now)? {
                until = until.min(at);
            }
        }
        self.settled.until = until;
        self.settled.armed = until > now;
        Ok(())
    }
    /// Recompute the pending scan a settled observer skipped for an idle
    /// M-profile controller and refuse a ready exception.
    fn verify_idle(&mut self, core: usize, idle: bool) -> Result<(), String> {
        if idle
            && let Some(p) = self.mprofile[core].as_mut()
            && p.pending()
        {
            return Err("settled M-profile controller has a ready exception".into());
        }
        Ok(())
    }
    /// Recompute what a settled observer skipped and refuse any difference.
    fn verify_settled(&mut self, cpus: &[Rc<dyn Cpu>], pins: bool) -> Result<(), String> {
        let saved = (self.settled.armed, self.settled.until);
        let observe = |s: &Self| {
            (
                s.pins.clone(),
                s.waiting.clone(),
                s.waiting_event.clone(),
                s.events.clone(),
                s.processor_deadlines.clone(),
                s.processor_generations.clone(),
                s.interrupts.as_ref().map(|g| g.gic.save()),
                s.mprofile
                    .iter()
                    .map(|p| p.as_ref().map(|p| p.state(s.timeline.now())))
                    .collect::<Vec<_>>(),
                s.timeline.save(),
            )
        };
        let before = observe(self);
        let changed = if pins {
            self.push_pins(cpus)?
        } else {
            self.refresh_processors()?;
            false
        };
        if changed || before != observe(self) {
            return Err("settled interrupt bookkeeping changed state".into());
        }
        self.settled.pass = false;
        (self.settled.armed, self.settled.until) = saved;
        Ok(())
    }
    fn tighten(&self, cpu: &dyn Cpu, core: usize) -> Result<(), String> {
        if cpu.features().tighten_instruction_limit {
            let entered = cpu
                .instruction_progress()
                .ok_or("active entry count is unavailable")?;
            let remaining = match self.parallel.as_ref() {
                Some(epoch) => epoch.room(core),
                None => self.timeline.processor_budget(
                    core,
                    self.scheduler_room(core),
                    self.clock_limit,
                )?,
            };
            // Entries a poll loop fast-forward accounted are not in the
            // engine's count: the run's own budget ends that much earlier.
            let remaining = if self.polls.active[core] {
                remaining.min(self.polls.run_end[core].saturating_sub(self.instructions[core]))
            } else {
                remaining
            };
            cpu.tighten_instruction_limit(entered.saturating_add(remaining))?;
        }
        Ok(())
    }
    fn access(
        &mut self,
        cpus: &[Rc<dyn Cpu>],
        binding: &Binding,
        address: u64,
        size: u8,
        value: Option<u64>,
    ) -> Result<u64, String> {
        let core = binding.core;
        if self.decoders.pending() {
            return Err("MMIO requires committed memory decoding".into());
        }
        self.settled.clear();
        // Block execution: a device access is the last instruction of its
        // block, so the whole block has been entered.
        self.settle_block(core, None)?;
        // Any access ends a poll loop candidate, unless it is the loop's own
        // read coming round again (`poll_round`).
        let poll = self.blocks[core].as_mut().and_then(|blocks| blocks.poll.take());
        let poll64 = self.polls.last[core].take();
        self.sync(&*cpus[core], core)?;
        self.due(cpus)?;
        let pc = scalar(&*cpus[core], Register::Pc)?;
        let _access_sample = self.access_histogram.as_ref().and_then(|histogram| {
            access_histogram::count(histogram, core, address, pc, value.is_some())
        });
        if let Some(p) = &mut self.mprofile[core]
            && (0xe0000000..0xe0100000).contains(&address)
        {
            let result = p.access(&*cpus[core], address, size, value, self.timeline.now())?;
            self.refresh_processors()?;
            self.push_pins(cpus)?;
            self.settle_after_pin_push(core)?;
            self.tighten(&*cpus[core], core)?;
            return Ok(result);
        }
        if let Some(g) = &mut self.interrupts
            && let Some(result) = g.access(core, address, size, value) {
            let result = result?;
            self.push_pins(cpus)?;
            self.tighten(&*cpus[core], core)?;
            return Ok(result);
        }
        self.memory_access = value.is_some();
        self.timeline.synchronize(core)?;
        let use_publication = value.is_none()
            && !binding.published_reads.is_empty()
            && self.execution_policy == ExecutionPolicy::Interactive
            && self.bus.loop_window();
        let result = self.with_host(cpus, |bus, host| {
            if use_publication {
                let tick = host.now();
                let offset = address - binding.base;
                if let Some(word) = binding.published_reads.iter().find_map(|read|
                    read.published_at(offset, u32::from(size), Some(tick)))
                {
                    bus.finish_published_read(host, address, size)?;
                    binding.published_hits.set(binding.published_hits.get() + 1);
                    return Ok(word);
                }
            }
            bus.access(host, core, pc, address, size, value)
        });
        self.memory_access = false;
        let result = result?;
        self.refresh_deadlines()?;
        self.refresh_processors()?;
        self.due(cpus)?;
        self.push_pins(cpus)?;
        self.settle_after_pin_push(core)?;
        if value.is_none() {
            self.poll_round(&*cpus[core], core, pc & !1, address, size, result, poll)?;
            self.poll_round_a64(&*cpus[core], core, pc, address, size, result, poll64)?;
        }
        self.tighten(&*cpus[core], core)?;
        Ok(result)
    }
    /// After a device read of an AArch64 processor: recognise a poll loop
    /// that came round unchanged (`poll_loops`) and account the rounds that
    /// must follow, up to one entry before the next device event, the peer's
    /// turn or the end of the run. Their reads are left out; the engine's
    /// own count does not hold these entries, so the caller's `tighten`
    /// lowers its limit by them.
    #[allow(clippy::too_many_arguments)]
    fn poll_round_a64(
        &mut self,
        cpu: &dyn Cpu,
        core: usize,
        pc: u64,
        address: u64,
        size: u8,
        value: u64,
        last: Option<poll_loops::Poll>,
    ) -> Result<(), String> {
        if !self.polls.active[core] || self.error.is_some() {
            return Ok(());
        }
        let instructions = self.instructions[core];
        let last = last.filter(|p| p.pc == pc && p.address == address && p.size == size);
        let mut poll = poll_loops::Poll {
            pc,
            address,
            size,
            stable: match last {
                Some(last) => last.stable,
                None => self.bus.poll_stable(address, size),
            },
            value,
            instructions,
            registers: None,
            proven: last.and_then(|p| p.proven),
        };
        if let Some(last) = last
            && poll.stable
            && last.value == value
            && self.pins[core] == (false, false)
            && let Some(steps) = instructions.checked_sub(last.instructions)
            && (2..=poll_loops::LONGEST_ROUND).contains(&steps)
        {
            let mut registers = [0; 33];
            for (index, slot) in registers.iter_mut().enumerate() {
                *slot = scalar(
                    cpu,
                    match index {
                        0..=30 => Register::X(index as u8),
                        31 => Register::Sp,
                        _ => Register::Pstate,
                    },
                )?;
            }
            poll.registers = Some(registers);
            if last.registers == Some(registers) {
                let link = registers[30];
                let pure = match poll.proven {
                    Some((length, at, pure)) if length == steps && at == link => pure,
                    _ => {
                        let mut read = |address: u64| {
                            let mut word = [0; 4];
                            cpu.read_memory(address, &mut word)
                                .ok()
                                .map(|()| u32::from_le_bytes(word))
                        };
                        let pure = poll_loops::round_is_pure(&mut read, pc, steps, link);
                        poll.proven = Some((steps, link, pure));
                        if !pure {
                            self.polls.refused[core] += 1;
                            self.polls.refused_at[core] = Some((pc, steps, link));
                        }
                        pure
                    }
                };
                // One entry less than the run may still begin: a read left
                // out must end before the next deadline (see `poll_round`).
                let rounds = if pure {
                    self.timeline
                        .processor_budget(core, self.scheduler_room(core), self.clock_limit)?
                        .min(self.polls.run_end[core].saturating_sub(instructions))
                        .saturating_sub(1)
                        / steps
                } else {
                    0
                };
                if rounds != 0 {
                    let entries = rounds * steps;
                    self.account(core, entries)?;
                    self.instructions[core] = instructions
                        .checked_add(entries)
                        .ok_or("instruction counter overflow")?;
                    poll.instructions = self.instructions[core];
                    self.bus.left_out_reads(address, size, rounds);
                    self.polls.skipped_reads += rounds;
                    self.polls.skips[core] += 1;
                    self.polls.reads[core] += rounds;
                }
            }
        }
        self.polls.last[core] = Some(poll);
        Ok(())
    }
    /// After a device read of an M-profile processor in block execution:
    /// recognise a poll loop that came round unchanged and account the rounds
    /// that must follow, up to the next device event, the peer's turn or the
    /// end of the run (`mprofile_blocks::Poll`). Their reads are left out;
    /// the processor then executes the rest of the last round itself.
    #[allow(clippy::too_many_arguments)]
    fn poll_round(
        &mut self,
        cpu: &dyn Cpu,
        core: usize,
        pc: u64,
        address: u64,
        size: u8,
        value: u64,
        last: Option<mprofile_blocks::Poll>,
    ) -> Result<(), String> {
        if !self.blocks[core]
            .as_ref()
            .is_some_and(|blocks| blocks.active && blocks.poll_enabled)
            || self.error.is_some()
        {
            return Ok(());
        }
        let instructions = self.instructions[core];
        let last = last.filter(|p| p.pc == pc && p.address == address && p.size == size);
        let period = match last {
            Some(last) => last.period,
            None if self.bus.poll_stable(address, size) => {
                let mut buffer = Vec::new();
                self.block_code(cpu, core, pc, 34, &mut buffer)
                    .ok()
                    .and_then(mprofile_blocks::poll_period)
            }
            None => None,
        };
        let mut poll = mprofile_blocks::Poll {
            pc,
            address,
            size,
            period,
            value,
            instructions,
            registers: None,
        };
        // One whole round since the last read, the same value, and nothing
        // ready that the next block entry would take.
        let round = match (last, period) {
            (Some(last), Some(period))
                if last.value == value
                    && last.instructions.checked_add(period) == Some(instructions)
                    && self.settled.armed
                    && self.settled.idle == Some(core) =>
            {
                Some((last.registers, period))
            }
            _ => None,
        };
        if let Some((before, period)) = round {
            let mut registers = [0; 16];
            for (index, slot) in registers.iter_mut().enumerate() {
                *slot = scalar(
                    cpu,
                    Register::Arm32(if index < 15 {
                        fujimod_emulator_cpu::arm::Register::R(index as u8)
                    } else {
                        fujimod_emulator_cpu::arm::Register::Cpsr
                    }),
                )?;
            }
            poll.registers = Some(registers);
            if before == Some(registers) {
                // The bound of a block admission, less one entry. An admitted
                // entry begins before the next deadline. A read happens after
                // its entry is accounted, so a read left out must also end
                // before that deadline: at the deadline itself the device
                // event comes first and the read could return a new value.
                let until = self.clock_limit.unwrap_or(u64::MAX).min(self.settled.until);
                let room = self.blocks[core].as_ref().map_or(0, |blocks| blocks.room);
                let rounds = room
                    .min(self.timeline.processor_budget(core, u64::MAX, Some(until))?)
                    .min(self.scheduler_room(core))
                    .saturating_sub(1)
                    / period;
                if rounds != 0 {
                    let entries = rounds * period;
                    self.account(core, entries)?;
                    self.instructions[core] = self.instructions[core]
                        .checked_add(entries)
                        .ok_or("instruction counter overflow")?;
                    poll.instructions = self.instructions[core];
                    let blocks = self.blocks[core].as_mut().unwrap();
                    blocks.room -= entries;
                    blocks.admitted += entries;
                    blocks.skipped_reads += rounds;
                    blocks.poll_skips += 1;
                    self.bus.left_out_reads(address, size, rounds);
                    blocks.poll_reads += rounds;
                }
            }
        }
        self.blocks[core].as_mut().unwrap().poll = Some(poll);
        Ok(())
    }
    /// Called after processor deadlines have been refreshed and pins pushed.
    /// A fixed-point pin push leaves the timer inputs and clock unchanged,
    /// so the next instruction can reuse the refreshed deadlines.
    fn settle_after_pin_push(&mut self, core: usize) -> Result<(), String> {
        if !self.settled.pass || self.mprofile[core].is_none() || self.decoders.pending() {
            return Ok(());
        }
        self.arm_settled()?;
        if self.settled.armed
            && let Some(p) = self.mprofile[core].as_mut()
            && !p.pending()
        {
            // The push above already ran this pending scan; no exception is
            // ready, as the next observer would find.
            self.settled.idle = Some(core);
        }
        Ok(())
    }
    fn system_register(&mut self, cpu: &dyn Cpu, core: usize, syndrome: u64) -> Result<(), String> {
        self.settled.clear();
        self.polls.last[core] = None;
        let r = fujimod_emulator_cpu::arm::SystemRegister {
            op0: ((syndrome >> 20) & 3) as u8,
            op1: ((syndrome >> 14) & 7) as u8,
            crn: ((syndrome >> 10) & 15) as u8,
            crm: ((syndrome >> 1) & 15) as u8,
            op2: ((syndrome >> 17) & 7) as u8,
        }
        .encoding()?;
        let rt = ((syndrome >> 5) & 31) as u8;
        let read = syndrome & 1 != 0;
        let value = if read || rt == 31 {
            None
        } else {
            Some(scalar(cpu, Register::X(rt))?)
        };
        let write = if read { None } else { Some(value.unwrap_or(0)) };
        let value = if r == 0xc230
            || r == 0xe64d
            || r == 0xf665
            || r == 0xf667
            || (r >> 14 == 3
                && (r >> 11) & 7 == 0
                && (r >> 7) & 15 == 12
                && (8..=15).contains(&((r >> 3) & 15)))
        {
            Some(
                self.interrupts
                    .as_mut()
                    .ok_or("ICC access without interrupt controller")?
                    .gic
                    .system(core, r, write)?,
            )
        } else if let Some(p) = self.aprofile[core].as_mut() {
            p.access(r, self.timeline.now(), write)?
                .or(p.implementation_access(&mut self.implementation_cluster, r, write)?)
        } else {
            None
        };
        let value = match value {
            Some(v) => v,
            None => {
                let pc = scalar(cpu, Register::Pc)?;
                let entry = self.unmodeled_system.entry((core, r, pc)).or_default();
                if read {
                    entry.0 += 1;
                } else {
                    entry.1 += 1;
                }
                if self.bus.mode() == bus::Mode::Strict {
                    return Err(format!("unmodeled architectural register {r:#x}"));
                }
                0
            }
        };
        self.refresh_processors()?;
        if read && rt != 31 {
            set(cpu, Register::X(rt), value)?;
        }
        set(
            cpu,
            Register::Pc,
            scalar(cpu, Register::Pc)?.wrapping_add(4),
        )
    }
}
struct Services<'a> {
    execution_policy: ExecutionPolicy,
    device_workers: usize,
    host_pause: &'a Cell<bool>,
    power_off: &'a mut bool,
    memory: &'a memory::Memory,
    timeline: &'a mut Timeline,
    interrupts: &'a mut Option<InterruptMap>,
    core_interrupts: &'a [Option<CoreInterruptMap>],
    interrupt_changed: &'a mut [bool],
    released: &'a mut [bool],
    release_state_changed: &'a mut bool,
    waiting: &'a mut [bool],
    error: &'a mut Option<String>,
    records: &'a mut Vec<Record>,
    irq_observations: &'a mut Option<(usize, Vec<Value>)>,
    console: &'a mut Vec<String>,
    frames: &'a mut std::collections::BTreeMap<
        usize,
        fujimod_emulator_devices::graphics::scanout::Frame,
    >,
    outputs: &'a mut std::collections::BTreeMap<usize, Value>,
    mprofile: &'a mut [Option<mprofile::Profile>],
    cpus: &'a [Rc<dyn Cpu>],
    exclusive: &'a fujimod_emulator_cpu::exclusive::Shared,
    code_run: &'a mut Option<usize>,
    code_write_deferred: &'a mut bool,
    decoders: &'a mut decoder::Decoders,
    active_core: Option<usize>,
    memory_access: bool,
}
impl Services<'_> {
    fn remember(&mut self, result: Result<(), String>) {
        if let Err(e) = result
            && self.error.is_none()
        {
            *self.error = Some(e);
        }
    }
}
impl Host for Services<'_> {
    fn compute_workers(&self) -> usize {
        device_workers(self.execution_policy, self.device_workers)
    }
    fn select_memory(&mut self, decoder: usize, choice: usize) -> Result<(), String> {
        self.decoders.select(
            decoder,
            choice,
            self.active_core.is_none() || self.memory_access,
        )?;
        if !self.decoders.pending() {
            return Ok(());
        }
        *self.code_run = None;
        if let Some(core) = self.active_core {
            // An MMIO callback is inside the selecting instruction. A raw
            // stop can leave its PC unchanged and replay it on resume.
            let cpu = &self.cpus[core];
            if let Some(progress)=cpu.instruction_progress() {
                cpu.tighten_instruction_limit(progress)
            }else{
                // The all-instruction hook stops before the next instruction,
                // after this MMIO instruction has retired and advanced its PC.
                Ok(())
            }
        } else {
            self.decoders.apply(self.cpus, self.memory)
        }
    }
    fn now(&self) -> u64 {
        self.timeline.now()
    }
    fn request_pause(&mut self) -> Result<(), String> {
        let core = self
            .active_core
            .ok_or("host request pause requires an active guest instruction")?;
        let cpu = self.cpus.get(core).ok_or("host pause targets absent CPU")?;
        let progress = cpu
            .instruction_progress()
            .ok_or("CPU cannot pause at the current instruction boundary")?;
        cpu.tighten_instruction_limit(progress)?;
        self.host_pause.set(true);
        Ok(())
    }
    fn power_off(&mut self) -> Result<(), String> {
        if self.active_core.is_some() { self.request_pause()?; }
        self.host_pause.set(true);
        *self.power_off = true;
        Ok(())
    }
    fn schedule(&mut self, device: usize, at: u64, token: u64) {
        let r = self
            .timeline
            .schedule(Target::DeviceEvent(device), at, token);
        self.remember(r);
    }
    fn set_irq(&mut self, line: u32, level: bool) {
        let r = self
            .interrupts
            .as_mut()
            .ok_or("SPI without interrupt controller".into())
            .and_then(|g| g.gic.set_spi(line as usize, level));
        self.remember(r);
    }
    fn raise_irq(&mut self, cpu: usize, line: u32) -> Result<(), String> {
        if let Some(Some(controller)) = self.core_interrupts.get(cpu) {
            controller.inputs.pulse(line)?;
            if let Some((limit, rows)) = self.irq_observations.as_mut()
                && rows.len() < *limit
            {
                rows.push(json!({"name":"Peripheral IRQ request","cpu":cpu,"line":line,"tick":self.timeline.now()}));
            }
            self.interrupt_changed[cpu] = true;
            return Ok(());
        }
        if let Some(Some(p)) = self.mprofile.get_mut(cpu) {
            return p.pulse(line);
        }
        self.set_core_irq(cpu, line, true)
    }
    fn set_core_irq(&mut self, cpu: usize, line: u32, level: bool) -> Result<(), String> {
        if let Some(Some(controller)) = self.core_interrupts.get(cpu) {
            controller.inputs.level(line, level)?;
            if let Some((limit, rows)) = self.irq_observations.as_mut()
                && rows.len() < *limit
            {
                rows.push(json!({"name":"Peripheral IRQ level","cpu":cpu,"line":line,"high":level,"tick":self.timeline.now()}));
            }
            self.interrupt_changed[cpu] = true;
            return Ok(());
        }
        if let Some(Some(p)) = self.mprofile.get_mut(cpu) {
            return p.level(line, level);
        }
        self.interrupts
            .as_mut()
            .ok_or("PPI without interrupt controller")?
            .gic
            .set_ppi(cpu, line as usize, level)
    }
    fn reset_core(&mut self, cpu: usize, asserted: bool) -> Result<(), String> {
        let r = self
            .released
            .get_mut(cpu)
            .ok_or("reset targets absent CPU")?;
        if let Some(Some(p)) = self.mprofile.get_mut(cpu) {
            if asserted || !*r {
                p.clear_exclusive()?;
                p.needs_reset = true;
            }
            let next_released = !asserted;
            *self.release_state_changed |= *r != next_released;
            *r = next_released;
            self.waiting[cpu] = false;
            return Ok(());
        }
        if asserted && *r {
            return Err("live CPU reset requires a declared reset context".into());
        }
        let next_released = !asserted;
        *self.release_state_changed |= *r != next_released;
        *r = next_released;
        self.waiting[cpu] = false;
        Ok(())
    }
    fn ram_read(&self, address: u64, out: &mut [u8]) -> Result<(), String> {
        if self.decoders.pending() {
            return Err("DMA requires committed memory decoding".into());
        }
        self.memory.read_dma(address, out)
    }
    fn ram_write(&mut self, address: u64, bytes: &[u8]) -> Result<(), String> {
        if self.decoders.pending() {
            return Err("DMA requires committed memory decoding".into());
        }
        // A write that lands during the slice of an M-profile processor and
        // touches none of the RAM that processor maps cannot change its code
        // or its page copies. Its slice continues; the mark follows when the
        // slice ends, where every later reader finds it.
        let foreign = self.execution_policy == ExecutionPolicy::Exact
            && self.active_core.is_some_and(|core| {
                *self.code_run == Some(core)
                    && self.mprofile[core]
                        .as_ref()
                        .is_some_and(|p| !p.maps_physical(address, bytes.len()))
            });
        if foreign {
            *self.code_write_deferred = true;
        } else {
            *self.code_run = None;
        }
        self.memory
            .watch_transfer(write_watch::WriteSource::Dma, address, bytes, self.timeline.now());
        self.memory
            .write_dma(self.cpus, self.exclusive, address, bytes)
    }
    fn scanout(
        &mut self,
        frame: fujimod_emulator_devices::graphics::scanout::Frame,
    ) -> Result<(), String> {
        self.frames.insert(frame.output, frame);
        Ok(())
    }
    fn output_status(&mut self, output: usize, state: Value) -> Result<(), String> {
        self.outputs.insert(output, state);
        Ok(())
    }
    fn record(&mut self, record: Record) {
        self.records.push(record);
    }
    fn log(&mut self, line: &str) {
        self.console.push(line.into());
    }
    fn console(&mut self, line: &str) {
        self.console.push(line.into());
    }
}
unsafe fn before(pc: u64, _: u32, data: *mut c_void) {
    let b = unsafe { &*data.cast::<Binding>() };
    let Some(cpus) = b.cpus.upgrade() else {
        return;
    };
    let result = (|| {
        let cpu = &*cpus[b.core];
        let mut s = b.state.borrow_mut();
        if let Some(blocks) = s.blocks[b.core].as_mut() {
            blocks.instruction_callbacks += 1;
        }
        s.run_entries[b.core] += 1;
        s.sync(cpu, b.core)?;
        s.due(&cpus)?;
        if s.decoders.pending() {
            cpu.stop()?;
            return Ok(());
        }
        // Before anything below that is not repeatable: when this stops for
        // a translation resync, the observer runs again for this entry, and
        // repeating the catch-up above at an unchanged time does nothing.
        if s.enter_code(cpu, b.core, pc)? {
            return Ok(());
        }
        let settled = s.settled();
        // One decision covers the pin push here and the deadline refresh below.
        let verify = settled && s.settled.verifies();
        if settled {
            s.settled.skips += 1;
        } else {
            s.settled.full += 1;
        }
        if settled {
            if verify {
                s.verify_settled(&cpus, true)?;
            }
        } else if s.push_pins(&cpus)? {
            cpu.stop()?;
            return Ok(());
        }
        if s.error.is_some() {
            cpu.stop()?;
            return Ok(());
        }
        if s.scheduler_room(b.core) == 0 {
            cpu.stop()?;
            return Ok(());
        }
        if s.core_interrupts[b.core].is_some()
            && s.pins[b.core].0
            && fujimod_emulator_cpu_unicorn::rprofile::enter_irq(cpu)?.is_some()
        {
            cpu.stop()?;
            return Ok(());
        }
        if s.mprofile[b.core].is_some() && s.code_run != Some(b.core) {
            // A due DMA write may have replaced the instruction in this TB.
            // Restart before counting or executing its stale translation.
            cpu.stop()?;
            return Ok(());
        }
        if let Some(p) = &s.armv7a[b.core]
            && s.pins[b.core].0 && p.enter_irq(cpu)? {
            s.armv7a_entries[b.core].irq += 1;
            return Ok(());
        }
        if s.observe_entry(b.core,pc,cpu)? {return Ok(());}
        // Architectural counters follow the core's own position.
        let now = s.timeline.processor_clock(b.core)?.now;
        if s.aarch64_before(cpu, b.core, pc)? { return Ok(()); }
        if let Some(p) = &mut s.armv7a[b.core] {
            let entry = p.before(cpu, pc, now)?;
            if let armv7a::Entry::Hint(hint) = entry { s.hint(b.core, hint); }
        }
        // The page just proven is mapped host memory: read the instruction
        // there instead of through the engine.
        let fetched = s.code_shadow[b.core]
            .as_ref()
            .and_then(|shadow| shadow.fetch(pc));
        let idle = settled && s.settled.idle == Some(b.core);
        if let Some(p) = &mut s.mprofile[b.core] {
            let entry = p.before(cpu, pc, fetched, idle)?;
            let ready = p.ready();
            p.take_stopped();
            if p.take_alias_write() {
                s.code_run = None;
            }
            let execute = matches!(entry, mprofile::Entry::Execute);
            if !execute {
                // The controller entered, returned from or emulated something.
                s.settled.clear();
            }
            match entry {
                mprofile::Entry::Redirect => return Ok(()),
                mprofile::Entry::Emulated => (),
                mprofile::Entry::Wait => s.waiting[b.core] = true,
                mprofile::Entry::Execute => (),
            }
            if settled && execute {
                if verify {
                    s.verify_settled(&cpus, false)?;
                    s.verify_idle(b.core, idle)?;
                }
            } else {
                s.refresh_processors()?;
                if execute {
                    s.arm_settled()?;
                    if s.settled.armed && !ready {
                        s.settled.idle = Some(b.core);
                    }
                }
            }
            if execute && s.mprofile_loop(cpu, b.core, pc)? {
                // The window moved the processor and its clock.
                s.settled.clear();
                return Ok(());
            }
        }
        if !cpu.features().instruction_progress {
            s.account(b.core, 1)?;
            s.instructions[b.core] = s.instructions[b.core]
                .checked_add(1)
                .ok_or("instruction counter overflow")?;
        }
        Ok::<(), String>(())
    })();
    if let Err(e) = result {
        b.state.borrow_mut().fail(e);
        let _ = cpus[b.core].stop();
    }
}
/// Classify Thumb instructions from their encoding when the accelerator opts in.
unsafe fn block_classify(_: *mut c_void, _: u64, word: u32) -> fujimod_emulator_cpu::Translation {
    fujimod_emulator_cpu::Translation {
        instruction: fujimod_emulator_cpu::InstructionDisposition::Execute,
        observe_block: !mprofile_blocks::can_run_inline(word),
    }
}
/// Recoverable blocks can also execute ordinary memory transfers. Device and
/// watched accesses still enter the board at the instruction that made them.
unsafe fn block_classify_transfers(
    data: *mut c_void,
    pc: u64,
    word: u32,
) -> fujimod_emulator_cpu::Translation {
    let mut classification = unsafe { block_classify(data, pc, word) };
    if mprofile_blocks::can_run_in_it(word) {
        classification.instruction = fujimod_emulator_cpu::InstructionDisposition::ExecuteItSafe;
        classification.observe_block = false;
    }
    // PUSH can make several independent writes. Keeping it last lets each
    // MMIO or watched store complete without restarting an earlier write.
    let short_push = word & 0xffff_fe00 == 0xb400 && word & 0x01ff != 0;
    let wide_push = word >> 16 == 0xe92d
        && word & 0xa000 == 0
        && (word as u16).count_ones() >= 2;
    if short_push || wide_push {
        classification.instruction = fujimod_emulator_cpu::InstructionDisposition::ExecuteAndEndBlock;
        classification.observe_block = false;
    } else {
        classification.observe_block &= !mprofile_blocks::is_single_transfer(word);
    }
    classification
}
/// The engine is about to redo the instruction at `pc` as a block of its own:
/// of the admitted block, only the instructions before it were entered.
unsafe fn block_restarted(data: *mut c_void, pc: u64) {
    let b = unsafe { &*data.cast::<Binding>() };
    let Some(cpus) = b.cpus.upgrade() else {
        return;
    };
    let result = {
        let mut state = b.state.borrow_mut();
        if let Some(blocks) = state.blocks[b.core].as_mut() {
            blocks.restart_callbacks += 1;
        }
        state.settle_interrupted_block(&*cpus[b.core], b.core, pc, false)
    };
    if let Err(e) = result {
        b.state.borrow_mut().fail(e);
        let _ = cpus[b.core].stop();
    }
}
/// Block execution's observer: the instruction observer's work for the first
/// instruction of a block, then the admission of the instructions that may
/// follow it unobserved (`mprofile_blocks`).
unsafe fn block_entered(block: fujimod_emulator_cpu::Block, data: *mut c_void) {
    let b = unsafe { &*data.cast::<Binding>() };
    let Some(cpus) = b.cpus.upgrade() else {
        return;
    };
    let _callback_sample = callback_service_sample(b, &cpus, callback_timing::CallbackKind::Block);
    let core = b.core;
    let result = (|| {
        let cpu = &*cpus[core];
        let mut s = b.state.borrow_mut();
        if let Some(blocks) = s.blocks[core].as_mut() {
            blocks.block_callbacks += 1;
        }
        let pc = block.address;
        let bytes = block.bytes & 0x3fff_ffff;
        // The block before this one ran whole.
        s.settle_block(core, None)?;
        let room = s.blocks[core].as_ref().map_or(0, |blocks| blocks.room);
        if room == 0 {
            // The engine's counter stops before the observer of the first
            // entry beyond the budget.
            cpu.stop()?;
            return Ok(());
        }
        s.due(&cpus)?;
        if s.decoders.pending() {
            cpu.stop()?;
            return Ok(());
        }
        if s.enter_code(cpu, core, pc)? {
            return Ok(());
        }
        let was_settled = s.settled();
        if !was_settled {
            s.settled.full += 1;
            if s.push_pins(&cpus)? {
                return cpu.stop();
            }
            if s.settled.pass {
                // A fixed-point push can establish the window before the
                // first instruction, avoiding a one-entry translation.
                s.refresh_processors()?;
                s.settle_after_pin_push(core)?;
            }
        }
        // A shortened translation can return here before any instruction
        // executes. Reuse the fixed point established above if it still holds.
        let settled = was_settled || s.settled();
        let scan = {
            let mut buffer = Vec::new();
            let code = s.block_code(cpu, core, pc, bytes as usize, &mut buffer)?;
            match s.blocks[core].as_ref().and_then(|blocks| blocks.scans.get(pc, code)) {
                Some(scan) => scan,
                None => {
                    let scan = mprofile_blocks::scan(code)?;
                    let kept = mprofile_blocks::ScanCache::key(code);
                    if let (Some(kept), Some(blocks)) = (kept, s.blocks[core].as_mut()) {
                        blocks.scans.put(pc, kept, scan);
                    }
                    scan
                }
            }
        };
        let (entries, last) = (scan.entries, scan.last);
        // A `bx lr` that ends the admission is an ordinary branch when LR is
        // not an exception return value now and the block leaves LR alone:
        // the controller would let it execute. BX ends its block.
        let unit = if scan.returns_through_lr
            && scan.unit + 1 == entries
            && scalar(
                cpu,
                Register::Arm32(fujimod_emulator_cpu::arm::Register::R(14)),
            )? < 0xffff_ffe0
        {
            entries
        } else {
            scan.unit
        };
        let scheduler_room = settled.then(|| s.scheduler_room(core));
        let window_room = if let Some(scheduler_room) = scheduler_room {
            // Until the next device deadline, the end of the run window, the
            // next generic-timer output change and the peer's turn.
            let until = s.clock_limit.unwrap_or(u64::MAX).min(s.settled.until);
            room.min(s.timeline.processor_budget(core, u64::MAX, Some(until))?)
                .min(scheduler_room)
        } else {
            // The pin push and deadline refresh below are not yet known to be
            // a fixed point: the next instruction needs them again.
            1
        };
        let limit = unit.min(window_room);
        if limit == 0 {
            // No entry fits before the run window ends or the peer's turn.
            cpu.stop()?;
            return Ok(());
        }
        if limit < entries {
            let reason = if !settled {
                0
            } else if limit == unit {
                1
            } else if limit == room {
                2
            } else if scheduler_room == Some(limit) {
                3
            } else {
                4
            };
            if let Some(blocks) = s.blocks[core].as_mut() {
                blocks.shortened += 1;
                blocks.shortened_by[reason] += 1;
            }
            return cpu.retry_block(limit as u32);
        }
        // From here on, once per entry, in the instruction observer's order.
        let verify = settled && s.settled.verifies();
        if was_settled {
            s.settled.skips += 1;
            if verify {
                s.verify_settled(&cpus, true)?;
            }
        }
        if s.error.is_some()
            || (!settled && s.scheduler_room(core) == 0)
            || s.code_run != Some(core)
        {
            cpu.stop()?;
            return Ok(());
        }
        // This entry reaches the controller: the engine's counter would have
        // counted it.
        let blocks = s.blocks[core].as_mut().ok_or("block execution without its state")?;
        blocks.room -= 1;
        blocks.observed += 1;
        let fetched = s.code_shadow[core]
            .as_ref()
            .and_then(|shadow| shadow.fetch(pc));
        let idle = settled && s.settled.idle == Some(core);
        let p = s.mprofile[core]
            .as_mut()
            .ok_or("block execution without an M-profile controller")?;
        let entry = p.before(cpu, pc, fetched, idle)?;
        let ready = p.ready();
        // An exclusive instruction stops the engine as a scheduling boundary;
        // other emulated instructions only move the PC.
        let stopped = p.take_stopped();
        if p.take_alias_write() {
            s.code_run = None;
        }
        let execute = matches!(entry, mprofile::Entry::Execute);
        if !execute {
            s.settled.clear();
            s.blocks[core].as_mut().unwrap().poll = None;
        }
        match entry {
            mprofile::Entry::Redirect => {
                // The controller entered an exception: resume at its PC.
                s.blocks[core].as_mut().unwrap().resume = true;
                return cpu.stop();
            }
            mprofile::Entry::Emulated => (),
            mprofile::Entry::Wait => s.waiting[core] = true,
            mprofile::Entry::Execute => (),
        }
        if settled && execute {
            if verify {
                s.verify_settled(&cpus, false)?;
                s.verify_idle(core, idle)?;
            }
        } else {
            s.refresh_processors()?;
            if execute {
                s.arm_settled()?;
                if s.settled.armed && !ready {
                    s.settled.idle = Some(core);
                }
            }
        }
        if !execute {
            // The controller performed the instruction (or waits after it):
            // one entry, and the engine continues at the PC it left.
            s.account(core, 1)?;
            s.instructions[core] = s.instructions[core]
                .checked_add(1)
                .ok_or("instruction counter overflow")?;
            if matches!(entry, mprofile::Entry::Emulated) && !stopped {
                s.blocks[core].as_mut().unwrap().resume = true;
            }
            return cpu.stop();
        }
        // The translated guard admits subsequent eligible blocks on this
        // checked page. Hooked memory accesses settle and disarm the window.
        let inline = settled && s.settled.armed && !ready && fetched.is_some()
            && s.code_run == Some(core);
        // Admit the block: its first entry is already debited.
        let blocks = s.blocks[core].as_mut().unwrap();
        blocks.room -= entries - 1;
        blocks.pending = Some((pc, bytes, entries, pc + last));
        if inline {
            blocks.inline.arm(pc, window_room - entries);
        }
        Ok::<(), String>(())
    })();
    if let Err(e) = result {
        b.state.borrow_mut().fail(e);
        let _ = cpus[core].stop();
    }
}
fn notify_memory(
    cpus: &[Rc<dyn Cpu>],
    profiles: &[Option<mprofile::Profile>],
    domain: &fujimod_emulator_cpu::exclusive::Shared,
    writer: usize,
    address: u64,
    size: usize,
) -> Result<(), String> {
    if size == 0 || cpus[writer].exclusive_domain().is_some() {
        return Ok(());
    }
    let physical = if let Some(p) = &profiles[writer] {
        // Peripheral writes are not writes to the shared RAM reservation domain.
        let Ok(address) = p.physical(address, size) else {
            return Ok(());
        };
        address
    } else {
        address
    };
    domain
        .lock()
        .map_err(|_| "exclusive monitors poisoned")?
        .written(Some((physical, size)))
}
unsafe fn written(
    _: fujimod_emulator_cpu::Access,
    address: u64,
    size: i32,
    value: i64,
    data: *mut c_void,
) {
    let b = unsafe { &*data.cast::<Binding>() };
    let Some(cpus) = b.cpus.upgrade() else {
        return;
    };
    let _callback_sample = callback_service_sample(b, &cpus, callback_timing::CallbackKind::Store);
    let diagnostic_m4;
    let mut observed_reasons = 0u8;
    let mut classification_finished = false;
    let result = {
        let mut state = b.state.borrow_mut();
        diagnostic_m4 = cpus[b.core].model() == fujimod_emulator_cpu::Model::CortexM4
            && state
                .store_observer
                .as_ref()
                .is_some_and(|counters| counters.get(b.core).is_some());
        if diagnostic_m4 {
            if let Some(counters) = state
                .store_observer
                .as_mut()
                .and_then(|counters| counters.get_mut(b.core))
            {
                counters.callbacks_total = counters.callbacks_total.saturating_add(1);
            }
            let reservation_live = state
                .exclusive
                .lock()
                .is_ok_and(|monitors| monitors.active());
            if reservation_live {
                state.observe_store_reason(b.core, StoreObserverReason::LiveExclusiveReservation);
                observed_reasons = observed_reasons.saturating_add(1);
            }
        }
        if let Some(blocks) = state.blocks[b.core].as_mut() {
            blocks.store_callbacks += 1;
        }
        usize::try_from(size)
            .map_err(|_| "invalid memory-store width".into())
            .and_then(|size| {
                // Block execution. While the hook sees every store, a hooked
                // store is the last instruction of its block. Once it sees
                // only watched pages, a hooked store may be inside a block:
                // have the engine redo it as a block of its own, and do this
                // hook's work then. The restart accounts the entered prefix.
                if let Some(blocks) = state.blocks[b.core].as_ref().filter(|b| b.fast_stores) {
                    let cpu = &*cpus[b.core];
                    if let Some((start, bytes, _)) = blocks.inline.charged_block() {
                        let pc = scalar(cpu, Register::Pc)? & !1;
                        let remaining = (start + u64::from(bytes)).checked_sub(pc)
                            .ok_or("watched store outside its charged Thumb block")?;
                        let terminal = match remaining {
                            2 => true,
                            4 => {
                                let mut buffer = Vec::new();
                                let code = state.block_code(cpu, b.core, pc, 4, &mut buffer)?;
                                mprofile_blocks::count_entries(code)? == 1
                            }
                            _ => false,
                        };
                        if !terminal {
                            if diagnostic_m4 {
                                state.observe_store_reason(
                                    b.core,
                                    StoreObserverReason::RestartAttempt,
                                );
                                observed_reasons = observed_reasons.saturating_add(1);
                                state.finish_store_observation(b.core, observed_reasons, false);
                                classification_finished = true;
                            }
                            return cpu.request_restart();
                        }
                    } else if let Some((_, _, _, last)) = blocks.pending
                        && scalar(cpu, Register::Pc)? & !1 != last
                    {
                        if diagnostic_m4 {
                            state.observe_store_reason(b.core, StoreObserverReason::RestartAttempt);
                            observed_reasons = observed_reasons.saturating_add(1);
                            state.finish_store_observation(b.core, observed_reasons, false);
                            classification_finished = true;
                        }
                        return cpu.request_restart();
                    }
                }
                state.settle_block(b.core, None)?;
                // A store this hook sees ends a poll loop candidate.
                if let Some(blocks) = state.blocks[b.core].as_mut() {
                    blocks.poll = None;
                }
                state.polls.last[b.core] = None;
                let address =
                    cpus[b.core].translate(address, fujimod_emulator_cpu::Access::Write)?;
                let explicit_write_watch_overlap = state.memory.watching_writes()
                    && write_watch::observe_store(
                        &state,
                        &*cpus[b.core],
                        b.core,
                        address,
                        size,
                        value,
                    );
                if diagnostic_m4 && explicit_write_watch_overlap {
                    state.observe_store_reason(
                        b.core,
                        StoreObserverReason::ExplicitWriteWatchOverlap,
                    );
                    observed_reasons = observed_reasons.saturating_add(1);
                }
                let mprofile_alias_write = state.mprofile[b.core]
                    .as_ref()
                    .is_some_and(|p| p.writes_an_alias(address, size));
                if mprofile_alias_write {
                    if diagnostic_m4 {
                        state.observe_store_reason(b.core, StoreObserverReason::MProfileAliasWrite);
                        observed_reasons = observed_reasons.saturating_add(1);
                    }
                    state.code_run = None;
                }
                if state.execution_policy == ExecutionPolicy::Exact {
                    let code_shadow_active = state.code_shadow[b.core].is_some();
                    if diagnostic_m4 && code_shadow_active {
                        if let Some(counters) = state
                            .store_observer
                            .as_mut()
                            .and_then(|counters| counters.get_mut(b.core))
                        {
                            counters.code_shadow_store_checks =
                                counters.code_shadow_store_checks.saturating_add(1);
                        }
                    }
                    if let Some(shadow) = state.code_shadow[b.core].as_mut() {
                        shadow.stored(&*cpus[b.core], address, size);
                    }
                }
                let result = notify_memory(
                    &cpus,
                    &state.mprofile,
                    &state.exclusive,
                    b.core,
                    address,
                    size,
                );
                if diagnostic_m4 {
                    state.finish_store_observation(b.core, observed_reasons, true);
                    classification_finished = true;
                }
                result
            })
    };
    if let Err(e) = result {
        if diagnostic_m4 && !classification_finished {
            let mut state = b.state.borrow_mut();
            state.fail_store_observation_before_classification(b.core);
            state.finish_store_observation(b.core, observed_reasons, false);
        }
        b.state.borrow_mut().fail(e);
        let _ = cpus[b.core].stop();
    }
}
unsafe fn access(data: *mut c_void, offset: u64, size: u32, value: Option<u64>) -> u64 {
    let b = unsafe { &*data.cast::<Binding>() };
    let Some(cpus) = b.cpus.upgrade() else {
        return 0;
    };
    let _callback_sample = callback_service_sample(b, &cpus, callback_timing::CallbackKind::Mmio);
    let result = (|| {
        let address = b.base.checked_add(offset).ok_or("MMIO address overflow")?;
        let size = u8::try_from(size).map_err(|_| "MMIO width overflow")?;
        if let Some(count) = b.mmio_callback_counts.borrow_mut().get_mut(b.core) {
            *count = count.saturating_add(1);
        }
        let mut state = b.state.borrow_mut();
        if let Some(blocks) = state.blocks[b.core].as_mut() {
            blocks.mmio_callbacks += 1;
        }
        let result = state.access(&cpus, b, address, size, value);
        // Reads a poll loop fast-forward left out count as serviced.
        let skipped = std::mem::take(&mut state.polls.skipped_reads)
            + state.blocks[b.core]
                .as_mut()
                .map_or(0, |blocks| std::mem::take(&mut blocks.skipped_reads));
        if skipped != 0
            && let Some(count) = b.mmio_callback_counts.borrow_mut().get_mut(b.core)
        {
            *count = count.saturating_add(skipped);
        }
        result
    })();
    match result {
        Ok(v) => v,
        Err(e) => {
            b.state.borrow_mut().fail(e);
            let _ = cpus[b.core].stop();
            0
        }
    }
}
unsafe fn read(offset: u64, size: u32, data: *mut c_void) -> u64 {
    unsafe { access(data, offset, size, None) }
}
// The async owner pump parks every peer before dispatching this callback.
// Sensitive CPU memory operations retire only after it returns.
unsafe fn synchronization_barrier(_: *mut c_void) {}
unsafe fn write(offset: u64, size: u32, value: u64, data: *mut c_void) {
    unsafe {
        access(data, offset, size, Some(value));
    }
}
