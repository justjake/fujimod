//! Three-byte motor period fields: selector in register3[13:10], first
//! period in [9:0], remaining periods in registers5..8, stage pulses in4.
//! Periods use ceil(count * divider_picoseconds * 32 * ticks_per_second /
//! 1e12). The relative encoder law retains 32 electrical pulses per step;
//! the staircase ramp is uncalibrated.
use super::mechanics::Ramp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ThreeByteClock {
    Fixed,
    Divided {
        ticks_per_second: u64,
        period_picoseconds: [u64; 11],
    },
}
impl ThreeByteClock {
    pub(super) fn profile(self, r: &[u16]) -> Result<Option<Ramp>, String> {
        let Self::Divided {
            ticks_per_second,
            period_picoseconds,
        } = self
        else {
            return Ok(None);
        };
        let selector = usize::from(r[3] >> 10 & 15);
        if selector >= period_picoseconds.len()
            || ticks_per_second == 0
            || period_picoseconds.contains(&0)
        {
            return Err("three-byte lens requires an admitted positive divider clock".into());
        }
        let mut periods = [0; 5];
        for (p, i) in periods.iter_mut().zip([3, 5, 6, 7, 8]) {
            let count = u128::from(r[i] & 1023);
            let ticks = count
                .checked_mul(u128::from(period_picoseconds[selector]))
                .and_then(|n| n.checked_mul(32))
                .and_then(|n| n.checked_mul(u128::from(ticks_per_second)))
                .ok_or("three-byte lens clock overflow")?;
            *p = u64::try_from(ticks.div_ceil(1_000_000_000_000))
                .ok()
                .filter(|t| *t > 0)
                .ok_or("three-byte lens requires finite positive periods")?;
        }
        let mode = r[0] >> 4 & 3;
        let stage = u32::from(r[4] & 1023);
        if mode != 0 && (stage == 0 || !stage.is_multiple_of(32)) {
            return Err("three-byte lens ramp requires integral encoder steps".into());
        }
        Ok(Some(Ramp {
            periods,
            stage_steps: (stage / 32).max(1),
            accelerate: mode & 2 != 0,
            decelerate: mode & 1 != 0,
        }))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encoded_three_byte_periods_have_their_own_divider_and_stage_fields() {
        let mut r = [0; 16];
        r[0] = 0x30;
        r[3] = 10 << 10 | 58;
        r[4] = 128;
        for (i, v) in [5, 6, 7, 8].into_iter().zip([45, 39, 34, 31]) {
            r[i] = v;
        }
        let c = ThreeByteClock::Divided {
            ticks_per_second: 12_000_000,
            period_picoseconds: [
                213333333, 106666666, 53333333, 26666666, 13333333, 6666666, 3333333, 1666666,
                833333, 416666, 208333,
            ],
        };
        let ramp = c.profile(&r).unwrap().unwrap();
        assert_eq!(ramp.periods, [4640, 3600, 3120, 2720, 2480]);
        assert_eq!(ramp.stage_steps, 4);
        r[3] = 11 << 10 | 58;
        assert!(c.profile(&r).is_err());
        r[3] = 10 << 10 | 58;
        r[4] = 31;
        assert!(c.profile(&r).is_err());
    }
}
