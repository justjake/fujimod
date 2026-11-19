//! The declared radiometric rendering of a sensor scene: its frames, exposure,
//! aperture, parameter wires, calibration and optional optical projection.
use super::exposure::{Calibration, NormalizedRgb};
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicalSpec {
    pub frames: String,
    pub exposure: String,
    pub aperture: String,
    pub parameters: String,
    pub calibration: Calibration,
    pub projection: Option<super::optics::Spec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedSpec {
    pub frames: String,
    pub parameters: String,
    pub normalized_rgb: NormalizedRgb,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum Spec {
    Physical(PhysicalSpec),
    Normalized(NormalizedSpec),
}
