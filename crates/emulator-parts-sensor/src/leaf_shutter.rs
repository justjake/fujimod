//! A driven leaf shutter with declared aperture-dependent closing latency.
//! The light interval ends when the commanded blade motion reaches optical
//! closure. Latency is body calibration; this model never receives a requested
//! photographic shutter speed or software exposure metadata.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub closing_delay_seconds: Vec<(f64, f64)>,
}
pub struct LeafShutter {
    curve: Vec<(f64, f64)>,
}
impl LeafShutter {
    pub fn new(options: Options) -> Result<Self, String> {
        let curve = options.closing_delay_seconds;
        if curve.len() < 2
            || curve.windows(2).any(|p| p[0].0 >= p[1].0)
            || curve
                .iter()
                .any(|&(f, t)| !f.is_finite() || f <= 0. || !t.is_finite() || t <= 0.)
        {
            return Err("leaf shutter needs an ordered f-number/closing-time calibration".into());
        }
        Ok(Self { curve })
    }
    pub fn closing_delay(&self, aperture_squared: f64) -> Result<f64, String> {
        if !aperture_squared.is_finite() || aperture_squared <= 0. {
            return Err("leaf shutter needs a physical aperture".into());
        }
        let f = libm::sqrt(aperture_squared);
        let first = self.curve[0];
        let last = *self.curve.last().unwrap();
        Ok(if f <= first.0 {
            first.1
        } else if f >= last.0 {
            last.1
        } else {
            let pair = self
                .curve
                .windows(2)
                .find(|p| f >= p[0].0 && f <= p[1].0)
                .unwrap();
            let t = libm::log(f / pair[0].0) / libm::log(pair[1].0 / pair[0].0);
            pair[0].1 * (1. - t) + pair[1].1 * t
        })
    }
    pub fn integration(
        &self,
        drive_seconds: f64,
        reset_seconds: f64,
        aperture_squared: f64,
    ) -> Result<f64, String> {
        let interval = drive_seconds + self.closing_delay(aperture_squared)? - reset_seconds;
        if !interval.is_finite() || interval <= 0. {
            return Err("global reset occurs after declared optical shutter closure".into());
        }
        Ok(interval)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optical_close_limits_global_reset_integration_without_a_software_tv() {
        let s = LeafShutter::new(Options {
            closing_delay_seconds: vec![(2., 0.003), (4., 0.004)],
        })
        .unwrap();
        assert!((s.integration(0.0002, 0.0022, 4.).unwrap() - 0.001).abs() < 1e-12);
        assert!((s.integration(0.0002, 0.0027, 4.).unwrap() - 0.0005).abs() < 1e-12);
        assert!((s.integration(0.0002, 0.0022, 16.).unwrap() - 0.002).abs() < 1e-12);
        assert!(s.integration(0., 0.005, 4.).is_err());
    }
}
