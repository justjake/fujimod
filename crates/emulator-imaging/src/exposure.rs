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
/// ADC response for a declared sRGB input without an optical exposure model.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedRgb {
    pub full_scale: u16,
    pub reference_white: [u16; 3],
    pub color_sample_weights: [u8; 3],
}

impl NormalizedRgb {
    pub fn validate(self) -> Result<Self, String> {
        if self.full_scale == 0
            || self.reference_white.contains(&0)
            || self.color_sample_weights.contains(&0)
        {
            return Err("invalid normalized RGB sensor response".into());
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
impl Exposure {
    /// The exposure that renders a scene at its nominal level ("E_ref"): 1/125 s at unity
    /// analog gain, the aperture not modelled. Scene models scale their signal by
    /// [`Exposure::relative`].
    pub const REFERENCE: Self = Self { seconds: 1. / 125., analog_gain: 1. };
    /// This exposure over [`Exposure::REFERENCE`] (seconds x gain), or 1 while either factor is
    /// unset (zero, negative or not finite), so a sensor that has not been programmed yet
    /// renders at the reference level.
    pub fn relative(self) -> f64 {
        let valid = |v: f64| v.is_finite() && v > 0.;
        if !valid(self.seconds) || !valid(self.analog_gain) {
            return 1.;
        }
        (self.seconds * self.analog_gain) / (Self::REFERENCE.seconds * Self::REFERENCE.analog_gain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_exposure_is_one_at_the_reference_and_while_unset() {
        assert_eq!(Exposure::default().relative(), 1.);
        assert_eq!(Exposure::REFERENCE.relative(), 1.);
        let doubled = Exposure { seconds: 1. / 125., analog_gain: 2. };
        assert!((doubled.relative() - 2.).abs() < 1e-12);
        let half = Exposure { seconds: 1. / 250., analog_gain: 1. };
        assert!((half.relative() - 0.5).abs() < 1e-12);
    }
}
