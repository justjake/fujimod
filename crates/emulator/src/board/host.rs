//! Stopped-board capabilities for host sessions. No CPU/device callback access.
use super::Board;
use fujimod_emulator_devices::Device;

impl Board {
    /// Complete physical RAM digests without serializing device state.
    pub fn memory_fingerprint(&self) -> Result<serde_json::Value, String> {
        let state = self.state.try_borrow().map_err(|_| "board is executing")?;
        Ok(state.memory.fingerprint())
    }

    pub fn device_states(&self, kind: &str) -> Result<serde_json::Value, String> {
        let state = self.state.try_borrow().map_err(|_| "board is executing")?;
        self.track_device_kind(kind, &state.bus);
        Ok(serde_json::json!(state.bus.device_states(kind)))
    }

    pub fn device_fingerprints(&self, kind: &str) -> Result<serde_json::Value, String> {
        let state = self.state.try_borrow().map_err(|_| "board is executing")?;
        self.track_device_kind(kind, &state.bus);
        Ok(serde_json::json!(state.bus.device_fingerprints(kind)?))
    }

    /// The current execution failure without observing devices.
    pub fn failure(&self) -> Result<Option<String>, String> {
        let state = self.state.try_borrow().map_err(|_| "board is executing")?;
        Ok(state.error.clone())
    }

    pub fn device_buffer(&self, kind: &str, name: &str) -> Result<Vec<u8>, String> {
        let state = self.state.try_borrow().map_err(|_| "board is executing")?;
        self.track_device_kind(kind, &state.bus);
        state.bus.device_buffer(kind, name)
    }

    fn execution_observation(
        &self,
        track_interrupt_controllers: bool,
    ) -> Result<serde_json::Value, String> {
        let s = self.state.try_borrow().map_err(|_| "board is executing")?;
        if track_interrupt_controllers {
            self.track_device_kind("exr2-interrupt-controller", &s.bus);
        }
        Ok(serde_json::json!({
            "execution_mode":s.bus.mode().name(), "execution_policy":s.execution_policy.name(),
            "counts_as_boot":s.bus.mode()==super::bus::Mode::Strict,
            "timeline":s.timeline.save(), "instructions":s.instructions,
            "released":s.released, "waiting":s.waiting, "waiting_event":s.waiting_event,
            "events":s.events, "next_core":self.next_core.get(), "slice_left":self.slice_left.get(),
            "quantum":self.quantum.get(), "failure":s.error,
            "interrupt_controllers":s.bus.device_states("exr2-interrupt-controller")
        }))
    }

    /// CPU/IRQ/scheduler observation without serializing device DMA payloads.
    pub fn observation(&self) -> Result<serde_json::Value, String> {
        self.execution_observation(true)
    }

    /// Inspect the same paused execution state without adding device dependencies.
    pub fn inspection_observation(&self) -> Result<serde_json::Value, String> {
        self.execution_observation(false)
    }

    pub fn inspect_device<T: Device, R>(
        &self,
        id: usize,
        observe: impl FnOnce(&T) -> Result<R, String>,
    ) -> Result<R, String> {
        let state = self.state.try_borrow().map_err(|_| "board is executing")?;
        self.track_device_read(id);
        observe(state.bus.device::<T>(id)?)
    }

    /// A device capability while processors are stopped.
    pub fn inspect_capability<C: fujimod_emulator_devices::capability::Capability + ?Sized, R>(
        &self,
        id: usize,
        observe: impl FnOnce(&C) -> Result<R, String>,
    ) -> Result<R, String> {
        let state = self.state.try_borrow().map_err(|_| "board is executing")?;
        self.track_device_read(id);
        let device = state.bus.device_dyn(id)?;
        observe(C::of(device).ok_or_else(|| missing_capability::<C>(device))?)
    }

    pub fn edit_capability<C: fujimod_emulator_devices::capability::Capability + ?Sized, R>(
        &mut self,
        id: usize,
        edit: impl FnOnce(&mut C) -> Result<R, String>,
    ) -> Result<R, String> {
        let result = {
            let mut state = self
                .state
                .try_borrow_mut()
                .map_err(|_| "board is executing")?;
            let device = state.bus.device_dyn_mut(id)?;
            let message = missing_capability::<C>(device);
            edit(C::of_mut(device).ok_or(message)?)
        };
        self.inputs_changed(id)?;
        result
    }

    /// An external input through a device capability; see `device_input`.
    pub fn capability_input<C: fujimod_emulator_devices::capability::Capability + ?Sized, R>(
        &mut self,
        id: usize,
        input: impl FnOnce(&mut C, &mut fujimod_emulator_devices::Cx<'_>) -> Result<R, String>,
    ) -> Result<R, String> {
        let result = {
            let mut state = self
                .state
                .try_borrow_mut()
                .map_err(|_| "board is executing")?;
            state.with_host(&self.cpus, |bus, host| {
                let device = bus.device_dyn_mut(id)?;
                let message = missing_capability::<C>(device);
                let capability = C::of_mut(device).ok_or(message)?;
                let mut cx = fujimod_emulator_devices::Cx::new(host, usize::MAX, 0, id);
                let result = input(capability, &mut cx);
                Ok(match cx.take_refusal() {
                    Some(error) => Err(error),
                    None => result,
                })
            })
        }
        .and_then(|result| result);
        let refresh = self.inputs_changed(id);
        result.and_then(|value| refresh.map(|()| value))
    }

    /// A device's diagnostic state while processors are stopped.
    pub fn device_save(&self, id: usize) -> Result<serde_json::Value, String> {
        let state = self.state.try_borrow().map_err(|_| "board is executing")?;
        self.track_device_read(id);
        Ok(state.bus.device_dyn(id)?.save())
    }

    pub fn edit_device<T: Device, R>(
        &mut self,
        id: usize,
        edit: impl FnOnce(&mut T) -> Result<R, String>,
    ) -> Result<R, String> {
        let result = {
            let mut state = self
                .state
                .try_borrow_mut()
                .map_err(|_| "board is executing")?;
            edit(state.bus.device_mut::<T>(id)?)
        };
        // Even a refused edit may have changed an input. Re-read only this
        // device and its declared dependents before execution resumes.
        self.inputs_changed(id)?;
        result
    }
    /// Apply an explicit external device input while processors are stopped.
    /// Its context carries guest time, physical DMA and IRQ wires, never CPUs.
    pub fn device_input<T: Device, R>(
        &mut self,
        id: usize,
        input: impl FnOnce(&mut T, &mut fujimod_emulator_devices::Cx<'_>) -> Result<R, String>,
    ) -> Result<R, String> {
        let result = {
            let mut state = self
                .state
                .try_borrow_mut()
                .map_err(|_| "board is executing")?;
            state.with_host(&self.cpus, |bus, host| {
                let device = bus.device_mut::<T>(id)?;
                let mut cx = fujimod_emulator_devices::Cx::new(host, usize::MAX, 0, id);
                let result = input(device, &mut cx);
                // Return the operation's refusal inside the host result so
                // already emitted IRQ effects still wake their controllers.
                Ok(match cx.take_refusal() {
                    Some(error) => Err(error),
                    None => result,
                })
            })
        }
        .and_then(|result| result);
        let refresh = self.inputs_changed(id);
        result.and_then(|value| refresh.map(|()| value))
    }
}

fn missing_capability<C: ?Sized>(device: &dyn Device) -> String {
    format!("{} has no {} capability", device.name(), std::any::type_name::<C>())
}
