//! Plan options of the EXR II FFF11000 block model in `fujimod-emulator-exr2-isp`.
use serde::{Deserialize, Serialize};

/// Selects whether the observed control 3-to-2 transition is acknowledged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlMode {
    #[default]
    RegisterOnly,
    ClearBit0Ack {
        delay_ticks: u64,
    },
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(default)]
    pub control_mode: ControlMode,
}
