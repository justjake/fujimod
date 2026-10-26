//! Plan options of the EXR II metering arithmetic model in `fujimod-emulator-exr2-isp`.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Arithmetic {
    RgbCellMeans12,
}
