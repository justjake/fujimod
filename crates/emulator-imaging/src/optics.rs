//! The declared thin-lens optics a projection follows.
use serde::{Deserialize, Serialize};
/// Relative field illumination is explicit optical fixture data. Cosine-fourth
/// is the ideal thin-lens approximation; a radial polynomial is a measured
/// lens profile, 1 / (1 + r2 rho^2 + r4 rho^4) with rho the image-plane radius
/// over the sensor half-diagonal.
#[derive(Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Illumination {
    Uniform,
    CosineFourth,
    RadialPolynomial { r2: f64, r4: f64 },
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub focus: String,
    pub focal_length_mm: f64,
    pub subject_distance_mm: f64,
    pub sensor_width_mm: f64,
    pub relative_illumination: Illumination,
}
