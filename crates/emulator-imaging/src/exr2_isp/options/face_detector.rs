//! Plan options of the EXR II face detector model in `fujimod-emulator-exr2-isp`.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(default)]
    pub mode: Mode,
    /// Declared detector results that complete armed requests.
    #[serde(default)]
    pub results: Option<Results>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Configuration,
    RetainedWords,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Results {
    /// Every armed request completes with zero detections.
    Empty,
}
