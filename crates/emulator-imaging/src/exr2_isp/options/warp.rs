//! Plan options of the EXR II warp model in `fujimod-emulator-exr2-isp`.
use serde::{Deserialize};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub completion_ticks: u64,
    pub stop_ticks: u64,
}
