//! The sensor calibration, exposure and aperture that radiometric sensor input reads.
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Calibration {
    pub full_scale: u16,
    pub reference_white: [u16; 3],
    pub saturation_lux_seconds: f64,
    pub color_sample_weights: [u8; 3],
}
impl Calibration {
    pub fn validate(self) -> Result<Self, String> {
        if self.full_scale == 0
            || self.reference_white.contains(&0)
            || !self.saturation_lux_seconds.is_finite()
            || self.saturation_lux_seconds <= 0.
            || self.color_sample_weights.contains(&0)
        {
            return Err("invalid radiometric sensor calibration".into());
        }
        Ok(self)
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Exposure {
    pub seconds: f64,
    pub analog_gain: f64,
}
pub type ExposureHandle = Arc<Mutex<Exposure>>;
