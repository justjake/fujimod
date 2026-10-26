//! Exact in-process board keyframes. No guest access, tick, deadline refresh,
//! diagnostic serialization or execution occurs while capturing/restoring.
use super::*;
use fujimod_emulator_cpu::{Snapshot as CpuSnapshot, exclusive::Monitors};
use std::collections::BTreeMap;
mod portable;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    #[serde(with = "portable::policy")]
    execution_policy: ExecutionPolicy,
    #[serde(with = "portable::timeline")]
    timeline: Timeline,
    interrupt_changed: Vec<bool>,
    released: Vec<bool>,
    waiting: Vec<bool>,
    waiting_event: Vec<bool>,
    events: Vec<bool>,
    pins: Vec<(bool, bool)>,
    interrupt_delivery: Vec<bool>,
    irq_observations: Option<(usize, Vec<Value>)>,
    instructions: Vec<u64>,
    accounted: Vec<u64>,
    deadlines: Vec<Option<u64>>,
    generations: Vec<u64>,
    error: Option<String>,
    records: Vec<Record>,
    console: Vec<String>,
    #[serde(with = "portable::frames")]
    frames: BTreeMap<usize, fujimod_emulator_devices::graphics::scanout::Frame>,
    outputs: BTreeMap<usize, Value>,
    mprofile: Vec<Option<mprofile::Checkpoint>>,
    aprofile: Vec<Option<aprofile::Profile>>,
    armv7a: Vec<Option<armv7a::Profile>>,
    implementation_cluster: aprofile::cortex_a53::Registers,
    unmodeled_system: BTreeMap<(usize, u16, u64), (u64, u64)>,
    processor_deadlines: Vec<Option<u64>>,
    processor_generations: Vec<u64>,
    code_run: Option<usize>,
    /// The GIC's saved state.
    gic: Option<Value>,
}
pub struct Keyframe {
    owner: Weak<RefCell<State>>,
    processors: Vec<Box<dyn CpuSnapshot>>,
    devices: bus::Checkpoint,
    memory: memory::Checkpoint,
    decoders: decoder::Checkpoint,
    exclusive: Monitors,
    saved: Saved,
    entries: observe::Entries,
    next_core: usize,
    slice_left: usize,
    quantum: Option<usize>,
    host_pause: bool,
    isolated_core: Option<usize>,
    hooks: Vec<(usize, HookId)>,
    mmio: Vec<(usize, u64, usize)>,
    controllers: Vec<Option<usize>>,
    observations: Vec<(usize, Vec<u64>)>,
    durable_blocker: Option<String>,
}
impl Keyframe {
    pub fn execution_policy(&self) -> ExecutionPolicy {
        self.saved.execution_policy
    }
    pub fn observations(&self) -> &[(usize, Vec<u64>)] {
        &self.observations
    }
    pub fn failure(&self) -> Option<&str> {
        self.saved.error.as_deref()
    }
    pub fn encoded_device_bytes(&self) -> usize {
        self.devices.encoded_bytes()
    }
    pub fn now(&self) -> u64 {
        self.saved.timeline.now()
    }
    pub fn instruction_counts(&self) -> &[u64] {
        &self.saved.instructions
    }
}
impl State {
    fn checkpoint_boundary(&self) -> Result<(), String> {
        if self.active_slice.is_some() || self.parallel.is_some() || self.memory_access {
            return Err("keyframe requires a stopped board".into());
        }
        Ok(())
    }
    fn checkpoint_state(&self) -> Saved {
        // Exhaustive matching makes a new runtime field an explicit checkpoint
        // decision. Wiring/allocations are retained by this exact Board owner.
        let Self {
            execution_policy,
            device_workers: _,
            clock_limit: _,
            memory: _,
            bus: _,
            timeline,
            interrupts,
            core_interrupts: _,
            interrupt_changed,
            released,
            waiting,
            waiting_event,
            events,
            pins,
            interrupt_delivery,
            instructions,
            accounted,
            deadlines,
            generations,
            error,
            records,
            irq_observations,
            console,
            frames,
            outputs,
            mprofile,
            aprofile,
            armv7a,
            entry_points: _,
            implementation_cluster,
            unmodeled_system,
            processor_deadlines,
            processor_generations,
            reset_contexts: _,
            exclusive: _,
            code_run,
            active_slice: _,
            parallel: _,
            decoders: _,
            memory_access: _,
            continuation: _,
        } = self;
        Saved {
            execution_policy: *execution_policy,
            timeline: timeline.clone(),
            interrupt_changed: interrupt_changed.clone(),
            released: released.clone(),
            waiting: waiting.clone(),
            waiting_event: waiting_event.clone(),
            events: events.clone(),
            pins: pins.clone(),
            interrupt_delivery: interrupt_delivery.clone(),
            irq_observations: irq_observations.clone(),
            instructions: instructions.clone(),
            accounted: accounted.clone(),
            deadlines: deadlines.clone(),
            generations: generations.clone(),
            error: error.clone(),
            records: records.clone(),
            console: console.clone(),
            frames: frames.clone(),
            outputs: outputs.clone(),
            mprofile: mprofile
                .iter()
                .map(|p| p.as_ref().map(|p| p.checkpoint()))
                .collect(),
            aprofile: aprofile.clone(),
            armv7a: armv7a.clone(),
            implementation_cluster: implementation_cluster.clone(),
            unmodeled_system: unmodeled_system.clone(),
            processor_deadlines: processor_deadlines.clone(),
            processor_generations: processor_generations.clone(),
            code_run: *code_run,
            gic: interrupts.as_ref().map(|g| g.gic.save()),
        }
    }
    fn restore_state(&mut self, saved: &Saved) -> Result<(), String> {
        self.execution_policy = saved.execution_policy;
        self.clock_limit = None;
        self.timeline.clone_from(&saved.timeline);
        self.interrupt_changed.clone_from(&saved.interrupt_changed);
        self.released.clone_from(&saved.released);
        self.waiting.clone_from(&saved.waiting);
        self.waiting_event.clone_from(&saved.waiting_event);
        self.events.clone_from(&saved.events);
        self.pins.clone_from(&saved.pins);
        self.interrupt_delivery
            .clone_from(&saved.interrupt_delivery);
        self.irq_observations.clone_from(&saved.irq_observations);
        self.instructions.clone_from(&saved.instructions);
        self.accounted.clone_from(&saved.accounted);
        self.deadlines.clone_from(&saved.deadlines);
        self.generations.clone_from(&saved.generations);
        self.error.clone_from(&saved.error);
        self.records.clone_from(&saved.records);
        self.console.clone_from(&saved.console);
        self.frames.clone_from(&saved.frames);
        self.outputs.clone_from(&saved.outputs);
        for (profile, saved) in self.mprofile.iter_mut().zip(&saved.mprofile) {
            if let (Some(profile), Some(saved)) = (profile, saved) {
                profile.restore_checkpoint(saved);
            }
        }
        self.aprofile.clone_from(&saved.aprofile);
        self.armv7a.clone_from(&saved.armv7a);
        self.implementation_cluster
            .clone_from(&saved.implementation_cluster);
        self.unmodeled_system.clone_from(&saved.unmodeled_system);
        self.processor_deadlines
            .clone_from(&saved.processor_deadlines);
        self.processor_generations
            .clone_from(&saved.processor_generations);
        self.code_run = saved.code_run;
        if let (Some(interrupts), Some(gic)) = (&mut self.interrupts, &saved.gic) {
            interrupts.gic.restore(gic)?;
        }
        Ok(())
    }
}
impl Board {
    fn durable_blocker(&self) -> Option<String> {
        if self.continuation.as_ref().is_some_and(|watch| watch.active)
            || self.read_set.try_borrow().map_or(true, |r| r.is_some())
            || self.isolated_core.get().is_some()
            || !self.discovery_stub_contexts.is_empty()
        {
            Some("durable snapshots require a whole-board session without an installed execution harness".into())
        } else {
            None
        }
    }
    fn keyframe_owner(&self, saved: &Keyframe) -> Result<(), String> {
        if !Weak::ptr_eq(&saved.owner, &Rc::downgrade(&self.state)) {
            return Err("keyframe belongs to a different board".into());
        }
        Ok(())
    }
    pub fn checkpoint(&self, previous: Option<&Keyframe>) -> Result<Keyframe, String> {
        let state = self
            .state
            .try_borrow()
            .map_err(|_| "keyframe requires a stopped board")?;
        state.checkpoint_boundary()?;
        if let Some(previous) = previous {
            self.keyframe_owner(previous)?;
        }
        // Refuse an unsupported device before copying any physical memory.
        let devices = state.bus.checkpoint(previous.map(|p| &p.devices))?;
        state.memory.rearm(&self.cpus)?;
        let processors = self
            .cpus
            .iter()
            .enumerate()
            .map(|(index, cpu)| cpu.snapshot(previous.map(|p| p.processors[index].as_ref())))
            .collect::<Result<_, _>>()?;
        let memory = state.memory.checkpoint();
        state.memory.rearm(&self.cpus)?;
        Ok(Keyframe {
            owner: Rc::downgrade(&self.state),
            processors,
            devices,
            memory,
            decoders: state.decoders.checkpoint(),
            exclusive: state
                .exclusive
                .lock()
                .map_err(|_| "exclusive monitors poisoned")?
                .clone(),
            saved: state.checkpoint_state(),
            entries: self.entries.borrow().clone(),
            next_core: self.next_core.get(),
            slice_left: self.slice_left.get(),
            quantum: self.quantum.get(),
            host_pause: self.host_pause.get(),
            isolated_core: self.isolated_core.get(),
            hooks: self.hooks.clone(),
            mmio: self.mmio.clone(),
            controllers: state
                .core_interrupts
                .iter()
                .map(|c| c.as_ref().map(|c| c.device))
                .collect(),
            observations: self.observation_topology(),
            durable_blocker: self.durable_blocker(),
        })
    }
    pub fn restore_keyframe(&mut self, saved: &Keyframe) -> Result<(), String> {
        self.keyframe_owner(saved)?;
        let mut state = self
            .state
            .try_borrow_mut()
            .map_err(|_| "keyframe requires a stopped board")?;
        state.checkpoint_boundary()?;
        if saved.hooks != self.hooks
            || saved.observations != self.observation_topology()
            || saved.mmio != self.mmio
            || saved.controllers
                != state
                    .core_interrupts
                    .iter()
                    .map(|c| c.as_ref().map(|c| c.device))
                    .collect::<Vec<_>>()
            || state.mprofile.iter().map(Option::is_some).ne(saved
                .saved
                .mprofile
                .iter()
                .map(Option::is_some))
            || state.armv7a.iter().map(|p|p.as_ref().map(|p|&p.config)).ne(saved.saved.armv7a.iter().map(|p|p.as_ref().map(|p|&p.config)))
            || state.aprofile.iter().map(Option::is_some).ne(saved
                .saved
                .aprofile
                .iter()
                .map(Option::is_some))
        {
            return Err("keyframe execution wiring differs".into());
        }
        state.bus.validate_checkpoint(&saved.devices)?;
        state.memory.validate_checkpoint(&saved.memory)?;
        state.memory.rearm(&self.cpus)?;
        state.decoders.validate_checkpoint(&saved.decoders)?;
        for (profile, saved_profile) in state.mprofile.iter().zip(&saved.saved.mprofile) {
            if let (Some(profile), Some(saved_profile)) = (profile, saved_profile) {
                profile.validate_checkpoint(saved_profile)?;
            }
        }
        for (profile, saved_profile) in state.aprofile.iter().zip(&saved.saved.aprofile) {
            if let (Some(profile), Some(saved_profile)) = (profile, saved_profile) {
                profile.validate_checkpoint(saved_profile, saved.now())?;
            }
        }
        state
            .exclusive
            .lock()
            .map_err(|_| "exclusive monitors poisoned")?
            .validate_checkpoint(&saved.exclusive)?;
        let result = (|| {
            let memory = state.memory.clone();
            state
                .decoders
                .restore_checkpoint(&saved.decoders, &self.cpus, &memory)?;
            state
                .memory
                .restore_checkpoint(&saved.memory, &self.cpus, &state.exclusive)?;
            state.bus.restore_checkpoint(&saved.devices)?;
            for (cpu, saved) in self.cpus.iter().zip(&saved.processors) {
                cpu.restore(saved.as_ref())?;
            }
            // RAM/TCM restoration can invalidate reservations. The complete
            // physical domain rolls back only after every CPU has restored.
            state
                .exclusive
                .lock()
                .map_err(|_| "exclusive monitors poisoned")?
                .restore_checkpoint(&saved.exclusive)?;
            state.restore_state(&saved.saved)?;
            state.memory.rearm(&self.cpus)?;
            Ok::<_, String>(())
        })();
        if let Err(error) = result {
            // A backend failure can follow partial mapping or RAM restoration.
            // Never execute a partially restored board.
            state.error = Some(format!("keyframe restore failed: {error}"));
            return Err(state.error.as_ref().unwrap().clone());
        }
        *self.entries.borrow_mut() = saved.entries.clone();
        self.next_core.set(saved.next_core);
        self.slice_left.set(saved.slice_left);
        self.quantum.set(saved.quantum);
        self.host_pause.set(saved.host_pause);
        self.isolated_core.set(saved.isolated_core);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
