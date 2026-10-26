//! Physical connections from masked GPIO drive fields to sensed pads.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Feedback {
    pub port: usize,
    pub register: usize,
    pub bit: usize,
    pub input_port: usize,
    pub input_pin: usize,
}
impl Feedback {
    pub fn validate(&self, ports: usize) -> Result<(), String> {
        if self.port >= ports
            || self.input_port >= ports
            || self.bit >= 16
            || self.input_pin >= 16
            || !matches!(self.register, 0..=4 | 8)
        {
            return Err("invalid GPIO feedback connection".into());
        }
        Ok(())
    }
}

/// A physical pad wired to a level-sensitive processor interrupt input.
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InputIrq {
    pub port: usize,
    pub pin: usize,
    pub line: u32,
    pub active_high: bool,
}
impl InputIrq {
    pub fn validate(&self, ports: usize) -> Result<(), String> {
        if self.port >= ports || self.pin >= 16 {
            return Err("invalid GPIO interrupt pad".into());
        }
        Ok(())
    }
}
