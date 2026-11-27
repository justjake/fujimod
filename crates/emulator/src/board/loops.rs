//! Proven loop windows inherit the scheduler's existing event and peer caps.
//!
//! R-profile cores prove and fast-forward their own loops. M-profile cores run
//! on a plain backend under the board's per-instruction entry hook, so the
//! board applies the same instruction-byte proof there: inputs are RAM and
//! device-declared stable registers only, and a window ends at the exact run
//! boundary, which the scheduler places at the first device deadline,
//! interrupt or runnable-peer turn.
use super::*;
use fujimod_emulator_cpu::loop_proof::{Advance, AdvanceFailure, Isa, Proof, State as Registers};
use fujimod_emulator_cpu::loops::LoopMode;
use fujimod_emulator_cpu::{Access, FastMmioRead, arm};
use std::collections::BTreeMap;

/// Loop fast-forward state of the M-profile cores. Host execution state only:
/// it is neither modeled device state nor part of a keyframe.
pub(super) struct MProfileLoops {
    mode: LoopMode,
    /// Device-declared stable reads by processor address, per core.
    reads: Vec<Vec<(u64, FastMmioRead)>>,
    /// The active exact run: its core and the instruction count it ends at.
    /// Deadlines scheduled during the run bound a window as well.
    run_end: Option<(usize, u64)>,
    /// The branch the previous window stopped at executes once normally.
    bypass: Option<(usize, u64)>,
    /// A run a window stopped before its end: its core and that end. The
    /// scheduler resumes it before the run's clock limit applies, as ordinary
    /// execution runs its whole counted budget.
    resume: Option<(usize, u64)>,
    /// Loop bytes that failed structurally or have no iteration model.
    failures: BTreeMap<(usize, u64), Vec<u8>>,
    counters: Vec<BTreeMap<&'static str, u64>>,
}
impl MProfileLoops {
    pub(super) fn new(cores: usize) -> Self {
        Self {
            mode: LoopMode::default(),
            reads: vec![Vec::new(); cores],
            run_end: None,
            bypass: None,
            resume: None,
            failures: BTreeMap::new(),
            counters: vec![BTreeMap::new(); cores],
        }
    }
    pub(super) fn add_reads(&mut self, core: usize, address: u64, reads: &[FastMmioRead]) {
        self.reads[core].extend(reads.iter().map(|read| (address, read.clone())));
    }
    pub(super) fn take_resume(&mut self) -> Option<(usize, u64)> {
        self.resume.take()
    }
    pub(super) fn begin_run(&mut self, core: usize, end: u64) {
        self.run_end = Some((core, end));
    }
    pub(super) fn end_run(&mut self) {
        self.run_end = None;
    }
    fn count(&mut self, core: usize, name: &'static str, n: u64) {
        let value = self.counters[core].entry(name).or_default();
        *value = value.saturating_add(n);
    }
}

impl Board {
    pub fn set_loop_mode(&self, mode: LoopMode) -> Result<(), String> {
        for cpu in self
            .cpus
            .iter()
            .filter(|cpu| cpu.model() == fujimod_emulator_cpu::Model::CortexR5F)
        {
            cpu.set_loop_mode(mode)?;
        }
        let mut state = self
            .state
            .try_borrow_mut()
            .map_err(|_| "loop mode requires a stopped board")?;
        state.mprofile_loops.mode = mode;
        state.mprofile_loops.bypass = None;
        Ok(())
    }
    pub fn loop_statistics(&self) -> Result<Value, String> {
        let state = self.state.borrow();
        Ok(Value::Array(
            self.cpus
                .iter()
                .enumerate()
                .map(|(core, cpu)| {
                    let mut counters = cpu
                        .exit_counters()?
                        .into_iter()
                        .filter(|(name, _)| name.starts_with("loop_"))
                        .collect::<BTreeMap<_, _>>();
                    if state.mprofile[core].is_some() {
                        counters.extend(&state.mprofile_loops.counters[core]);
                    }
                    Ok(json!(counters))
                })
                .collect::<Result<Vec<_>, String>>()?,
        ))
    }
}
impl State {
    pub(super) fn loop_window(&self) -> bool {
        self.bus.loop_window()
            && (self.execution_policy == ExecutionPolicy::Exact
                || self
                    .released
                    .iter()
                    .zip(&self.waiting)
                    .filter(|(released, waiting)| **released && !**waiting)
                    .count()
                    == 1)
    }
    /// Fast-forward an M-profile core whose next instruction, at `pc`, is a
    /// proven T32 loop branch. On success the core stops at that branch with
    /// the skipped instructions charged to its count and virtual time.
    pub(super) fn mprofile_loop(
        &mut self,
        cpu: &dyn Cpu,
        core: usize,
        pc: u64,
    ) -> Result<bool, String> {
        if self.execution_policy != ExecutionPolicy::Exact
            || self.mprofile_loops.mode == LoopMode::Disabled
        {
            return Ok(false);
        }
        let mut word = [0; 4];
        if cpu.read_virtual(pc, Access::Fetch, &mut word).is_err()
            && cpu.read_virtual(pc, Access::Fetch, &mut word[..2]).is_err()
        {
            return Ok(false);
        }
        let word = u32::from_le_bytes(word);
        let isa = Isa::T32;
        let (Some(start), Some(size)) = (
            fujimod_emulator_cpu::loop_proof::loop_start(isa, pc, word),
            fujimod_emulator_cpu::loop_proof::branch_size(isa, pc, word),
        ) else {
            return Ok(false);
        };
        let l = &mut self.mprofile_loops;
        let mut bytes = vec![0; (pc - start + size) as usize];
        if cpu.read_virtual(start, Access::Fetch, &mut bytes).is_err() {
            l.count(core, "loop_proofs_failed_loop_bytes_unavailable", 1);
            return Ok(false);
        }
        if l.failures.get(&(core, pc)) == Some(&bytes) {
            return Ok(false);
        }
        l.count(core, "loop_candidate_entries", 1);
        let end = l
            .run_end
            .filter(|(run, _)| *run == core)
            .map(|(_, end)| end);
        let window = self.parallel.is_none()
            && !cpu.features().instruction_progress
            && self.bus.loop_window();
        let (true, Some(end)) = (window, end) else {
            self.mprofile_loops
                .count(core, "loop_proofs_failed_window_inactive", 1);
            return Ok(false);
        };
        let slice = self.scheduler_room(core);
        let l = &mut self.mprofile_loops;
        if l.bypass.take() == Some((core, pc)) {
            return Ok(false);
        }
        l.count(core, "loop_proofs_attempted", 1);
        let Some(proof) = Proof::decode(isa, start, pc, &bytes) else {
            l.count(core, "loop_proofs_failed_structural", 1);
            l.failures.insert((core, pc), bytes);
            return Ok(false);
        };
        let register = |r: Register| -> Result<u32, String> {
            let mut value = [0; 4];
            cpu.read_register(r, &mut value)?;
            Ok(u32::from_le_bytes(value))
        };
        let mut state = Registers {
            registers: [0; 15],
            cpsr: register(arm::CPSR)?,
        };
        // The proof models a branch outside any IT block.
        if state.cpsr & 0x0600_fc00 != 0 {
            l.count(core, "loop_proofs_failed_it_block", 1);
            return Ok(false);
        }
        for (r, value) in state.registers.iter_mut().enumerate() {
            *value = register(arm::CORE[r])?;
        }
        // The run's remaining instructions, including this branch, ending at
        // the next timeline event and at this core's slice. An access during
        // the run may have scheduled an earlier deadline or made a peer
        // runnable, which bounds the slice the run began without.
        let room = end
            .saturating_sub(self.instructions[core])
            .min(
                self.timeline
                    .processor_budget(core, u64::MAX, self.clock_limit)?,
            )
            .min(slice);
        let reads = &l.reads[core];
        let device_read = std::cell::Cell::new(false);
        let advanced = proof.advance(state, (room / proof.length()).max(1), |address, width| {
            loop_read(cpu, reads, address, width, &device_read)
        });
        let Advance {
            state: after,
            iterations,
            counted,
        } = match advanced {
            Ok(advanced) => advanced,
            Err(reason) => {
                let name = match reason {
                    AdvanceFailure::BudgetRoom => "loop_proofs_failed_budget_room",
                    AdvanceFailure::ConditionNotTaken => {
                        "loop_proofs_failed_loop_condition_not_taken"
                    }
                    AdvanceFailure::ReadUnavailable => "loop_proofs_failed_loop_read_unavailable",
                    AdvanceFailure::InputsChanged => "loop_proofs_failed_inputs_changed",
                    AdvanceFailure::IterationModelUnsupported => {
                        l.failures.insert((core, pc), bytes);
                        "loop_proofs_failed_iteration_model_unsupported"
                    }
                    AdvanceFailure::CounterAtBoundary => "loop_proofs_failed_counter_at_boundary",
                };
                l.count(core, name, 1);
                return Ok(false);
            }
        };
        // A device access stalls a lagging processor to the frontier, so in
        // ordinary execution the first poll read drops the lag the room above
        // includes. A window that reads a device starts at the frontier; one
        // ordinary iteration performs the stall first.
        if device_read.get() && self.timeline.lags()[core] != 0 {
            self.mprofile_loops
                .count(core, "loop_proofs_failed_processor_lag", 1);
            return Ok(false);
        }
        let l = &mut self.mprofile_loops;
        let kind = if counted {
            "loop_counted_instructions"
        } else {
            "loop_poll_instructions"
        };
        l.count(core, kind, proof.length());
        let skipped = iterations * proof.length();
        if l.mode != LoopMode::Accelerate || skipped > room {
            l.count(core, "loop_proofs_failed_budget_room", 1);
            return Ok(false);
        }
        for (r, value) in after.registers.iter().enumerate() {
            if *value != state.registers[r] {
                cpu.write_register(arm::CORE[r], &value.to_le_bytes())?;
            }
        }
        if after.cpsr != state.cpsr {
            cpu.write_register(arm::CPSR, &after.cpsr.to_le_bytes())?;
        }
        l.count(core, kind, skipped - proof.length());
        l.count(core, "loop_accelerated_instructions", skipped);
        l.bypass = Some((core, proof.branch()));
        cpu.write_register(Register::Pc, &(proof.branch() as u32 | 1).to_le_bytes())?;
        self.account(core, skipped)?;
        self.instructions[core] = self.instructions[core]
            .checked_add(skipped)
            .ok_or("instruction counter overflow")?;
        if self.instructions[core] < end {
            self.mprofile_loops.resume = Some((core, end));
        }
        cpu.stop()?;
        Ok(true)
    }
}
/// RAM through an observational host read, or a device-declared stable
/// register at exactly its declared width, which sets `device_read`. Other
/// device reads are refused.
fn loop_read(
    cpu: &dyn Cpu,
    reads: &[(u64, FastMmioRead)],
    address: u32,
    width: usize,
    device_read: &std::cell::Cell<bool>,
) -> Option<u32> {
    if !(address as usize).is_multiple_of(width) {
        return None;
    }
    let mut bytes = [0; 4];
    if cpu
        .read_virtual(u64::from(address), Access::Read, &mut bytes[..width])
        .is_ok()
    {
        return Some(u32::from_le_bytes(bytes));
    }
    reads.iter().find_map(|(base, read)| {
        (u64::from(address) == base + read.register_offset)
            .then(|| read.read(read.register_offset, width as u32))
            .flatten()
            .map(|value| {
                device_read.set(true);
                value as u32
            })
    })
}

#[cfg(test)]
mod tests;
