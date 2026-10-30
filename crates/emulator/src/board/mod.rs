//! Board execution and device scheduling above the CPU backend boundary.
pub mod aprofile;
pub mod armv7a;
pub mod bus;
pub mod continuation;
pub mod decoder;
#[cfg(test)]
mod decoder_tests;
pub mod discovery;
pub mod factory;
pub mod harness;
mod host;
mod host_profile;
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
mod preservation_tests;
mod observe;
mod schedule;
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

mod interrupts;
pub use interrupts::{InterruptMap, InterruptRegisters};
struct CoreInterruptMap {
    device: usize,
    inputs: Box<dyn fujimod_emulator_devices::bus::CoreInterrupts>,
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

struct State {
    continuation: Rc<RefCell<continuation::Stops>>,
    host_pause: Rc<Cell<bool>>,
    execution_policy: ExecutionPolicy,
    // Host computation preference; absent from fingerprints and keyframes.
    device_workers: usize,
    // Host run boundary, not a device deadline or persistent modeled state.
    clock_limit: Option<u64>,
    memory: Rc<memory::Memory>,
    bus: Bus,
    timeline: Timeline,
    interrupts: Option<InterruptMap>,
    core_interrupts: Vec<Option<CoreInterruptMap>>,
    interrupt_changed: Vec<bool>,
    released: Vec<bool>,
    waiting: Vec<bool>,
    waiting_event: Vec<bool>,
    events: Vec<bool>,
    pins: Vec<(bool, bool)>,
    interrupt_delivery: Vec<bool>,
    instructions: Vec<u64>,
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
    active_slice: Option<schedule::Slice>,
    parallel: Option<interactive::Epoch>,
    decoders: decoder::Decoders,
    memory_access: bool,
}
struct Binding {
    state: Rc<RefCell<State>>,
    cpus: Weak<Vec<Rc<dyn Cpu>>>,
    mmio_callback_counts: Rc<RefCell<Vec<u64>>>,
    core: usize,
    base: u64,
}
/// Devices and callbacks have one owner. Interactive native processors run
/// concurrently between barriers; exact execution uses ordered slices.
pub struct Board {
    cpus: Rc<Vec<Rc<dyn Cpu>>>,
    state: Rc<RefCell<State>>,
    run_wakeup: fujimod_emulator_cpu::RunWakeup,
    bindings: Vec<Box<Binding>>,
    hooks: Vec<(usize, HookId)>,
    mmio: Vec<(usize, u64, usize)>,
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
    continuation: Option<Box<continuation::StoreWatch>>,
    /// Processor address views of RAM, from the checked graph.
    mappings: Vec<factory::MemoryMap>,
    read_set: RefCell<Option<continuation::ReadSet>>,
    watches: Vec<Box<observe::Watch>>,
    entries: Rc<RefCell<observe::Entries>>,
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
        self.slice_left.set(0);
        Ok(())
    }
    pub fn fingerprint(&self) -> Result<Value, String> {
        let state = self
            .state
            .try_borrow()
            .map_err(|_| "fingerprint requires a stopped board")?;
        if state.active_slice.is_some() || state.parallel.is_some() || state.memory_access {
            return Err("fingerprint requires a stopped board".into());
        }
        let mut runtime = self.state_with_bus(state.bus.fingerprint()?);
        // accounted is progress relative to the last backend host call. Every
        // new processor run resets it; it is not future-affecting guest state.
        runtime["scheduler"] = json!({"deadlines":state.deadlines,"generations":state.generations,
            "processor_deadlines":state.processor_deadlines,"processor_generations":state.processor_generations,
            "interrupt_delivery":state.interrupt_delivery,"irq_observations":state.irq_observations,
            "isolated_core":self.isolated_core.get(),"host_pause":self.host_pause.get()});
        runtime["records"] = json!(
            state
                .records
                .iter()
                .map(|r| format!("{r:?}"))
                .collect::<Vec<_>>()
        );
        runtime["console"] = json!(state.console);
        runtime["frames"] = state
            .frames
            .iter()
            .map(|(output, f)| {
                json!({"output":output,"sequence":f.sequence,
            "width":f.width,"height":f.height,"rgba":fujimod_emulator_devices::fingerprint::bytes(&f.rgba)})
            })
            .collect();
        let mut value = json!({"memory":state.memory.fingerprint(),
            "processors":self.cpus.iter().map(|c|c.fingerprint()).collect::<Result<Vec<_>,_>>()?,"pc_entries":self.pc_entries()});
        value["runtime"] = runtime;
        Ok(value)
    }
    pub fn new(
        cpus: Vec<Box<dyn Cpu>>,
        bus: Bus,
        ratio: Ratio,
        interrupts: Option<InterruptMap>,
        released: Vec<bool>,
        memory: memory::Memory,
    ) -> Result<Self, String> {
        if cpus.is_empty() || cpus.len() != released.len() || !released.iter().any(|r| *r) {
            return Err("invalid released processor set".into());
        }
        if let Some(g) = &interrupts {
            g.validate(cpus.len())?;
        }
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
        let continuation = Rc::new(RefCell::new(continuation::Stops::default()));
        let host_pause = Rc::new(Cell::new(false));
        let state = Rc::new(RefCell::new(State {
            continuation,
            host_pause: host_pause.clone(),
            execution_policy: ExecutionPolicy::Exact,
            device_workers: 0,
            clock_limit: None,
            memory: memory.clone(),
            bus,
            timeline: Timeline::new(ratio)?,
            interrupts,
            core_interrupts: (0..n).map(|_| None).collect(),
            interrupt_changed: vec![false; n],
            released,
            waiting: vec![false; n],
            waiting_event: vec![false; n],
            events: vec![false; n],
            pins: vec![(false, false); n],
            interrupt_delivery: vec![true; n],
            instructions: vec![0; n],
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
            active_slice: None,
            parallel: None,
            decoders: decoder::Decoders::default(),
            memory_access: false,
        }));
        let mut board = Self {
            run_wakeup: Default::default(),
            host_pause,
            progress_observer: None,
            guest_profile_active: Cell::new(false),
            isolated_core: Cell::new(None),
            continuation: None,
            mappings: vec![],
            read_set: RefCell::new(None),
            watches: vec![],
            entries: Default::default(),
            discovery_stub_contexts: vec![],
            translation_cache_size_bytes: 0,
            cpus: Rc::new(cpus.into_iter().map(Rc::from).collect()),
            state,
            bindings: vec![],
            hooks: vec![],
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
                cpus: Rc::downgrade(&board.cpus),
                mmio_callback_counts: board.mmio_callback_counts.clone(),
                core,
                base: 0,
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
            }
            let batches = board.cpus[core].model() != fujimod_emulator_cpu::Model::CortexM4
                && features.architectural_exits
                && features.instruction_progress
                && features.tighten_instruction_limit;
            if !batches {
                let hook = unsafe {
                    board.cpus[core].install_hook(Hook::Instruction {
                        callback: before,
                        data,
                        range: HookRange::All,
                    })?
                };
                board.hooks.push((core, hook));
            }
            board.bindings.push(binding);
            if !board.cpus[core].features().shared_physical_maps
                && board.cpus.iter().any(|c| {
                    c.features().shared_physical_maps
                        || c.model() == fujimod_emulator_cpu::Model::CortexM4
                })
            {
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
    pub fn now(&self) -> u64 {
        self.state.borrow().timeline.now()
    }
    /// Begin host observations while all processors are stopped.
    pub fn start_host_profile(&self) -> Result<(), String> {
        let baseline = self
            .cpus
            .iter()
            .map(|cpu| cpu.exit_counters())
            .collect::<Result<_, _>>()?;
        self.state.borrow_mut().bus.start_host_profile(baseline)
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
    /// Observe phase costs while all processors are stopped after a board run.
    pub fn host_profile(&self) -> Value {
        self.state
            .borrow()
            .bus
            .host_profile(|| self.cpus.iter().map(|cpu| cpu.exit_counters()).collect())
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
    /// Read physical memory without adding it to an active predicate's read
    /// set: the bytes are observed, but stores to them do not end a wait.
    pub fn inspect_physical(&self, address: u64, bytes: &mut [u8]) -> Result<(), String> {
        self.physical_memory.read_host(address, bytes)
    }
    pub fn write_physical(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
        let mut state = self.state.borrow_mut();
        state.code_run = None;
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
        let mut s = self.state.borrow_mut();
        if s.instructions.iter().any(|n| *n != 0)
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
        self.state.borrow_mut().mprofile[core] = Some(profile);
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
        let binding = Box::new(Binding {
            state: self.state.clone(),
            cpus: Rc::downgrade(&self.cpus),
            mmio_callback_counts: self.mmio_callback_counts.clone(),
            core,
            base: physical,
        });
        let data = (&*binding as *const Binding).cast_mut().cast();
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
            .collect();
        unsafe {
            cpu.map_mmio_fast(address, bytes, Mmio { read, write, data }, fast_reads)?;
        }
        self.bindings.push(binding);
        self.mmio.push((core, address, bytes));
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
        let start = self.state.borrow().total_instructions()?;
        if end < start && self.execution_policy() == ExecutionPolicy::Exact {
            return Err("board instruction window moved backward".into());
        }
        if end <= start {
            return Ok(());
        }
        let profile_run = self.state.borrow_mut().bus.profile_begin_run();
        let result = self.run_inner(until, Some(end), pause_after, quantum);
        self.state.borrow_mut().clock_limit = None;
        self.state.borrow_mut().bus.profile_finish_run(profile_run);
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
            s.due(&self.cpus)?;
            s.push_pins(&self.cpus)?;
            // Clock inputs advance with execution/idle time, including for
            // parked peers. Host counter inspection never drives an update.
            for cpu in self.cpus.iter().filter(|cpu| cpu.features().guest_clock) {
                cpu.set_guest_clock(fujimod_emulator_cpu::Clock {
                    now: s.timeline.now(),
                    phase: s.timeline.phase(),
                    ratio: s.timeline.ratio(),
                })?;
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
            if until.is_some_and(|until| s.timeline.now() >= until) || remaining == 0 {
                return Ok(());
            }
            let n = self.cpus.len();
            if pause_after.is_some_and(|pause| s.total_instructions().is_ok_and(|n| n >= pause))
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
                    let budget = s.timeline.budget(room.min(cap).min(usize::MAX as u64))? as usize;
                    if budget == 0 {
                        return Err("scheduler made no progress at a device deadline".into());
                    }
                    drop(s);
                    self.interactive_window(&cores, budget, quantum)?;
                    completed_slice = None;
                    continue;
                }
            }
            let start = self.next_core.get();
            let core = (0..n)
                .map(|i| (start + i) % n)
                .find(|&c| s.released[c] && !s.waiting[c]);
            let Some(core) = core else {
                if until.is_none() && s.timeline.next_event().is_none() {
                    return Ok(());
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
            let cap = s.timeline.ratio().budget(
                s.timeline.now(),
                s.timeline.phase(),
                until.unwrap_or(u64::MAX),
            )?;
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
            let budget = s.timeline.budget(
                if peer {
                    self.slice_left.get() as u64
                } else {
                    solo_pause
                }
                .min(cap)
                .min(remaining)
                .min(usize::MAX as u64),
            )? as usize;
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
                s.code_run = None;
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
            if s.mprofile[core].is_some() && s.code_run != Some(core) {
                self.cpus[core].invalidate_code(None)?;
            }
            s.code_run = Some(core);
            s.active_slice = Some(slice);
            let entered = s.instructions[core];
            drop(s);
            let cpu = &self.cpus[core];
            // The exact budget ends at the first device/DMA/processor event
            // or runnable-peer turn; those boundaries end the stable window.
            cpu.set_loop_window(self.state.borrow().loop_window())?;
            let pc = scalar(&**cpu, Register::Pc)?;
            let profile_engine = self.state.borrow().bus.profile_begin_engine();
            let result = cpu.run(Run {
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
                instructions: Some(budget),
            });
            let mut s = self.state.borrow_mut();
            s.bus.profile_finish_engine(core, profile_engine);
            s.active_slice = None;
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
                    format!(
                        "CPU{core} {} at PC {stop_pc:#x}, instruction {word}, X0 {x0}: {error}",
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
    pub fn state(&self) -> Value {
        let bus = self.state.borrow().bus.state();
        self.state_with_bus(bus)
    }
    fn state_with_bus(&self, bus: Value) -> Value {
        let s = self.state.borrow();
        self.track_all_devices(&s.bus);
        let mut value = json!({"execution_mode":s.bus.mode().name(),"execution_policy":s.execution_policy.name(),"counts_as_boot":s.bus.mode()==bus::Mode::Strict,
            "timeline":s.timeline.save(),"instructions":s.instructions,"released":s.released,"waiting":s.waiting,"waiting_event":s.waiting_event,"events":s.events,"next_core":self.next_core.get(),"slice_left":self.slice_left.get(),"quantum":self.quantum.get(),"failure":s.error,
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
        let _ = self.continuation_end();
        let _ = self.release_store_watch();
        // The board exclusively owns processors; bindings hold only Weak
        // handles. CPUs drop before their still-valid callback contexts.
        for (core, hook) in self.hooks.drain(..) {
            let _ = self.cpus[core].remove_hook(hook);
        }
        for (core, base, bytes) in self.mmio.drain(..) {
            let _ = self.cpus[core].unmap(base, bytes);
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
    fn finish_exit(
        &mut self,
        cpu: &dyn Cpu,
        core: usize,
        pc: u64,
        exit: Exit,
    ) -> Result<(), String> {
        match exit {
            Exit::BeforeInstruction(word) => {
                let pc = scalar(cpu, Register::Pc)?;
                match cpu.model() {
                    fujimod_emulator_cpu::Model::ArmV8A => {
                        self.aarch64_instruction(cpu, core, pc, word)?;
                    }
                    fujimod_emulator_cpu::Model::CortexA7 => {
                        let now = self.timeline.now();
                        let profile = self.armv7a[core]
                            .as_mut()
                            .ok_or("ARMv7-A framework instruction without a profile")?;
                        match profile.before(cpu, pc, now)? {
                            // A failed condition retires the instruction unexecuted.
                            armv7a::Entry::Execute => set(cpu, Register::Pc, pc.wrapping_add(4))?,
                            armv7a::Entry::Completed => (),
                            armv7a::Entry::Hint(hint) => self.hint(core, hint),
                        }
                    }
                    _ => return Err("unsupported framework instruction exit".into()),
                }
                self.timeline.advance_instructions(1)?;
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
    fn apply_memory(&mut self, cpus: &[Rc<dyn Cpu>]) -> Result<(), String> {
        if self.decoders.pending() {
            self.code_run = None;
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
            let elapsed = match self.parallel.as_mut() {
                Some(epoch) => epoch.advance(core, delta)?,
                None => delta,
            };
            let elapsed = if self.execution_policy == ExecutionPolicy::Interactive {
                let boundary = self.timeline.ratio().budget(
                    self.timeline.now(),
                    self.timeline.phase(),
                    self.clock_limit.unwrap_or(u64::MAX),
                )?;
                self.timeline.budget(elapsed.min(boundary))?
            } else {
                elapsed
            };
            self.timeline.advance_instructions(elapsed)?;
            self.instructions[core] = self.instructions[core]
                .checked_add(delta)
                .ok_or("instruction counter overflow")?;
            self.accounted[core] = progress;
        } else if cpu.features().instruction_progress {
            return Err("processor entry count is unavailable".into());
        }
        Ok(())
    }
    fn refresh_deadlines(&mut self) -> Result<(), String> {
        for (id, at) in self.bus.changed_deadlines(self.timeline.now()) {
            if at == self.deadlines[id] {
                continue;
            }
            self.generations[id] = self.generations[id]
                .checked_add(1)
                .ok_or("device deadline generation exhausted")?;
            self.deadlines[id] = at;
            self.timeline.cancel_target(Target::DeviceDeadline(id));
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
            self.refresh_processors()?;
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
            bus,
            memory,
            timeline,
            interrupts,
            core_interrupts,
            interrupt_changed,
            released,
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
            decoders,
            active_slice,
            memory_access,
            ..
        } = self;
        let mut host = Services {
            execution_policy: *execution_policy,
            device_workers: *device_workers,
            host_pause,
            memory,
            timeline,
            interrupts,
            core_interrupts,
            interrupt_changed,
            released,
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
            cpus,
            decoders,
            active_core: active_slice.map(|s| s.core),
            memory_access: *memory_access,
        };
        let result = f(bus, &mut host);
        if self::continuation::fired(&self.continuation) {
            if let Some(core) = host.active_core {
                continuation::finish_instruction(&*cpus[core])?;
            }
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
        for (core, controller) in self.core_interrupts.iter().enumerate() {
            if let Some(controller) = controller {
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
                if pins.0 || pins.1 {
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
            if let Some(p) = p {
                if self.released[core] && p.pending() {
                    self.waiting[core] = false;
                }
            }
        }
        Ok(changed)
    }
    fn tighten(&self, cpu: &dyn Cpu, core: usize) -> Result<(), String> {
        if cpu.features().tighten_instruction_limit {
            let entered = cpu
                .instruction_progress()
                .ok_or("active entry count is unavailable")?;
            let remaining = match self.parallel.as_ref() {
                Some(epoch) => epoch.room(core),
                None => self.timeline.budget(self.scheduler_room(core))?,
            };
            cpu.tighten_instruction_limit(entered.saturating_add(remaining))?;
        }
        Ok(())
    }
    fn access(
        &mut self,
        cpus: &[Rc<dyn Cpu>],
        core: usize,
        address: u64,
        size: u8,
        value: Option<u64>,
    ) -> Result<u64, String> {
        if self.decoders.pending() {
            return Err("MMIO requires committed memory decoding".into());
        }
        self.sync(&*cpus[core], core)?;
        self.due(cpus)?;
        let pc = scalar(&*cpus[core], Register::Pc)?;
        if let Some(p) = &mut self.mprofile[core] {
            if (0xe0000000..0xe0100000).contains(&address) {
                let result = p.access(&*cpus[core], address, size, value, self.timeline.now())?;
                self.refresh_processors()?;
                self.push_pins(cpus)?;
                self.tighten(&*cpus[core], core)?;
                return Ok(result);
            }
        }
        if let Some(g) = &mut self.interrupts
            && let Some(result) = g.access(core, address, size, value) {
            let result = result?;
            self.push_pins(cpus)?;
            self.tighten(&*cpus[core], core)?;
            return Ok(result);
        }
        self.memory_access = value.is_some();
        let result = self.with_host(cpus, |bus, host| {
            bus.access(host, core, pc, address, size, value)
        });
        self.memory_access = false;
        let result = result?;
        self.refresh_deadlines()?;
        self.refresh_processors()?;
        self.due(cpus)?;
        self.push_pins(cpus)?;
        self.tighten(&*cpus[core], core)?;
        Ok(result)
    }
    fn system_register(&mut self, cpu: &dyn Cpu, core: usize, syndrome: u64) -> Result<(), String> {
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
            || r == 0xf665
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
    memory: &'a memory::Memory,
    timeline: &'a mut Timeline,
    interrupts: &'a mut Option<InterruptMap>,
    core_interrupts: &'a [Option<CoreInterruptMap>],
    interrupt_changed: &'a mut [bool],
    released: &'a mut [bool],
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
    decoders: &'a mut decoder::Decoders,
    active_core: Option<usize>,
    memory_access: bool,
}
impl Services<'_> {
    fn remember(&mut self, result: Result<(), String>) {
        if let Err(e) = result {
            if self.error.is_none() {
                *self.error = Some(e);
            }
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
            *r = !asserted;
            self.waiting[cpu] = false;
            return Ok(());
        }
        if asserted && *r {
            return Err("live CPU reset requires a declared reset context".into());
        }
        *r = !asserted;
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
        *self.code_run = None;
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
        s.sync(cpu, b.core)?;
        s.due(&cpus)?;
        if s.decoders.pending() {
            cpu.stop()?;
            return Ok(());
        }
        if s.push_pins(&cpus)? {
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
        let now = s.timeline.now();
        if s.aarch64_before(cpu, b.core, pc)? { return Ok(()); }
        if let Some(p) = &mut s.armv7a[b.core] {
            let entry = p.before(cpu, pc, now)?;
            if let armv7a::Entry::Hint(hint) = entry { s.hint(b.core, hint); }
        }
        if let Some(p) = &mut s.mprofile[b.core] {
            let entry = p.before(cpu, pc)?;
            if p.take_alias_write() {
                s.code_run = None;
            }
            match entry {
                mprofile::Entry::Redirect => return Ok(()),
                mprofile::Entry::Emulated => (),
                mprofile::Entry::Wait => s.waiting[b.core] = true,
                mprofile::Entry::Execute => (),
            }
            s.refresh_processors()?;
        }
        if !cpu.features().instruction_progress {
            s.timeline.advance_instructions(1)?;
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
    _: i64,
    data: *mut c_void,
) {
    let b = unsafe { &*data.cast::<Binding>() };
    let Some(cpus) = b.cpus.upgrade() else {
        return;
    };
    let result = {
        let mut state = b.state.borrow_mut();
        usize::try_from(size)
            .map_err(|_| "invalid memory-store width".into())
            .and_then(|size| {
                let address =
                    cpus[b.core].translate(address, fujimod_emulator_cpu::Access::Write)?;
                if state.mprofile[b.core]
                    .as_ref()
                    .is_some_and(|p| p.writes_an_alias(address, size))
                {
                    state.code_run = None;
                }
                notify_memory(
                    &cpus,
                    &state.mprofile,
                    &state.exclusive,
                    b.core,
                    address,
                    size,
                )
            })
    };
    if let Err(e) = result {
        b.state.borrow_mut().fail(e);
        let _ = cpus[b.core].stop();
    }
}
unsafe fn access(data: *mut c_void, offset: u64, size: u32, value: Option<u64>) -> u64 {
    let b = unsafe { &*data.cast::<Binding>() };
    let Some(cpus) = b.cpus.upgrade() else {
        return 0;
    };
    let result = (|| {
        let address = b.base.checked_add(offset).ok_or("MMIO address overflow")?;
        let size = u8::try_from(size).map_err(|_| "MMIO width overflow")?;
        if let Some(count) = b.mmio_callback_counts.borrow_mut().get_mut(b.core) {
            *count = count.saturating_add(1);
        }
        b.state
            .borrow_mut()
            .access(&cpus, b.core, address, size, value)
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
