//! Whole-board host orchestration over the existing XP4 execution law.
use super::*;

impl Session {
    pub fn instructions(&self) -> Vec<u64> {
        unsafe { (*self.inner.state.get()).per_core.clone() }
    }
    pub fn now(&self) -> u64 {
        unsafe { (*self.inner.state.get()).ticks }
    }
    pub fn quiescent(&self) -> bool {
        self.quiescent.get()
    }
    pub fn read_register(&self, core: usize, register: Register) -> Result<u64, String> {
        self.inner.cpu(core)?;
        self.inner.reg(core, register).map(u64::from)
    }
    pub fn write_register(
        &self,
        core: usize,
        register: Register,
        value: u64,
    ) -> Result<(), String> {
        self.inner.cpu(core)?;
        let value = u32::try_from(value)
            .map_err(|_| "ARMv7-A scalar register exceeds 32 bits".to_owned())?;
        self.inner.write_reg(core, register, value)
    }
    pub fn coprocessor(
        &self,
        core: usize,
        selector: arm::Coprocessor,
        value: Option<u64>,
    ) -> Result<u64, String> {
        self.inner.cpu(core)?;
        if selector.cp == 15 && !selector.wide && selector.secure == 0 && value.is_none() {
            self.inner.cp(
                core,
                selector.crn,
                selector.crm,
                selector.opc1,
                selector.opc2,
            )
        } else {
            self.inner.cpus[core].coprocessor(selector, value)
        }
    }
    pub fn run_board(&self, limit: u64, quantum: usize) -> Result<(), String> {
        if self.inner.running.get() {
            return Err("Reentrant ARMv7-A execution".to_owned());
        }
        if quantum == 0 {
            return Err("Board quantum must be positive".to_owned());
        }
        let target = unsafe { (*self.inner.state.get()).instructions }
            .checked_add(limit)
            .ok_or_else(|| "Instruction budget overflows".to_owned())?;
        loop {
            let state = unsafe { &*self.inner.state.get() };
            if state.instructions >= target || state.failure.is_some() {
                return Ok(());
            }
            let remaining = target - state.instructions;
            let runnable: Vec<_> = state
                .waiting
                .iter()
                .enumerate()
                .filter_map(|(core, waiting)| (*waiting == 0).then_some(core))
                .collect();
            if runnable.is_empty() {
                if self.idle() {
                    continue;
                }
                self.quiescent.set(true);
                return Ok(());
            }
            let core = runnable
                .iter()
                .copied()
                .find(|core| *core >= self.next_core.get())
                .unwrap_or(runnable[0]);
            self.next_core.set((core + 1) % self.inner.cpus.len());
            self.run(
                core,
                quantum.min(usize::try_from(remaining).unwrap_or(usize::MAX)),
            )?;
        }
    }
}
