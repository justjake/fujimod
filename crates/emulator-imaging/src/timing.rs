//! Physical scan-period peer. Zero means that the transmitter has not been
//! configured; positive periods use the board's declared guest timebase.
use serde::Deserialize;
use std::sync::{Arc, Mutex};
pub type FramePeriod = Arc<Mutex<u64>>;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockOutput {
    pub output: String,
    pub ticks_per_second: u64,
}
