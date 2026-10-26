//! A timed touch part attached to the primary bus's prefixed-register endpoint.
use fujimod_emulator_devices::parts::touch_input::ContactInput;
use crate::touch_controller::TouchController;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub register: u16,
    pub core: usize,
    pub irq: u8,
    pub ticks_per_second: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Panel {
    pub options: Options,
    pub controller: TouchController,
}
impl Panel {
    pub fn new(options: Options) -> Result<Self, String> {
        if options.core >= 2 || options.irq >= 128 {
            return Err("invalid touch interrupt connection".into());
        }
        let controller = TouchController::new(options.ticks_per_second)?;
        Ok(Self {
            options,
            controller,
        })
    }
    pub fn inject(&mut self, now: u64, script: &[ContactInput]) -> Result<(), String> {
        self.controller.inject(now, script)
    }
}
