//! Plan options of the EXR II image input model in `fujimod-emulator-exr2-isp`.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    PackedWrites,
    PackedWords,
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PhaseAck {
    Disabled,
    Inactive,
    Enabled,
    PreserveFields,
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClockMode {
    InputEnable,
    RegisterPeriods,
    SourceMask,
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClockSource {
    Periodic {
        period_ticks: u64,
        first_delay_ticks: Option<u64>,
    },
    Sensor {},
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clock {
    pub mode: ClockMode,
    pub source: ClockSource,
    pub secondary: bool,
    pub companion_secondary: bool,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub reset_ticks: u64,
    pub command_ticks: u64,
    pub phase_ticks: u64,
    pub command: Option<Command>,
    pub command_controls: Vec<u32>,
    pub phase_ack: PhaseAck,
    pub clock: Option<Clock>,
    pub sensor_period_ticks: Option<u64>,
    pub raw_dma_ticks: u64,
    pub readback_ticks: u64,
    pub cfa: Option<Vec<u8>>,
}
