use super::Entry;
use crate::board::{Board, RunCode, scalar};
use fujimod_emulator_cpu::{Access, ExecutionError, Exit, Fault, Register, Run, arm::thumb};

impl Board {
    /// Re-observe a translated M-profile processor without handing its turn
    /// to a peer at every native exit.
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
                if remaining == 0
                    || s.scheduler_room(core) == 0
                    || s.error.is_some()
                    || s.continuation.borrow().fired
                    || self.host_pause.get()
                {
                    break;
                }
                s.due(&self.cpus)?;
                if s.decoders.pending() || !s.released[core] || self.host_pause.get() {
                    break;
                }
                if !s.code_run.is_running(core) {
                    cpu.revalidate_code()?;
                    s.code_run = RunCode::Running(core);
                }
                let settled = s.settled();
                if !settled {
                    s.push_pins(&self.cpus)?;
                    s.refresh_processors()?;
                    s.settle_after_pin_push(core)?;
                }
                let settled = s.settled();
                let (budget, _) = s.admission_room(core, remaining, settled)?;
                if budget == 0 || s.scheduler_room(core) == 0 {
                    break;
                }
                let idle = settled && s.settled.idle == Some(core);
                let profile = s.mprofile[core].as_mut().unwrap();
                let entry = if profile.take_pending(cpu, pc, idle)? {
                    Entry::Redirect
                } else {
                    let mut bytes = [0; 2];
                    cpu.read_virtual(pc, Access::Fetch, &mut bytes)
                        .map_err(|message| ExecutionError::Memory {
                            fault: Fault::FetchUnmapped,
                            message,
                        })?;
                    profile.before_instruction(cpu, pc, u16::from_le_bytes(bytes))?
                };
                let stop = profile.take_stopped();
                if profile.take_alias_write() {
                    s.code_run = RunCode::Unknown;
                }
                if !matches!(entry, Entry::Execute) {
                    s.settled.clear();
                    s.refresh_processors()?;
                    if matches!(entry, Entry::Redirect) {
                        // Like the instruction and block observers, use an
                        // admission when an interrupt replaces the entry.
                        // The redirected instruction does not advance time.
                        redirects += 1;
                        continue;
                    }
                    s.account(core, 1)?;
                    s.instructions[core] = s.instructions[core]
                        .checked_add(1)
                        .ok_or("instruction counter overflow")?;
                    if matches!(entry, Entry::Wait) {
                        s.waiting[core] = true;
                        break;
                    }
                    if stop {
                        break;
                    }
                    continue;
                }
                s.accounted[core] = 0;
                budget
            };
            let exit = cpu.run_mprofile_batch(Run {
                begin: pc | 1,
                until: None,
                watchdog_us: 0,
                instructions: Some(budget as usize),
            });
            let mut s = self.state.borrow_mut();
            s.sync(cpu, core)?;
            if s.error.is_some() {
                break;
            }
            match exit? {
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
                Exit::Stopped if cpu.instruction_progress() != Some(0) => continue,
                other => return Ok(other),
            }
            s.settled.clear();
            s.refresh_processors()?;
        }
        Ok(Exit::Stopped)
    }
}
