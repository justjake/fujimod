//! Explicit noise at a modeled sensor sampling site, before ADC clipping.
use serde::{Deserialize, Serialize};

/// Unmeasured caller-supplied charge/noise parameters. `realization` selects
/// an immutable exposure's random field; changing it is an external input.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensorNoise {
    pub seed: u64,
    pub realization: u64,
    /// Collected charge represented by ADC full scale at unit analog gain.
    pub full_well_electrons: f64,
    pub read_noise_electrons: f64,
}
impl SensorNoise {
    pub fn validate(self) -> Result<Self, String> {
        if !self.full_well_electrons.is_finite()
            || self.full_well_electrons <= 0.
            || !self.read_noise_electrons.is_finite()
            || self.read_noise_electrons < 0.
        {
            return Err(
                "sensor noise needs positive finite full well and nonnegative finite read noise"
                    .into(),
            );
        }
        Ok(self)
    }
    /// `signal` is the unclipped, analog-amplified signal in 14-bit ADC units.
    /// Shot charge uses a Gaussian Poisson approximation (variance = mean
    /// electrons); independent Gaussian read noise is added in electrons.
    /// Each normal uses twelve centered uniforms, a finite approximation with
    /// unit variance. Both terms are amplified before the ADC rounds/clips.
    pub fn quantize(self, signal: f64, analog_gain: f64, x: usize, y: usize, color: usize) -> u16 {
        let electrons_to_adc = analog_gain * 16383. / self.full_well_electrons;
        let electrons = signal.max(0.) / electrons_to_adc;
        let key = mix(self.seed
            ^ mix(self.realization)
            ^ mix(x as u64)
            ^ mix((y as u64).wrapping_add(0x7f4a7c15))
            ^ mix((color as u64).wrapping_add(0x94d049bb)));
        let shot = libm::sqrt(electrons) * normal(key);
        let read = self.read_noise_electrons * normal(key ^ 0xd1b54a32d192ed03);
        (signal + (shot + read) * electrons_to_adc)
            .round_ties_even()
            .clamp(0., 16383.) as u16
    }
}
fn mix(mut v: u64) -> u64 {
    v = (v ^ (v >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    v = (v ^ (v >> 27)).wrapping_mul(0x94d049bb133111eb);
    v ^ (v >> 31)
}
fn normal(key: u64) -> f64 {
    let mut sum = -6.;
    for i in 0u64..12 {
        sum += ((mix(key.wrapping_add(i.wrapping_mul(0x9e3779b97f4a7c15))) >> 11) as f64 + 0.5)
            / 9007199254740992.;
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile() -> SensorNoise {
        SensorNoise {
            seed: 17,
            realization: 3,
            full_well_electrons: 20000.,
            read_noise_electrons: 3.,
        }
    }
    #[test]
    fn same_exposure_is_immutable_and_new_realizations_change_samples() {
        let p = profile();
        let samples = |p: SensorNoise| {
            (0..128)
                .map(|x| p.quantize(4000., 2., x, 0, 1))
                .collect::<Vec<_>>()
        };
        assert_eq!(samples(p), samples(p));
        assert_ne!(
            samples(p),
            samples(SensorNoise {
                realization: 4,
                ..p
            })
        );
        assert_ne!(samples(p), samples(SensorNoise { seed: 18, ..p }));
    }
    #[test]
    fn variance_follows_collected_charge_and_analog_gain() {
        let p = profile();
        for gain in [1., 4.] {
            for signal in [1000., 6000.] {
                let values = (0..10000)
                    .map(|x| f64::from(p.quantize(signal, gain, x, 7, 0)))
                    .collect::<Vec<_>>();
                let mean = values.iter().sum::<f64>() / values.len() as f64;
                let variance = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>()
                    / values.len() as f64;
                let scale = gain * 16383. / p.full_well_electrons;
                let expected = signal * scale + (p.read_noise_electrons * scale).powi(2);
                assert!((mean - signal).abs() < 2., "{mean} vs {signal}");
                assert!(
                    (variance / expected - 1.).abs() < 0.07,
                    "{variance} vs {expected}"
                );
            }
        }
    }
    #[test]
    fn noise_precedes_adc_clipping_and_parameters_are_checked() {
        let p = profile();
        assert!((0..1000).any(|x| p.quantize(16383., 1., x, 0, 0) < 16383));
        assert!((0..1000).all(|x| p.quantize(100000., 1., x, 0, 0) == 16383));
        assert!((0..1000).any(|x| p.quantize(0., 1., x, 0, 0) > 0));
        for value in [0., -1., f64::NAN, f64::INFINITY] {
            assert!(
                SensorNoise {
                    full_well_electrons: value,
                    ..p
                }
                .validate()
                .is_err()
            );
        }
        for value in [-1., f64::NAN, f64::INFINITY] {
            assert!(
                SensorNoise {
                    read_noise_electrons: value,
                    ..p
                }
                .validate()
                .is_err()
            );
        }
    }
}
