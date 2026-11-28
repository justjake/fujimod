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
    /// Virtual ticks between the drive bit rising and the pad reading high.
    /// A fall reaches the pad at once and cancels a pending rise.
    #[serde(default, skip_serializing_if = "no_delay")]
    pub rise_delay: u64,
    /// After its first completed rise the pad stays high: the far end holds
    /// the line once it has answered, whatever the drive bit does later.
    #[serde(default, skip_serializing_if = "not_latched")]
    pub latch_high: bool,
}
fn no_delay(ticks: &u64) -> bool {
    *ticks == 0
}
fn not_latched(latch: &bool) -> bool {
    !*latch
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
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
