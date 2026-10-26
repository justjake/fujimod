//! Plan options of the EXR II phase detection model in `fujimod-emulator-exr2-isp`.
use serde::{Deserialize};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Arithmetic {
    Disabled,
    PairedU16Sad,
    ApertureSad,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub arithmetic: Arithmetic,
    pub data_ticks: u64,
    pub stop_ticks: u64,
}
