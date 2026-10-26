//! Counted run continuation after architectural remaps and code invalidation.
use super::*;

impl Core {
    pub(super) fn execute(&self, request: Run, mut begin: u64) -> Result<Exit, ExecutionError> {
        let e = &self.execution;
        loop {
            e.memory.start(e.id.get())?;
            e.active.set(true);
            let result = e.cpu.run(Run {
                begin,
                instructions: None,
                watchdog_us: 0,
                ..request
            });
            let settlement = if result.is_err() {
                e.scalar(arm::PC)
                    .and_then(|pc| e.settle(e.tail(pc as u64, true), true))
            } else {
                e.settle(0, true)
            };
            let settlement = if result.is_ok() && e.exception.get().is_none() {
                settlement.and_then(|_| e.publish_stores())
            } else {
                e.pending_stores.borrow_mut().clear();
                settlement
            };
            e.active.set(false);
            e.memory.end();
            settlement?;
            if let Some(message) = e.failure.borrow_mut().take() {
                return Err(ExecutionError::Backend(message));
            }
            if let Some(exception) = e.exception.take() {
                crate::rprofile::abort::enter(&*e.cpu, exception)?;
                e.clear_exclusive()?;
                e.observe_exception(exception);
                return Ok(Exit::Stopped);
            }
            if matches!(&result, Err(ExecutionError::InvalidInstruction(_)))
                && crate::rprofile::enter_disabled_vfp(&*e.cpu)?
            {
                e.clear_exclusive()?;
                e.observe_exception(arm::Exception::UndefinedInstruction);
                return Ok(Exit::Stopped);
            }
            let exit = result?;
            let remap = e.tcm_pending.take();
            if let Some((bank, value)) = remap {
                e.apply_tcm(bank, value)?;
            }
            let flush = e.memory.flush_pending();
            if flush {
                e.memory.flush()?;
            }
            let loop_resume = e.loops.resume.replace(false);
            let restarted = e.restarted.get();
            let interactive_retry =
                e.policy.get() == ExecutionPolicy::Interactive && restarted.is_some();
            if (remap.is_some() || flush || loop_resume || restarted.is_some())
                && e.pause_requested.load(Ordering::Acquire) == 0
                && !e.user_stop.get()
                && e.hint.get().is_none()
                && (e.count.get() < e.cap.get() || interactive_retry)
            {
                begin = restarted.unwrap_or(u64::from(e.scalar(arm::PC)?))
                    | u64::from(e.scalar(arm::CPSR)? & 32 != 0);
                continue;
            }
            return Ok(e.hint.get().map_or(exit, Exit::Hint));
        }
    }
}

impl Execution {
    pub(super) fn observe_exception(&self, exception: arm::Exception) {
        for (_, hook) in &self.observer_index.borrow().exceptions {
            if let Hook::Exception { callback, data } = *hook {
                unsafe {
                    callback(exception, data);
                }
            }
        }
    }
}
