//! Concurrent CPU windows with owner-only device and memory barriers.
use super::*;
use fujimod_emulator_cpu::{Clock, RunStatus, RunToken};

#[cfg(test)]
mod tests;

// A cohort survives owner callbacks. Run tokens reset their progress counters,
// but callbacks do not grant a fast peer a new share of the host budget.
pub(super) struct Epoch {
    clock: Clock,
    limit: u64,
    progress: Vec<u64>,
    advanced: u64,
}
impl Epoch {
    fn new(cores: usize, clock: Clock, limit: u64) -> Self {
        Self {
            clock,
            limit,
            progress: vec![0; cores],
            advanced: 0,
        }
    }
    pub(super) fn constrain(
        &mut self,
        timeline: &Timeline,
        until: Option<u64>,
    ) -> Result<(), String> {
        let deadline = timeline
            .next_event()
            .map_or(u64::MAX, |event| event.at)
            .min(until.unwrap_or(u64::MAX));
        self.limit = self.limit.min(self.clock.ratio.budget(
            self.clock.now,
            self.clock.phase,
            deadline,
        )?);
        Ok(())
    }
    pub(super) fn room(&self, core: usize) -> u64 {
        self.limit.saturating_sub(self.progress[core])
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
    done: bool,
    pending: bool,
    exit: Option<Exit>,
}

impl Board {
    /// Every return joins started runs, including failures. A pending callback
    /// is a parked CPU, not permission to touch devices while a peer executes.
    pub(super) fn interactive_window(
        &mut self,
        cores: &[usize],
        budget: usize,
        quantum: usize,
    ) -> Result<(), String> {
        {
            let mut state = self.state.borrow_mut();
            state.parallel = Some(Epoch::new(
                self.cpus.len(),
                Clock {
                    now: state.timeline.now(),
                    phase: state.timeline.phase(),
                    ratio: state.timeline.ratio(),
                },
                budget as u64,
            ));
            state.code_run = None;
        }
        let result = self.interactive_cohort(cores, quantum);
        let mut state = self.state.borrow_mut();
        state.active_slice = None;
        state.parallel = None;
        self.next_core.set((cores[0] + 1) % self.cpus.len());
        self.slice_left.set(0);
        result
    }

    fn interactive_cohort(&mut self, cores: &[usize], quantum: usize) -> Result<(), String> {
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
                    let begin = if scalar(&**cpu, Register::Pstate)? & 0x20 != 0 {
                        pc | 1
                    } else {
                        pc
                    };
                    state.accounted[core] = 0;
                    prepared.push((
                        core,
                        pc,
                        Run {
                            begin,
                            until: None,
                            watchdog_us: 0,
                            instructions: Some(room.min(usize::MAX as u64) as usize),
                        },
                    ));
                }
            }
            if prepared.is_empty() {
                return Ok(());
            }
            self.interactive_segment(prepared, quantum)?;
        }
    }

    // Every segment joins all its workers before returning, including errors.
    fn interactive_segment(
        &mut self,
        prepared: Vec<(usize, u64, Run)>,
        quantum: usize,
    ) -> Result<(), String> {
        let mut entries = Vec::with_capacity(prepared.len());
        let mut error = None;
        for (core, pc, request) in prepared {
            let cpu = &self.cpus[core];
            match cpu.start_run(request) {
                Ok(token) => entries.push(Entry {
                    core,
                    pc,
                    token,
                    done: false,
                    pending: false,
                    exit: None,
                }),
                Err(e) => {
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
                        match result {
                            Ok(exit) => entry.exit = Some(exit),
                            Err(e) => {
                                error.get_or_insert_with(|| e.to_string());
                            }
                        }
                    }
                    Err(e) => {
                        entry.done = true;
                        entry.pending = false;
                        error.get_or_insert_with(|| e.to_string());
                    }
                }
            }
            if !barrier && (error.is_some() || entries.iter().any(|e| e.done || e.pending)) {
                barrier = true;
                for entry in entries.iter().filter(|e| !e.done) {
                    if let Err(e) = self.cpus[entry.core].request_pause(entry.token) {
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
                for entry in &entries {
                    if let Err(e) = state.sync(&*self.cpus[entry.core], entry.core) {
                        error.get_or_insert(e);
                    }
                }
                if let Some(entry) = entries.iter().find(|e| e.pending) {
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
        for entry in &entries {
            if let Err(e) = state.sync(&*self.cpus[entry.core], entry.core) {
                error.get_or_insert(e);
            }
        }
        state.active_slice = None;
        for entry in entries {
            if let Some(exit) = entry.exit {
                if let Err(e) =
                    state.finish_exit(&*self.cpus[entry.core], entry.core, entry.pc, exit)
                {
                    error.get_or_insert(e);
                }
            }
        }
        if let Err(e) = state.apply_memory(&self.cpus) {
            error.get_or_insert(e);
        }
        if let Some(e) = state.error.take() {
            error.get_or_insert(e);
        }
        error.map_or(Ok(()), Err)
    }
}
