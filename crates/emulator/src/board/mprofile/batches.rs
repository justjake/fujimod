use super::Entry;
use crate::board::{Board, RunCode, State, scalar};
use fujimod_emulator_cpu::{
    Cpu, ExecutionError, Exit, MProfileAction, MProfileObservation, Register, Run, arm::thumb,
};

/// Check the board's scheduling and interrupt boundaries before admitting
/// another group of Thumb instructions.
fn admit(
    board: &Board,
    s: &mut State,
    cpu: &dyn Cpu,
    core: usize,
    pc: u64,
    remaining: u64,
) -> Result<Admission, String> {
    if remaining == 0
        || s.scheduler_room(core) == 0
        || s.error.is_some()
        || s.continuation.borrow().fired
        || board.host_pause.get()
    {
        return Ok(Admission::Stop);
    }
    s.due(&board.cpus)?;
    if s.decoders.pending() || !s.released[core] || board.host_pause.get() {
        return Ok(Admission::Stop);
    }
    if !s.code_run.is_running(core) {
        if !s.memory.all_code_writers_report {
            cpu.revalidate_code()?;
        }
        s.code_run = RunCode::Running(core);
    }
    if !s.settled() {
        s.push_pins(&board.cpus)?;
        s.refresh_processors()?;
        s.settle_after_pin_push(core)?;
    }
    let settled = s.settled();
    let (budget, _) = s.admission_room(core, remaining, settled)?;
    if budget == 0 || s.scheduler_room(core) == 0 {
        return Ok(Admission::Stop);
    }
    let idle = settled && s.settled.idle == Some(core);
    let profile = s.mprofile[core].as_mut().unwrap();
    let redirected = profile.take_pending(cpu, pc, idle)?;
    if profile.take_alias_write() {
        s.code_run = RunCode::Unknown;
    }
    if redirected {
        s.settled.clear();
        s.refresh_processors()?;
        return Ok(Admission::Redirect);
    }
    Ok(Admission::Run(budget))
}

enum Admission {
    Run(u64),
    Redirect,
    Stop,
}

impl Board {
    pub(in crate::board) fn run_mprofile_batches(
        &self,
        core: usize,
        limit: u64,
    ) -> Result<Exit, ExecutionError> {
        let cpu = &*self.cpus[core];
        let start = {
            let mut s = self.state.borrow_mut();
            s.accounted[core] = cpu.instruction_progress().unwrap_or(0);
            s.instructions[core]
        };
        let mut redirects = 0;
        loop {
            let pc = scalar(cpu, Register::Pc)?;
            let budget = {
                let mut s = self.state.borrow_mut();
                let remaining = limit.saturating_sub(s.instructions[core] - start + redirects);
                match admit(self, &mut s, cpu, core, pc, remaining)? {
                    Admission::Run(budget) => {
                        s.accounted[core] = 0;
                        budget
                    }
                    Admission::Redirect => {
                        // Admission itself consumes room, but does not advance guest time.
                        redirects += 1;
                        continue;
                    }
                    Admission::Stop => break,
                }
            };
            let mut finish_slice = false;
            let previous_redirects = redirects;
            let mut observe = |observation| {
                let mut s = self.state.borrow_mut();
                s.sync(cpu, core)?;
                let pc = u64::from(match observation {
                    MProfileObservation::Instruction { pc, .. }
                    | MProfileObservation::Recheck { pc } => pc,
                });
                let remaining = limit.saturating_sub(s.instructions[core] - start + redirects);
                match admit(self, &mut s, cpu, core, pc, remaining)? {
                    Admission::Run(budget) => {
                        let entered = cpu.instruction_progress().unwrap();
                        cpu.tighten_instruction_limit(entered.saturating_add(budget))?;
                    }
                    Admission::Redirect => {
                        redirects += 1;
                        return Ok(MProfileAction::Stop);
                    }
                    Admission::Stop => {
                        finish_slice = true;
                        return Ok(MProfileAction::Stop);
                    }
                }
                let MProfileObservation::Instruction { first, .. } = observation else {
                    return Ok(MProfileAction::Execute);
                };
                let profile = s.mprofile[core].as_mut().unwrap();
                let entry = profile.before_instruction(cpu, pc, first)?;
                finish_slice = profile.take_stopped() || matches!(entry, Entry::Wait);
                if profile.take_alias_write() {
                    s.code_run = RunCode::Unknown;
                }
                if matches!(entry, Entry::Execute) {
                    return Ok(MProfileAction::Execute);
                }
                s.settled.clear();
                s.refresh_processors()?;
                s.account(core, 1)?;
                s.instructions[core] = s.instructions[core]
                    .checked_add(1)
                    .ok_or("instruction counter overflow")?;
                // The backend will include the emulated instruction in its
                // progress. The board already charged it before entering wait.
                s.accounted[core] += 1;
                if matches!(entry, Entry::Wait) {
                    s.waiting[core] = true;
                }
                Ok(MProfileAction::Emulated)
            };
            let exit = cpu.run_mprofile_batch(
                Run {
                    begin: pc | 1,
                    until: None,
                    watchdog_us: 0,
                    instructions: Some(budget as usize),
                },
                &mut observe,
            );
            let mut s = self.state.borrow_mut();
            s.sync(cpu, core)?;
            if s.error.is_some() {
                break;
            }
            let exit = exit?;
            if finish_slice {
                break;
            }
            match exit {
                Exit::MProfileExceptionReturn(target) => {
                    s.mprofile[core]
                        .as_mut()
                        .unwrap()
                        .exception_return(cpu, target)?;
                }
                Exit::Trap(trap) if matches!(trap.syndrome, thumb::INVSTATE | thumb::NOCP) => {
                    let profile = s.mprofile[core].as_mut().unwrap();
                    profile.usage_fault(cpu, trap.syndrome as u32, trap.virtual_address as u32)?;
                    if profile.take_alias_write() {
                        s.code_run = RunCode::Unknown;
                    }
                }
                Exit::Stopped
                    if redirects != previous_redirects || cpu.instruction_progress() != Some(0) =>
                {
                    continue;
                }
                other => return Ok(other),
            }
            s.settled.clear();
            s.refresh_processors()?;
        }
        Ok(Exit::Stopped)
    }
}
