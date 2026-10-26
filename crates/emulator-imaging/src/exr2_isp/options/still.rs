//! Plan options of the EXR II still processor model in `fujimod-emulator-exr2-isp`.
use serde::{Deserialize};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub completion_ticks: u64,
    pub cfa: Vec<u8>,
}
