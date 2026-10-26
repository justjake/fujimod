//! Bounded write-only calibration-register endpoint, identity unproved.
//! Six 10-bit fields occupy registers2..7 (wire bits11..2); register B
//! retains an opaque 12-bit control value. MISO is an explicit pin input.
//! No actuator, calibration completion, or companion reply is synthesized.
use super::serial::{Endpoint, Frame};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Default, Serialize, Deserialize)]
struct State {
    channels: [Option<u16>; 6],
    control: Option<u16>,
    transfers: u64,
}
pub struct CalibrationRegisters {
    state: State,
    miso_high: bool,
}
impl CalibrationRegisters {
    pub fn new(miso_high: bool) -> Self {
        Self {
            state: State::default(),
            miso_high,
        }
    }
}
impl Endpoint for CalibrationRegisters {
    fn name(&self) -> &'static str {
        "serial-calibration-registers"
    }
    fn validate(&self, frame: Frame) -> Result<(), String> {
        if frame.bits != 16 || frame.mode != 0 {
            return Err("calibration endpoint requires a 16-bit mode0 frame".into());
        }
        match frame.word >> 12 {
            2..=7 if frame.word & 3 == 0 => Ok(()),
            0xb => Ok(()),
            _ => Err(format!(
                "unsupported calibration register frame {:#06x}",
                frame.word
            )),
        }
    }
    fn reset_transaction(&mut self) {}
    fn transfer(&mut self, frame: Frame, _: u64) -> u16 {
        match frame.word >> 12 {
            address @ 2..=7 => {
                self.state.channels[address as usize - 2] = Some((frame.word >> 2) & 0x3ff)
            }
            0xb => self.state.control = Some(frame.word & 0xfff),
            _ => unreachable!("controller validates before accepting a frame"),
        }
        self.state.transfers += 1;
        if self.miso_high { u16::MAX } else { 0 }
    }
    fn save(&self) -> Value {
        json!({"miso_high":self.miso_high, "registers":self.state})
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        if state["miso_high"].as_bool() != Some(self.miso_high) {
            return Err("calibration MISO input differs from checkpoint".into());
        }
        let decoded: State =
            serde_json::from_value(state["registers"].clone()).map_err(|e| e.to_string())?;
        if decoded.channels.iter().flatten().any(|n| *n > 0x3ff)
            || decoded.control.is_some_and(|n| n > 0xfff)
        {
            return Err("calibration register checkpoint contains reserved bits".into());
        }
        self.state = decoded;
        Ok(())
    }
}
