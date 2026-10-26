//! The declared radiometric rendering of a sensor scene: its frames, exposure,
//! aperture, parameter wires, calibration and optional optical projection.
use super::exposure::Calibration;
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub frames: String,
    pub exposure: String,
    pub aperture: String,
    pub parameters: String,
    pub calibration: Calibration,
    pub projection: Option<super::optics::Spec>,
}
