//! External XP5 controls and GPIO inputs of an opened XP5 session.
use fujimod_emulator_session::Session;
use fujimod_emulator_devices::xp5::pins::SharedPins;
pub use fujimod_emulator_parts_controls::resistor_keys::Key;
use fujimod_emulator_parts_controls::resistor_keys::ResistorKeys;
use std::time::Duration;

/// External XP5 controls retained after device construction, for the session
/// `boot::open` returned with them. Their modeled state is checkpointed by the
/// devices that consume them.
pub struct Inputs {
    pins: SharedPins,
    controls: std::sync::Arc<ResistorKeys>,
    gpio_devices: Vec<usize>,
    control_source: usize,
}

impl Inputs {
    pub(crate) fn new(
        pins: SharedPins,
        controls: std::sync::Arc<ResistorKeys>,
        gpio_devices: Vec<usize>,
        control_source: usize,
    ) -> Self {
        Self {
            pins,
            controls,
            gpio_devices,
            control_source,
        }
    }

    /// Queue a physical XP5 key edge at an absolute guest nanosecond.
    pub fn key(
        &self,
        session: &mut Session,
        at_ns: u64,
        key: Key,
        pressed: bool,
    ) -> Result<(), String> {
        self.controls
            .enqueue(Duration::from_nanos(at_ns), key, pressed)?;
        session.board.inputs_changed(self.control_source)
    }

    /// Drive one external XP5 GPIO input level and wake its registered devices.
    pub fn gpio(
        &self,
        session: &mut Session,
        port: usize,
        pin: usize,
        high: bool,
    ) -> Result<(), String> {
        if port >= 20 || pin >= 16 {
            return Err("XP5 GPIO input is outside the declared pin range".into());
        }
        self.pins
            .lock()
            .map_err(|_| "XP5 GPIO pins poisoned")?
            .set_input(port, pin, high);
        for &device in &self.gpio_devices {
            session.board.inputs_changed(device)?;
        }
        Ok(())
    }

    pub fn gpio_level(&self, port: usize, pin: usize) -> Result<bool, String> {
        if port >= 20 || pin >= 16 {
            return Err("XP5 GPIO input is outside the declared pin range".into());
        }
        Ok(self
            .pins
            .lock()
            .map_err(|_| "XP5 GPIO pins poisoned")?
            .level(port, pin)
            != 0)
    }

    pub fn gpio_input_levels(&self, port: usize) -> Result<u32, String> {
        if port >= 20 {
            return Err("XP5 GPIO port is outside the declared pin range".into());
        }
        Ok(self
            .pins
            .lock()
            .map_err(|_| "XP5 GPIO pins poisoned")?
            .word(port))
    }
}
