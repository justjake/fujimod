//! Concurrent CPU windows with owner-only device and memory barriers.
use super::*;
use fujimod_emulator_cpu::{Clock, ExecutionError, Fault, Model, RunStatus, RunToken};
use std::time::Instant;

#[cfg(test)]
mod tests;

// A cohort survives owner callbacks. Run tokens reset their progress counters,
// but callbacks do not grant a fast peer a new share of the host budget.
pub(super) struct Epoch {
    clock: Clock,
    limit: u64,
    segment_limit: u64,
    boundary: fujimod_perf::EngineEndReason,
    progress: Vec<u64>,
    advanced: u64,
}
impl Epoch {
    fn new(
        cores: usize,
        clock: Clock,
        limit: u64,
        boundary: fujimod_perf::EngineEndReason,
    ) -> Self {
        Self {
            clock,
            limit,
            segment_limit: limit,
            boundary,
            progress: vec![0; cores],
            advanced: 0,
        }
    }
    pub(super) fn constrain(
        &mut self,
        timeline: &Timeline,
        until: Option<u64>,
    ) -> Result<(), String> {
        let event = timeline.next_event();
        let event_budget = event
            .map(|event| {
                self.clock
                    .ratio
                    .budget(self.clock.now, self.clock.phase, event.at)
            })
            .transpose()?
            .unwrap_or(u64::MAX);
        let until_budget = until
            .map(|until| {
                self.clock
                    .ratio
                    .budget(self.clock.now, self.clock.phase, until)
            })
            .transpose()?
            .unwrap_or(u64::MAX);
        if event_budget <= self.limit && event_budget <= until_budget {
            self.boundary = match event.map(|event| event.target) {
                Some(
                    fujimod_emulator_cpu::timeline::Target::DeviceEvent(_)
                    | fujimod_emulator_cpu::timeline::Target::DeviceDeadline(_),
                ) => fujimod_perf::EngineEndReason::DeviceDeadline,
                _ => fujimod_perf::EngineEndReason::OtherExit,
            };
        } else if until_budget <= self.limit {
            self.boundary = fujimod_perf::EngineEndReason::OtherExit;
        }
        self.limit = self.limit.min(event_budget.min(until_budget));
        Ok(())
    }
    /// Limit the next segment against the least advanced runnable member.
    /// A member already at this limit stays parked until its peers catch up.
    fn bound_segment(&mut self, cores: &[usize], lead: std::num::NonZeroU64) {
        self.segment_limit = self.least_progress(cores).saturating_add(lead.get());
    }
    fn least_progress(&self, cores: &[usize]) -> u64 {
        cores
            .iter()
            .map(|&core| self.progress[core])
            .min()
            .unwrap_or(0)
    }
    pub(super) fn room(&self, core: usize) -> u64 {
        self.limit
            .min(self.segment_limit)
            .saturating_sub(self.progress[core])
    }
    /// Whether the cohort runs another segment. It ends when its first core
    /// reaches the limit: a peer that executed less does not run on alone,
    /// because shared memory shows it every store the exhausted cores made
    /// while those cores cannot answer. A core that leaves a flag rendezvous
    /// and clears the flags for its next round would then strand a peer that
    /// set its flag but had not yet read the others, as firmware cross-core
    /// cache maintenance does. The shortfall is forfeited, as by a slower
    /// core; the next epoch starts every core together at the frontier.
    pub(super) fn open(&self, cores: &[usize]) -> bool {
        cores.iter().all(|&core| self.progress[core] < self.limit)
    }
    fn clock(&self, core: usize) -> Result<Clock, String> {
        self.clock.after(self.progress[core])
    }
    pub(super) fn advance(&mut self, core: usize, delta: u64) -> Result<u64, String> {
        self.progress[core] = self.progress[core]
            .checked_add(delta)
            .ok_or("concurrent processor progress overflow")?;
        let furthest = self.progress.iter().copied().max().unwrap_or(0);
        let delta = furthest.saturating_sub(self.advanced);
        self.advanced = furthest;
        Ok(delta)
    }
}

struct Entry {
    core: usize,
    pc: u64,
    token: RunToken,
    start_instructions: u64,
    instruction_limit: u64,
    boundary: fujimod_perf::EngineEndReason,
    engine_run: fujimod_perf::EngineRunGuard,
    done: bool,
    pending: bool,
    watch_or_observer_stop: bool,
    interrupted_by_scheduler: bool,
    failed: bool,
    data_fault: bool,
    thumb_settled: bool,
    exit: Option<Exit>,
}

impl State {
    pub(super) fn account_interactive(&mut self, core: usize, delta: u64) -> Result<(), String> {
        let epoch = self
            .parallel
            .as_mut()
            .ok_or("no concurrent execution epoch")?;
        let elapsed = epoch.advance(core, delta)?;
        let boundary = self.timeline.ratio().budget(
            self.timeline.now(),
            self.timeline.phase(),
            self.clock_limit.unwrap_or(u64::MAX),
        )?;
        let elapsed = self.timeline.budget(elapsed.min(boundary))?;
        self.timeline.advance_instructions(elapsed)
    }
}

impl Board {
    /// Every return joins started runs, including failures. A pending callback
    /// is a parked CPU, not permission to touch devices while a peer executes.
    pub(super) fn interactive_window(
        &mut self,
        cores: &[usize],
        budget: usize,
        quantum: usize,
        boundary: fujimod_perf::EngineEndReason,
    ) -> Result<(), String> {
        let started = Instant::now();
        {
            let mut state = self.state.borrow_mut();
            // Window processors start together at the frontier.
            state.timeline.park(|_| false);
            state.parallel = Some(Epoch::new(
                self.cpus.len(),
                Clock {
                    now: state.timeline.now(),
                    phase: state.timeline.phase(),
                    ratio: state.timeline.ratio(),
                },
                budget as u64,
                boundary,
            ));
            state.code_run = RunCode::Unknown;
        }
        let result = self.interactive_cohort(cores, quantum);
        let mut state = self.state.borrow_mut();
        state.active_slice = None;
        state.parallel = None;
        state.code_run = RunCode::Unknown;
        state.settled.clear();
        self.next_core.set((cores[0] + 1) % self.cpus.len());
        self.slice_left.set(0);
        drop(state);
        self.interactive_windows
            .set(self.interactive_windows.get().saturating_add(1));
        self.interactive_host_duration_ns.set(
            self.interactive_host_duration_ns
                .get()
                .saturating_add(started.elapsed().as_nanos()),
        );
        result
    }

    fn interactive_cohort(&mut self, cores: &[usize], quantum: usize) -> Result<(), String> {
        let mprofile = cores
            .iter()
            .copied()
            .find(|&core| self.cpus[core].model() == Model::CortexM4);
        loop {
            let mut prepared = Vec::with_capacity(cores.len());
            {
                let mut state = self.state.borrow_mut();
                state.apply_memory(&self.cpus)?;
                state.refresh_processors()?;
                state.due(&self.cpus)?;
                state.push_pins(&self.cpus)?;
                if let Some(error) = state.error.take() {
                    return Err(error);
                }
                if self.host_pause.get()
                    || state.continuation.borrow().fired
                    || cores
                        .iter()
                        .any(|&core| !state.released[core] || state.waiting[core])
                {
                    return Ok(());
                }
                let epoch = state.parallel.as_mut().unwrap();
                if !epoch.open(cores) {
                    return Ok(());
                }
                if let Some(lead) = self.cohort_lead {
                    epoch.bound_segment(cores, lead);
                }
            }
            let mprofile = mprofile
                .filter(|&core| self.state.borrow().parallel.as_ref().unwrap().room(core) > 0);
            let (blocks, _thumb_slice) = if let Some(core) = mprofile {
                let room = self.state.borrow().parallel.as_ref().unwrap().room(core);
                self.prepare_mprofile_segment(core, room)?
            } else {
                (false, None)
            };
            {
                let mut state = self.state.borrow_mut();
                for &core in cores {
                    let epoch = state.parallel.as_ref().unwrap();
                    let room = epoch.room(core);
                    if room == 0 {
                        continue;
                    }
                    let cpu = &self.cpus[core];
                    if cpu.features().guest_clock {
                        cpu.set_guest_clock(epoch.clock(core)?)?;
                    }
                    cpu.set_loop_window(false)?;
                    let pc = scalar(&**cpu, Register::Pc)?;
                    let begin = match cpu.model() {
                        Model::CortexM4 => pc | 1,
                        Model::CortexR5F | Model::CortexA7
                            if scalar(&**cpu, Register::Pstate)? & 0x20 != 0 =>
                        {
                            pc | 1
                        }
                        _ => pc,
                    };
                    let boundary = epoch.boundary;
                    state.accounted[core] = 0;
                    let start_instructions = state.instructions[core];
                    prepared.push((
                        core,
                        pc,
                        start_instructions,
                        room,
                        boundary,
                        Run {
                            begin,
                            until: None,
                            watchdog_us: 0,
                            instructions: (!(mprofile == Some(core) && blocks))
                                .then_some(room.min(usize::MAX as u64) as usize),
                        },
                    ));
                }
            }
            let withheld = cores.len() - prepared.len();
            let result = self.interactive_segment(prepared, quantum, mprofile);
            if let Some(lead) = self.cohort_lead {
                let state = self.state.borrow();
                let epoch = state.parallel.as_ref().unwrap();
                self.bounded_segments += 1;
                self.withheld_lane_grants += withheld as u64;
                let reported_lead = epoch.advanced.saturating_sub(epoch.least_progress(cores));
                self.segments_over_lead += u64::from(reported_lead > lead.get());
            }
            if let Some(core) = mprofile {
                let mut state = self.state.borrow_mut();
                state.end_code_slice(&*self.cpus[core], core);
                state.code_run = RunCode::Unknown;
                state.settled.clear();
            }
            result?;
        }
    }

    fn prepare_mprofile_segment(
        &mut self,
        core: usize,
        room: u64,
    ) -> Result<(bool, Option<thumb_accounting::Slice>), String> {
        self.sync_store_hook(core)?;
        {
            let mut state = self.state.borrow_mut();
            let profile = state.mprofile[core]
                .as_ref()
                .ok_or("M-profile processor requires its architectural controller")?;
            if profile.needs_reset {
                let reset = state.reset_contexts[core]
                    .as_deref()
                    .ok_or("M-profile reset context is missing")?;
                self.cpus[core].restore(reset)?;
                state.mprofile[core]
                    .as_mut()
                    .unwrap()
                    .reset(&*self.cpus[core])?;
                if let Some(shadow) = state.code_shadow[core].as_mut() {
                    shadow.invalidate();
                }
            }
        }
        self.code_resync(core)?;
        {
            let mut state = self.state.borrow_mut();
            let shadow = &mut state.code_shadow[core];
            if shadow
                .as_ref()
                .is_none_or(|shadow| shadow.changed() || shadow.unproven())
            {
                self.cpus[core].invalidate_code(None)?;
                if let Some(shadow) = shadow {
                    shadow.discarded();
                }
            }
            if let Some(shadow) = shadow {
                shadow.peer_ran();
                shadow.begin_slice();
            }
            // Guest writes and effects that can change these pages rendezvous
            // at the owner. The shadow is checked again after every segment.
            state.code_run = RunCode::Running(core);
            state.code_write_deferred = false;
            state.run_entries[core] = 0;
        }
        let (blocks, thumb_slice) = self.sync_block_mode(core, room)?;
        self.state.borrow_mut().blocks[core]
            .as_mut()
            .unwrap()
            .poll_enabled = false;
        let Some(thumb_slice) = thumb_slice else {
            return Ok((blocks, None));
        };
        let mut state = self.state.borrow_mut();
        let cpu = &*self.cpus[core];
        // The owner's deadline and pin push ran before this preparation.
        // All device/DMA and guest MMIO callbacks pause the cohort.
        state.settled.pass = true;
        state.arm_settled()?;
        let active_exception = state.mprofile[core].as_ref().unwrap().active_exception();
        let interrupt_ready = state.mprofile[core].as_mut().unwrap().pending();
        if !interrupt_ready {
            state.settled.idle = Some(core);
            let pc = scalar(cpu, Register::Pc)? & !1;
            if !state.enter_code(cpu, core, pc, 4)?
                && state.code_shadow[core]
                    .as_ref()
                    .and_then(|shadow| shadow.fetch(pc))
                    .is_some()
            {
                let blocks = state.blocks[core].as_mut().unwrap();
                blocks.inline.arm(
                    pc,
                    room,
                    active_exception,
                    blocks.code_stores_notified,
                    true,
                );
            }
        }
        Ok((blocks, Some(thumb_slice)))
    }

    // Every segment joins all its workers before returning, including errors.
    fn interactive_segment(
        &mut self,
        prepared: Vec<(usize, u64, u64, u64, fujimod_perf::EngineEndReason, Run)>,
        quantum: usize,
        mprofile: Option<usize>,
    ) -> Result<(), String> {
        let mut entries = Vec::with_capacity(prepared.len());
        let mut error = None;
        for (core, pc, start_instructions, instruction_limit, boundary, request) in prepared {
            let cpu = &self.cpus[core];
            let engine_run = fujimod_perf::engine_run(u8::try_from(core).unwrap_or(u8::MAX), true);
            match cpu.start_run(request) {
                Ok(token) => entries.push(Entry {
                    core,
                    pc,
                    token,
                    start_instructions,
                    instruction_limit,
                    boundary,
                    engine_run,
                    done: false,
                    pending: false,
                    watch_or_observer_stop: false,
                    interrupted_by_scheduler: false,
                    failed: false,
                    data_fault: false,
                    thumb_settled: false,
                    exit: None,
                }),
                Err(e) => {
                    engine_run.cancel();
                    error = Some(e.to_string());
                    break;
                }
            }
        }
        let mut barrier = false;
        loop {
            let generation = self.run_wakeup.generation();
            let mut running = false;
            for entry in entries.iter_mut().filter(|e| !e.done) {
                match self.cpus[entry.core].poll_run(entry.token) {
                    Ok(RunStatus::Running) => {
                        entry.pending = false;
                        running = true;
                    }
                    Ok(RunStatus::CallbackPending) => entry.pending = true,
                    Ok(RunStatus::Complete(result)) => {
                        entry.done = true;
                        entry.pending = false;
                        entry.engine_run.stop();
                        entry.engine_run.set_instructions(
                            self.cpus[entry.core].instruction_progress().unwrap_or(0),
                        );
                        entry.watch_or_observer_stop = self.host_pause.get()
                            || self.state.borrow().continuation.borrow().fired;
                        match result {
                            Ok(exit) => {
                                entry.exit = Some(exit);
                                if matches!(exit, Exit::Stopped)
                                    && !entry.watch_or_observer_stop
                                    && !entry.interrupted_by_scheduler
                                    && self.cpus[entry.core].exhausted_grant(entry.token)
                                {
                                    self.local_grant_completions += 1;
                                }
                            }
                            Err(e) => {
                                entry.failed = true;
                                entry.data_fault = matches!(
                                    e,
                                    ExecutionError::Memory {
                                        fault: Fault::ReadUnmapped
                                            | Fault::WriteUnmapped
                                            | Fault::ReadProtected
                                            | Fault::WriteProtected,
                                        ..
                                    }
                                );
                                error.get_or_insert_with(|| e.to_string());
                            }
                        }
                    }
                    Err(e) => {
                        entry.done = true;
                        entry.pending = false;
                        entry.engine_run.stop();
                        entry.engine_run.set_instructions(
                            self.cpus[entry.core].instruction_progress().unwrap_or(0),
                        );
                        entry.watch_or_observer_stop = self.host_pause.get()
                            || self.state.borrow().continuation.borrow().fired;
                        entry.failed = true;
                        error.get_or_insert_with(|| e.to_string());
                    }
                }
            }
            if !barrier && (error.is_some() || entries.iter().any(|e| e.done || e.pending)) {
                barrier = true;
                self.cohort_pause_barriers += u64::from(entries.iter().any(|entry| !entry.done));
                // The M-profile owner must finish admitting a shortened
                // block when its own observer opened the barrier. Its
                // budget and subsequent owner callbacks remain bounded by
                // this epoch. A peer callback still preempts it.
                let mprofile_admitting = error.is_none()
                    && entries
                        .iter()
                        .any(|entry| mprofile == Some(entry.core) && entry.pending)
                    && !entries
                        .iter()
                        .any(|entry| mprofile != Some(entry.core) && (entry.pending || entry.done));
                for entry in entries.iter_mut().filter(|e| !e.done) {
                    if mprofile_admitting && mprofile == Some(entry.core) {
                        continue;
                    }
                    entry.interrupted_by_scheduler = true;
                    if let Err(e) = self.cpus[entry.core].request_pause(entry.token) {
                        entry.failed = true;
                        error.get_or_insert_with(|| e.to_string());
                    }
                }
            }
            if entries.iter().all(|e| e.done) {
                break;
            }
            if barrier && !running {
                // All assembly execution is stopped. Account the whole cluster
                // before a callback can settle timers or publish DMA.
                let mut state = self.state.borrow_mut();
                for entry in &mut entries {
                    if let Err(e) = self.settle_interactive_entry(&mut state, entry, mprofile) {
                        error.get_or_insert(e);
                    }
                }
                if let Some(entry) = entries
                    .iter()
                    .filter(|e| e.pending)
                    .min_by_key(|entry| mprofile != Some(entry.core))
                {
                    match schedule::Slice::new(
                        entry.core,
                        state.instructions[entry.core],
                        quantum,
                        quantum,
                    ) {
                        Ok(slice) => state.active_slice = Some(slice),
                        Err(e) => {
                            error.get_or_insert(e);
                        }
                    }
                    drop(state);
                    let result = if error.is_some() {
                        self.cpus[entry.core].cancel_pending_callback(entry.token)
                    } else {
                        self.cpus[entry.core].service_pending_callback(entry.token)
                    };
                    if let Err(e) = result {
                        error.get_or_insert_with(|| e.to_string());
                    }
                    // The response allows this CPU to retire its access and
                    // honor the sticky pause before another callback is served.
                    continue;
                }
            }
            self.run_wakeup.wait_for_change(generation);
        }
        let mut state = self.state.borrow_mut();
        for entry in &mut entries {
            if let Err(e) = self.settle_interactive_entry(&mut state, entry, mprofile) {
                error.get_or_insert(e);
            }
        }
        state.active_slice = None;
        for mut entry in entries {
            if let Some(exit) = entry.exit
                && let Err(e) =
                    state.finish_exit(&*self.cpus[entry.core], entry.core, entry.pc, exit)
            {
                entry.failed = true;
                error.get_or_insert(e);
            }
            let instructions =
                state.instructions[entry.core].saturating_sub(entry.start_instructions);
            let reason = super::engine_end_reason(
                entry.exit,
                instructions,
                entry.instruction_limit,
                entry.boundary,
                entry.watch_or_observer_stop,
                entry.failed,
                entry.interrupted_by_scheduler,
            );
            entry.engine_run.set_instructions(instructions);
            entry.engine_run.finish(reason);
        }
        if let Err(e) = state.apply_memory(&self.cpus) {
            error.get_or_insert(e);
        }
        if let Some(e) = state.error.take() {
            error.get_or_insert(e);
        }
        error.map_or(Ok(()), Err)
    }

    fn settle_interactive_entry(
        &self,
        state: &mut State,
        entry: &mut Entry,
        mprofile: Option<usize>,
    ) -> Result<(), String> {
        let core = entry.core;
        let cpu = &*self.cpus[core];
        if mprofile == Some(core) && entry.done && !entry.thumb_settled {
            entry.thumb_settled = true;
            if entry.data_fault {
                state.settle_interrupted_block(cpu, core, scalar(cpu, Register::Pc)? & !1, true)?;
            } else {
                state.settle_block(core, None)?;
            }
        }
        state.sync(cpu, core)
    }
}
