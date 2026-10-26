//! Plan options of the EXR II scaler model in `fujimod-emulator-exr2-isp`.
use serde::{Deserialize};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub completion_ticks: u64,
}
