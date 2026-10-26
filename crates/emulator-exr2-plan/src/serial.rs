//! Options of the EXR II boot serial register blocks.
use fujimod_emulator_devices::parts::pins::Pin;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PinLevel {
    pub pin: Pin,
    pub high: bool,
}
/// An explicitly fixed-low input for a selected byte-wide serial connection.
/// Presence is a second physical pad constraint, not a firmware predicate.
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixedLowByteInput {
    pub required: [u32; 3],
    pub selection: PinLevel,
    pub presence: PinLevel,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub transfer_ticks: u64,
    pub max_program_bytes: usize,
    pub spi0: Option<FixedLowByteInput>,
}
