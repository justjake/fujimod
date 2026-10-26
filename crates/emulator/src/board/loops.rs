//! Proven loop windows inherit the scheduler's existing event and peer caps.
use super::*;
use fujimod_emulator_cpu::loops::LoopMode;

impl Board {
    pub fn set_loop_mode(&self, mode: LoopMode) -> Result<(), String> {
        for cpu in self
            .cpus
            .iter()
            .filter(|cpu| cpu.model() == fujimod_emulator_cpu::Model::CortexR5F)
        {
            cpu.set_loop_mode(mode)?;
        }
        Ok(())
    }
    pub fn loop_statistics(&self) -> Result<Value, String> {
        Ok(Value::Array(
            self.cpus
                .iter()
                .map(|cpu| {
                    Ok(json!(
                        cpu.exit_counters()?
                            .into_iter()
                            .filter(|(name, _)| name.starts_with("loop_"))
                            .collect::<std::collections::BTreeMap<_, _>>()
                    ))
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
}

#[cfg(test)]
mod tests;
