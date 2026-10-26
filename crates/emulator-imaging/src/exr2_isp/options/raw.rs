//! Plan options of the EXR II RAW blocks model in `fujimod-emulator-exr2-isp`.
use serde::{Deserialize};

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterOptions {}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub completion_ticks: u64,
    pub reset_ticks: u64,
}

impl Options {
    pub fn validate(&self) -> Result<(), String> {
        if self.completion_ticks == 0 || self.reset_ticks == 0 {
            Err("RAW timing must be positive".into())
        } else {
            Ok(())
        }
    }
}
