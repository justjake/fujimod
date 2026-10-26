//! Plan options of the EXR II window analysis model in `fujimod-emulator-exr2-isp`.
use serde::{Deserialize};

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arithmetic {
    Y16GradientsAndLocalTracking,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub arithmetic: Arithmetic,
    pub ticks_per_window: u64,
}
