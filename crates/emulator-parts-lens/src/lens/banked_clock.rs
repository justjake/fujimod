//! Register-defined banked motor clocks. The protocol encodes nanoseconds in
//! 250 ns * 2^(10-selector) units; quantities/ramp lengths use
//! 32 electrical pulses per physical encoder step. Ramp shape is a declared
//! symmetric staircase approximation, not calibrated motor acceleration.
use super::mechanics::{Ramp, StepClock};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BankedClock {
    Fixed,
    EncoderStages { ticks_per_quantum: u64 },
    MicrostepStages { ticks_per_quantum: u64 },
}
pub(super) enum Profile {
    Encoder(Ramp),
    Microstep(StepClock),
}
impl BankedClock {
    pub(super) fn profile(self, registers: &[u16]) -> Result<Option<Profile>, String> {
        let ticks_per_quantum = match self {
            Self::Fixed => return Ok(None),
            Self::EncoderStages { ticks_per_quantum }
            | Self::MicrostepStages { ticks_per_quantum } => ticks_per_quantum,
        };
        let selector = (registers[4] >> 6) & 15;
        if selector > 10 || ticks_per_quantum == 0 {
            return Err("banked lens requires an admitted clock divider".into());
        }
        let scale = ticks_per_quantum
            .checked_mul(32 << (10 - selector))
            .ok_or("banked lens clock overflow")?;
        let mut periods = [0; 5];
        for (period, index) in periods.iter_mut().zip([3, 6, 7, 8, 9]) {
            *period = scale
                .checked_mul(u64::from(registers[index]))
                .filter(|p| *p != 0)
                .ok_or("banked lens requires five positive programmed periods")?;
        }
        if matches!(self, Self::MicrostepStages { .. }) {
            let microstep_periods = periods.map(|p| p / 32);
            return Ok(Some(Profile::Microstep(StepClock::new(
                microstep_periods,
                u32::from(registers[5]),
            )?)));
        }
        let mode = (registers[0] >> 4) & 3;
        let stage = u32::from(registers[5]);
        if mode != 0 && (stage == 0 || !stage.is_multiple_of(32)) {
            return Err("banked lens ramp length requires integral encoder steps".into());
        }
        Ok(Some(Profile::Encoder(Ramp {
            periods,
            stage_steps: (stage / 32).max(1),
            accelerate: mode & 2 != 0,
            decelerate: mode & 1 != 0,
        })))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn driver_encoded_period_divider_and_ramp_are_register_inputs() {
        let mut r = [0; 16];
        r[0] = 0x30;
        r[4] = 10 << 6;
        r[5] = 480;
        for (i, v) in [3, 6, 7, 8, 9].into_iter().zip([101, 60, 49, 42, 36]) {
            r[i] = v;
        }
        let clock = BankedClock::EncoderStages {
            ticks_per_quantum: 3,
        };
        let Profile::Encoder(ramp) = clock.profile(&r).unwrap().unwrap() else {
            panic!("expected encoder stages")
        };
        assert_eq!(ramp.periods, [9696, 5760, 4704, 4032, 3456]);
        assert_eq!(ramp.stage_steps, 15);
        r[4] = 9 << 6;
        let Profile::Encoder(ramp) = clock.profile(&r).unwrap().unwrap() else {
            panic!("expected encoder stages")
        };
        assert_eq!(ramp.periods[0], 19392);
        r[4] = 11 << 6;
        assert!(clock.profile(&r).is_err());
        r[4] = 10 << 6;
        r[5] = 31;
        assert!(clock.profile(&r).is_err());
    }
}
