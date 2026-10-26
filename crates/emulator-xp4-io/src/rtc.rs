//! Shared power-peer seconds counter and timebase, and the staged clock-load
//! law. The three-byte peer keeps its direct law with its protocol.
use serde::{Deserialize, Serialize};

// Field names and ordering preserve the three-byte peer's checkpoint encoding.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counter {
    pub(crate) seconds: u32,
    pub(crate) anchor: u64,
    pub(crate) running: bool,
    pub(crate) ticks_per_second: u64,
}
impl Counter {
    pub fn new(seconds: u32, running: bool, ticks_per_second: u64) -> Result<Self, String> {
        if !(1..=1_000_000_000).contains(&ticks_per_second) {
            return Err("Invalid RTC timebase".into());
        }
        Ok(Self {
            seconds,
            anchor: 0,
            running,
            ticks_per_second,
        })
    }
    pub fn counter(&self, tick: u64) -> Result<u32, String> {
        let elapsed = tick
            .checked_sub(self.anchor)
            .ok_or("Power RTC clock moved backwards")?;
        Ok(self.seconds.wrapping_add(if self.running {
            (elapsed / self.ticks_per_second) as u32
        } else {
            0
        }))
    }
    pub(crate) fn set_running(&mut self, running: bool, tick: u64) -> Result<(), String> {
        if self.running != running {
            self.seconds = self.counter(tick)?;
            self.anchor = tick;
            self.running = running;
        }
        Ok(())
    }
    pub(crate) fn load(&mut self, seconds: u32, tick: u64) {
        self.seconds = seconds;
        self.anchor = tick;
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagedConfig {
    pub control: u8,
    pub read_low: u8,
    pub read_high: u8,
    pub stage_low: u8,
    pub stage_high: u8,
    pub commit_value: u16,
    pub valid_mask: u16,
    pub retained_valid_mask: u16,
    pub strobe: StrobeWindow,
    pub initial_control: u16,
    pub initial_seconds: u32,
    pub ticks_per_second: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrobeWindow {
    pub mask: u16,
    pub set_through_ticks: u64,
    pub clear_from_ticks: u64,
}
impl StagedConfig {
    pub fn validate(&self) -> Result<(), String> {
        let roles = [
            self.control,
            self.read_low,
            self.read_high,
            self.stage_low,
            self.stage_high,
        ];
        let unique: std::collections::BTreeSet<_> = roles.into_iter().collect();
        if unique.len() != roles.len()
            || roles.iter().any(|r| *r > 127)
            || self.valid_mask.count_ones() != 1
            || self.retained_valid_mask.count_ones() != 1
            || self.strobe.mask.count_ones() != 1
            || self.valid_mask & self.retained_valid_mask != 0
            || (self.valid_mask | self.retained_valid_mask) & self.strobe.mask != 0
            || self.commit_value != self.valid_mask | self.strobe.mask
            || self.strobe.set_through_ticks >= self.strobe.clear_from_ticks
            || self.initial_control != 0
            || self.initial_seconds != 0
        {
            return Err("Unsupported staged RTC register law or initial state".into());
        }
        Counter::new(0, false, self.ticks_per_second).map(|_| ())
    }
    pub fn owns(&self, register: u8) -> bool {
        [
            self.control,
            self.read_low,
            self.read_high,
            self.stage_low,
            self.stage_high,
        ]
        .contains(&register)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagedClock {
    pub config: StagedConfig,
    pub counter: Counter,
    staged: u32,
    halves: u8,
    committed: bool,
    control_written: u16,
}
impl StagedClock {
    pub fn new(config: StagedConfig) -> Result<Self, String> {
        config.validate()?;
        let control_written = config.initial_control;
        Ok(Self {
            counter: Counter::new(config.initial_seconds, false, config.ticks_per_second)?,
            config,
            staged: 0,
            halves: 0,
            committed: false,
            control_written,
        })
    }
    pub fn read(&self, register: u8, tick: u64) -> Result<u16, String> {
        let c = &self.config;
        if register == c.control {
            let elapsed = tick
                .checked_sub(self.counter.anchor)
                .ok_or("Power RTC clock moved backwards")?;
            if self.control_written & c.strobe.mask == 0 {
                return Ok(self.control_written);
            }
            if elapsed <= c.strobe.set_through_ticks {
                Ok(self.control_written)
            } else if elapsed >= c.strobe.clear_from_ticks {
                Ok(self.control_written & !c.strobe.mask)
            } else {
                Err(format!(
                    "Staged RTC control strobe is unresolved at elapsed tick {elapsed}"
                ))
            }
        } else if register == c.read_low {
            Ok(self.counter.counter(tick)? as u16)
        } else if register == c.read_high {
            Ok((self.counter.counter(tick)? >> 16) as u16)
        } else {
            Err("Staged RTC staging-register readback is not modeled".into())
        }
    }
    pub fn write(&mut self, register: u8, value: u16, tick: u64) -> Result<(), String> {
        let c = &self.config;
        if register == c.stage_low {
            self.staged = self.staged & 0xffff0000 | u32::from(value);
            self.halves |= 1;
        } else if register == c.stage_high {
            self.staged = self.staged & 0xffff | u32::from(value) << 16;
            self.halves |= 2;
        } else if register == c.control
            && value == c.commit_value | (self.control_written & c.retained_valid_mask)
        {
            if self.halves != 3 {
                return Err("Staged RTC commit requires both seconds halves".into());
            }
            self.counter.load(self.staged, tick);
            self.counter.set_running(true, tick)?;
            self.committed = true;
            self.control_written = value;
            self.halves = 0;
        } else {
            return Err(format!(
                "Unsupported staged RTC write register{register} value0x{value:04x}"
            ));
        }
        Ok(())
    }
    pub fn retained_control_value(&self) -> u16 {
        self.config.valid_mask | self.config.retained_valid_mask
    }
    pub fn commit_retained(&mut self, value: u16, tick: u64) -> Result<(), String> {
        let elapsed = tick
            .checked_sub(self.counter.anchor)
            .ok_or("Power RTC clock moved backwards")?;
        if !self.committed
            || elapsed < self.config.strobe.clear_from_ticks
            || value != self.retained_control_value()
        {
            return Err("Unsupported staged RTC retained control operation".into());
        }
        // The words are stored by the serial peer. This latches validity only;
        // it supplies no persistence, power event or clock reload.
        self.control_written = value;
        Ok(())
    }
    pub fn validate_restore(&self, current: &Self) -> Result<(), String> {
        if self.config != current.config
            || self.counter.ticks_per_second != self.config.ticks_per_second
            || self.halves > 3
            || self.counter.running != self.committed
            || (!self.committed && self.control_written != self.config.initial_control)
            || (self.committed
                && ![
                    self.retained_control_value(),
                    self.config.commit_value,
                    self.config.commit_value | self.config.retained_valid_mask,
                ]
                .contains(&self.control_written))
        {
            return Err("Invalid staged RTC checkpoint or configuration".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> StagedConfig {
        StagedConfig {
            control: 100,
            read_low: 101,
            read_high: 102,
            stage_low: 103,
            stage_high: 104,
            commit_value: 0x202,
            valid_mask: 2,
            retained_valid_mask: 1,
            strobe: StrobeWindow {
                mask: 0x200,
                set_through_ticks: 2,
                clear_from_ticks: 5,
            },
            initial_control: 0,
            initial_seconds: 0,
            ticks_per_second: 10,
        }
    }
    #[test]
    fn staged_load_uses_the_shared_counter_and_refuses_unknown_control_semantics() {
        let mut clock = StagedClock::new(config()).unwrap();
        assert_eq!(clock.read(100, 0).unwrap(), 0);
        clock.write(103, 0x5678, 3).unwrap();
        assert!(clock.write(100, 0x202, 4).is_err());
        assert_eq!(clock.counter.counter(5).unwrap(), 0);
        clock.write(104, 0x1234, 5).unwrap();
        clock.write(100, 0x202, 30).unwrap();
        assert_eq!(clock.read(101, 40).unwrap(), 0x5679);
        assert_eq!(clock.read(102, 40).unwrap(), 0x1234);
        assert_eq!(clock.read(100, 30).unwrap(), 0x202);
        assert_eq!(clock.read(100, 32).unwrap(), 0x202);
        assert!(clock.read(100, 33).unwrap_err().contains("unresolved"));
        assert!(clock.read(100, 34).is_err());
        assert_eq!(clock.read(100, 35).unwrap(), 2);
        assert!(clock.commit_retained(3, 34).is_err());
        clock.commit_retained(3, 40).unwrap();
        assert_eq!(clock.read(100, 40).unwrap(), 3);
        assert_eq!(clock.counter.anchor, 30);
        for value in [0, 1, 2, 4, 0x200, 0x206] {
            assert!(clock.write(100, value, 41).is_err());
        }
        assert!(clock.write(42, 0, 41).is_err());
        clock.write(103, 0xffff, 50).unwrap();
        clock.write(104, 0xffff, 51).unwrap();
        assert_eq!(clock.counter.counter(50).unwrap(), 0x1234567a);
        assert!(clock.write(100, 0x202, 57).is_err());
        clock.write(100, 0x203, 57).unwrap();
        assert_eq!(clock.read(100, 59).unwrap(), 0x203);
        assert_eq!(clock.read(100, 62).unwrap(), 3);
        assert_eq!(clock.counter.counter(67).unwrap(), 0);
    }
    #[test]
    fn staged_checkpoint_preserves_partial_load_and_timebase() {
        let mut clock = StagedClock::new(config()).unwrap();
        clock.write(103, 0x9876, 2).unwrap();
        let bytes = serde_json::to_vec(&clock).unwrap();
        let mut resumed: StagedClock = serde_json::from_slice(&bytes).unwrap();
        resumed.validate_restore(&clock).unwrap();
        resumed.write(104, 0x1234, 3).unwrap();
        resumed.write(100, 0x202, 9).unwrap();
        assert_eq!(resumed.counter.counter(29).unwrap(), 0x12349878);
        let before = serde_json::to_value(&resumed).unwrap();
        assert_eq!(resumed.counter.counter(29).unwrap(), 0x12349878);
        assert_eq!(serde_json::to_value(&resumed).unwrap(), before);
        let mut other = config();
        other.ticks_per_second = 11;
        assert!(
            resumed
                .validate_restore(&StagedClock::new(other).unwrap())
                .is_err()
        );
    }
    #[test]
    fn strobe_windows_are_observational_and_survive_checkpoints() {
        let mut clock = StagedClock::new(config()).unwrap();
        clock.write(103, 0x9876, 2).unwrap();
        clock.write(104, 0x1234, 3).unwrap();
        clock.write(100, 0x202, 10).unwrap();
        let saved = serde_json::to_value(&clock).unwrap();
        assert_eq!(clock.read(100, 12).unwrap(), 0x202);
        assert!(clock.read(100, 13).is_err());
        assert_eq!(clock.read(100, 15).unwrap(), 2);
        assert_eq!(serde_json::to_value(&clock).unwrap(), saved);
        let resumed: StagedClock = serde_json::from_value(saved.clone()).unwrap();
        resumed.validate_restore(&clock).unwrap();
        assert_eq!(resumed.read(100, 12).unwrap(), 0x202);
        assert!(resumed.read(100, 13).is_err());
        assert_eq!(resumed.read(100, 15).unwrap(), 2);
        assert!(resumed.read(100, 9).is_err());
        let mut wrong = saved;
        wrong["control_written"] = 4.into();
        let invalid: StagedClock = serde_json::from_value(wrong).unwrap();
        assert!(invalid.validate_restore(&clock).is_err());
        let mut invalid = config();
        invalid.strobe.clear_from_ticks = invalid.strobe.set_through_ticks;
        assert!(invalid.validate().is_err());
    }
}
