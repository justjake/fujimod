//! Command-dial encoder wiring over the X-Processor Pro GPIO bank.
//!
//! The generic command-dial model in `fujimod-emulator-parts-controls` drives
//! declared pads and emits Gray-code phases; this adapter presents the SoC's
//! masked GPIO bank through that model's pad interface, so the encoder writes
//! the bank's external input levels and the firmware's own polling decodes
//! them.
use fujimod_emulator_devices::Cx;
use fujimod_emulator_devices::parts::pins::{Pin, Pins};
use fujimod_emulator_ip::socionext::gpio::SharedPins;

/// One board GPIO bank seen as physical pads by a part.
pub struct BankPins {
    pins: SharedPins,
}

impl BankPins {
    pub fn new(pins: SharedPins) -> Self {
        Self { pins }
    }
}

impl Pins for BankPins {
    fn output_level(&self, pin: Pin) -> Result<Option<bool>, String> {
        pin.validate()?;
        let pins = self.pins.lock().map_err(|_| "GPIO pins poisoned")?;
        Ok(Some(
            pins.output(usize::from(pin.port), usize::from(pin.pin)).0,
        ))
    }
    fn input(&mut self, _cx: &mut Cx<'_>, pin: Pin, high: bool) -> Result<(), String> {
        pin.validate()?;
        if pin.port >= 16 || pin.pin >= 16 {
            return Err("command-dial pad is outside the GPIO banks".into());
        }
        self.pins
            .lock()
            .map_err(|_| "GPIO pins poisoned")?
            .set_input(usize::from(pin.port), usize::from(pin.pin), high);
        Ok(())
    }
}
