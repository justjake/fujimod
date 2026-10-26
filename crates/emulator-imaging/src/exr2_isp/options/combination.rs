//! Plan options of the EXR II combination block model in `fujimod-emulator-exr2-isp`.
use serde::{Deserialize, Serialize};
use crate::exr2_isp::dma::Clock;

pub const BASE: u64 = 0xfff48000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Arithmetic {
    Unmodeled,
    PackedImageUnmeasured,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub reset_ticks: u64,
    pub clock: Clock,
    pub arithmetic: Arithmetic,
}
