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
            let mut runnable = 0usize;
            let mut first = None;
            let mut from_next = None;
            for (core, waiting) in state.waiting.iter().enumerate() {
                if *waiting == 0 {
                    runnable += 1;
                    first.get_or_insert(core);
                    if core >= self.next_core.get() {
                        from_next.get_or_insert(core);
                    }
                }
            }
            if runnable == 0 {
                if self.idle_until(until) {
                    continue;
                }
                self.inner.healthy()?;
                self.quiescent.set(true);
                return Ok(());
            }
            let core = from_next.or(first).unwrap();
            self.next_core.set((core + 1) % self.inner.cpus.len());
            unsafe {
                let state = &mut *self.inner.state.get();
                state.quantum = quantum as u64;
                state.solo_window = if runnable == 1 { remaining } else { 0 };
            }
            let result = self.run(
                core,
                quantum.min(usize::try_from(remaining).unwrap_or(usize::MAX)),
            );
            unsafe { (*self.inner.state.get()).solo_window = 0 };
            result?;
        }
    }

    /// Attach explicit ARM32 routine substitutions to a discovery session.
    pub fn install_discovery_callee_stubs(&self, stubs: &[CalleeStub]) -> Result<(), String> {
        if stubs.is_empty() {
            return Ok(());
        }
        self.inner.host_mutation()?;
        let state = unsafe { &mut *self.inner.state.get() };
        if self.inner.counted || !state.discovery.enabled || !state.callee_stubs.is_empty() || stubs.len() > 4096 {
            return Err("XP4 callee substitutions require a fresh exact discovery hook set".into());
        }
        let mut entries = BTreeMap::new();
        for stub in stubs {
            if stub.core >= self.inner.cpus.len() || stub.address & 1 != 0
                || u32::try_from(stub.address).is_err() || u32::try_from(stub.value).is_err()
                || entries.insert((stub.core, stub.address), (stub.clone(), 0)).is_some()
            {
                return Err("invalid or duplicate XP4 discovery callee substitution".into());
            }
        }
        state.callee_stubs = entries;
        Ok(())
    }

    pub fn discovery_callee_stubs(&self) -> serde_json::Value {
        let state = unsafe { &*self.inner.state.get() };
        serde_json::json!(state.callee_stubs.values().map(|(stub, hits)| serde_json::json!({
            "kind":"callee_stub", "name":stub.name, "core":stub.core, "address":format!("{:#x}", stub.address),
            "value":stub.value, "hits":hits,
        })).collect::<Vec<_>>())
    }

    pub fn set_scene(&self, scene: &serde_json::Value) -> Result<(), String> {
        self.inner.external_devices(|devices, _| devices.set_scene(scene))
    }

    /// Resolve an application processor address through its stopped MMU view.
    pub fn translate(&self, core: usize, address: u64, access: Access) -> Result<u64, String> {
        self.inner.cpu(core)?.translate(address, access)
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
