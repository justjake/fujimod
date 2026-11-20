//! Arm generic physical/virtual timer compare channels, on guest ticks only.
//! Arm 102379, "The processor timers". Counter/control observations are pure.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clock {
    pub counter_hz: u64,
    pub ticks_per_second: u64,
}
impl Clock {
    pub fn validate(self) -> Result<Self, String> {
        if self.counter_hz == 0 || self.counter_hz > u32::MAX as u64 || self.ticks_per_second == 0 {
            return Err("invalid architectural counter clock".into());
        }
        Ok(self)
    }
    pub fn count(self, now: u64) -> Result<u64, String> {
        self.validate()?;
        // The common product fits 64 bits; the wide form is the same value.
        if let Some(product) = now.checked_mul(self.counter_hz) {
            return Ok(product / self.ticks_per_second);
        }
        // floor((a * t + b) * h / t) = a * h + floor(b * h / t) for b < t.
        let (whole, rest) = (now / self.ticks_per_second, now % self.ticks_per_second);
        if let Some(count) = rest
            .checked_mul(self.counter_hz)
            .and_then(|part| whole.checked_mul(self.counter_hz)?.checked_add(part / self.ticks_per_second))
        {
            return Ok(count);
        }
        u64::try_from(now as u128 * self.counter_hz as u128 / self.ticks_per_second as u128)
            .map_err(|_| "architectural counter overflow".into())
    }
    fn at_count(self, count: u64) -> Result<u64, String> {
        self.validate()?;
        if let Some(product) = count.checked_mul(self.ticks_per_second) {
            return Ok(product.div_ceil(self.counter_hz));
        }
        // ceil((a * h + b) * t / h) = a * t + ceil(b * t / h) for b < h.
        let (whole, rest) = (count / self.counter_hz, count % self.counter_hz);
        if let Some(tick) = rest.checked_mul(self.ticks_per_second).and_then(|part| {
            whole.checked_mul(self.ticks_per_second)?.checked_add(part.div_ceil(self.counter_hz))
        }) {
            return Ok(tick);
        }
        u64::try_from(
            (count as u128 * self.ticks_per_second as u128).div_ceil(self.counter_hz as u128),
        )
        .map_err(|_| "architectural counter deadline overflow".into())
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Register {
    Compare,
    Value,
    Control,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Timer {
    compare: u64,
    control: u8,
}
impl Timer {
    fn condition(&self, count: u64) -> bool {
        (count.wrapping_sub(self.compare) as i64) >= 0
    }
    pub fn asserted(&self, count: u64) -> bool {
        self.control & 3 == 1 && self.condition(count)
    }
    pub fn access(
        &mut self,
        register: Register,
        count: u64,
        value: Option<u64>,
    ) -> Result<u64, String> {
        if let Some(value) = value {
            match register {
                Register::Compare => self.compare = value,
                // A TVAL write sign-extends its low 32 bits before adding to count.
                Register::Value => {
                    self.compare = count.wrapping_add(value as u32 as i32 as i64 as u64)
                }
                Register::Control => self.control = (value & 3) as u8,
            }
        }
        Ok(match register {
            Register::Compare => self.compare,
            Register::Value => self.compare.wrapping_sub(count) as u32 as u64,
            Register::Control => {
                self.control as u64
                    | if self.control & 1 != 0 && self.condition(count) {
                        4
                    } else {
                        0
                    }
            }
        })
    }
    /// A masked timer's status is calculated on access. It has no output
    /// transition to schedule; unmasking evaluates the live counter immediately.
    pub fn next_deadline(&self, clock: Clock, now: u64) -> Result<Option<u64>, String> {
        let count = clock.count(now)?;
        if self.control & 3 != 1 || self.condition(count) {
            return Ok(None);
        }
        let distance = self.compare.wrapping_sub(count);
        let target = count
            .checked_add(distance)
            .ok_or("architectural counter wrap is unsupported")?;
        Ok(Some(clock.at_count(target)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The 64-bit forms of the conversions equal the wide forms where the
    /// plain product overflows.
    #[test]
    fn split_conversions_equal_the_wide_forms() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for (hz, tps) in [
            (24_000_000u64, 1_000_000_000u64),
            (1_000_000_000, 24_000_000),
            (u32::MAX as u64, 1_000_000_007),
            (3, 10),
            (62_500_000, u64::MAX / 3),
        ] {
            let clock = Clock { counter_hz: hz, ticks_per_second: tps };
            for round in 0..4000 {
                let value = next() >> (round % 40);
                let wide = u64::try_from(value as u128 * hz as u128 / tps as u128).ok();
                assert_eq!(clock.count(value).ok(), wide, "count {value} {hz} {tps}");
                let wide = u64::try_from((value as u128 * tps as u128).div_ceil(hz as u128)).ok();
                assert_eq!(clock.at_count(value).ok(), wide, "at_count {value} {hz} {tps}");
            }
        }
    }
    #[test]
    fn compare_channels_share_fractional_clock_without_deadline_drift() {
        let clock = Clock {
            counter_hz: 3,
            ticks_per_second: 10,
        }
        .validate()
        .unwrap();
        let mut timer = Timer::default();
        timer.access(Register::Compare, 0, Some(1)).unwrap();
        timer.access(Register::Control, 0, Some(1)).unwrap();
        assert_eq!(timer.next_deadline(clock, 0).unwrap(), Some(4));
        assert!(!timer.asserted(clock.count(3).unwrap()));
        assert!(timer.asserted(clock.count(4).unwrap()));
        timer.access(Register::Compare, 1, Some(2)).unwrap();
        assert_eq!(timer.next_deadline(clock, 4).unwrap(), Some(7));
        timer.access(Register::Compare, 2, Some(3)).unwrap();
        assert_eq!(timer.next_deadline(clock, 7).unwrap(), Some(10));
        assert_eq!(timer.next_deadline(clock, 10).unwrap(), None);
    }
    #[test]
    fn masked_and_disabled_outputs_do_not_poll_and_tval_is_signed() {
        let clock = Clock {
            counter_hz: 100,
            ticks_per_second: 100,
        }
        .validate()
        .unwrap();
        let mut timer = Timer::default();
        timer
            .access(Register::Value, 1000, Some(u32::MAX as u64))
            .unwrap();
        assert_eq!(timer.access(Register::Compare, 1000, None).unwrap(), 999);
        assert_eq!(
            timer.access(Register::Value, 1000, None).unwrap(),
            u32::MAX as u64
        );
        timer.access(Register::Control, 1000, Some(3)).unwrap();
        assert!(!timer.asserted(1000));
        assert_eq!(timer.access(Register::Control, 1000, None).unwrap(), 7);
        assert_eq!(timer.next_deadline(clock, 1000).unwrap(), None);
        timer.access(Register::Control, 1000, Some(1)).unwrap();
        assert!(timer.asserted(1000));
        timer.access(Register::Control, 1000, Some(0)).unwrap();
        assert!(!timer.asserted(1000));
        assert_eq!(timer.access(Register::Control, 1000, None).unwrap(), 0);
        timer.access(Register::Value, 1000, Some(20)).unwrap();
        timer.access(Register::Control, 1000, Some(1)).unwrap();
        assert_eq!(timer.next_deadline(clock, 1000).unwrap(), Some(1020));
        let state = serde_json::to_value(&timer).unwrap();
        assert_eq!(serde_json::to_value(&timer).unwrap(), state);
        let restored: Timer = serde_json::from_value(state).unwrap();
        assert_eq!(restored.next_deadline(clock, 1019).unwrap(), Some(1020));
        assert!(restored.asserted(1020));
    }
    #[test]
    fn narrow_counter_arithmetic_equals_the_wide_form() {
        let values = [
            0u64,
            1,
            3,
            999,
            1 << 31,
            62_000_000_000,
            (1 << 40) + 7,
            u64::MAX / 3,
            u64::MAX,
        ];
        for (counter_hz, ticks_per_second) in [
            (1u64, 1u64),
            (3, 10),
            (24_000_000, 1_000_000_000),
            (u32::MAX as u64, 7),
            (19_200_000, u64::MAX),
        ] {
            let clock = Clock {
                counter_hz,
                ticks_per_second,
            };
            for &value in &values {
                let count = u64::try_from(
                    value as u128 * counter_hz as u128 / ticks_per_second as u128,
                )
                .ok();
                assert_eq!(clock.count(value).ok(), count);
                let at = u64::try_from(
                    (value as u128 * ticks_per_second as u128).div_ceil(counter_hz as u128),
                )
                .ok();
                assert_eq!(clock.at_count(value).ok(), at);
            }
        }
    }
}
