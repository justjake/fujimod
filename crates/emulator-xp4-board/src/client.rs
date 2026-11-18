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
        // The instruction observer may change registers while its core is live.
        // A failed board never accepts a host repair through that capability.
        self.inner.host_register_mutation(core)?;
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
        if value.is_some() {
            self.inner.host_register_mutation(core)?;
        }
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
        self.run_board_window(limit, quantum, None)
    }

    pub fn run_board_window(&self, limit: u64, quantum: usize, until: Option<u64>) -> Result<(), String> {
        self.inner.host_mutation()?;
        if quantum == 0 {
            return Err("Board quantum must be positive".to_owned());
        }
        let target = unsafe { (*self.inner.state.get()).instructions }
            .checked_add(limit)
            .ok_or_else(|| "Instruction budget overflows".to_owned())?;
        loop {
            self.inner.healthy()?;
            let state = unsafe { &*self.inner.state.get() };
            if state.instructions >= target || until.is_some_and(|at| state.ticks >= at) {
                return Ok(());
            }
            let mut remaining = target - state.instructions;
            if let Some(until) = until {
                remaining = remaining.min(self.inner.instruction_rate.budget(state.ticks, state.tick_phase, until)?);
            }
            let runnable: Vec<_> = state
                .waiting
                .iter()
                .enumerate()
                .filter_map(|(core, waiting)| (*waiting == 0).then_some(core))
                .collect();
            if runnable.is_empty() {
                if self.idle_until(until) {
                    continue;
                }
                self.inner.healthy()?;
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

    pub fn set_scene(&self, scene: &serde_json::Value) -> Result<(), String> {
        self.inner.external_devices(|devices, _| devices.set_scene(scene))
    }

    pub fn write_patch(&self, address: usize, bytes: &[u8]) -> Result<(), String> {
        self.write(address, bytes)?;
        for cpu in &self.inner.cpus {
            cpu.invalidate_code(None)?;
        }
        Ok(())
    }

    pub fn begin_pc_trace(&self, limit: usize) -> Result<(), String> {
        self.inner.host_mutation()?;
        if self.inner.counted {
            return Err(counted::NEEDS_EXACT.into());
        }
        let state = unsafe { &mut *self.inner.state.get() };
        if state.pc_trace.is_some() {
            return Err("PC trace is already attached".into());
        }
        state.pc_trace = Some(fujimod_emulator::board::observation::PcRecorder::new(limit)?);
        Ok(())
    }

    pub fn take_pc_trace(&self) -> Result<fujimod_emulator::board::observation::PcEntries, String> {
        self.inner.host_mutation()?;
        let state = unsafe { &mut *self.inner.state.get() };
        state.pc_trace.take().map(|mut trace| trace.take()).ok_or("no active PC trace".into())
    }

}
