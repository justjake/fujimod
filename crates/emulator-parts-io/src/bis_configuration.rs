//! Unidentified BIS serial configuration part: bounded byte-register contract.
//! Copied-image ecf0..edfa writes 01/02/0b/1c/05, emits single-byte command09,
//! and reads low revision nibbles at21/22. Physical identity and analog effects
//! are unproved. Revision and idle MISO are explicit external scenario inputs.
use super::serial::{Endpoint, Frame};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
enum Phase {
    #[default]
    Command,
    Write(u8),
    Read(u8),
    Finished,
}
#[derive(Default, Serialize, Deserialize)]
struct State {
    phase: Phase,
    registers: [Option<u8>; 5],
    command: Option<u8>,
    transfers: u64,
}
pub struct Registers {
    state: State,
    revision: [u8; 2],
    idle_high: bool,
}
impl Registers {
    pub fn new(revision: [u8; 2], idle_high: bool) -> Result<Self, String> {
        if revision.iter().any(|&v| v > 15) {
            return Err("BIS revision inputs must be nibbles".into());
        }
        Ok(Self {
            state: State::default(),
            revision,
            idle_high,
        })
    }
    fn slot(address: u8) -> Option<usize> {
        [1, 2, 5, 0xb, 0x1c].iter().position(|&v| v == address)
    }
}
impl Endpoint for Registers {
    fn name(&self) -> &'static str {
        "bis-configuration-registers"
    }
    fn validate(&self, frame: Frame) -> Result<(), String> {
        if frame.bits != 8 || frame.mode != 3 || frame.word > 255 {
            return Err("BIS configuration requires 8-bit mode3".into());
        }
        match self.state.phase {
            Phase::Command if matches!(frame.word, 1 | 2 | 5 | 9 | 0xb | 0x1c | 0xa1 | 0xa2) => {
                Ok(())
            }
            Phase::Write(_) | Phase::Read(_) => Ok(()),
            _ => Err("BIS configuration register/transaction is unmodeled".into()),
        }
    }
    fn reset_transaction(&mut self) {
        self.state.phase = Phase::Command;
    }
    fn transfer(&mut self, frame: Frame, _: u64) -> u16 {
        let idle = if self.idle_high { 255 } else { 0 };
        let response = match self.state.phase {
            Phase::Command => {
                let command = frame.word as u8;
                self.state.command = Some(command);
                self.state.phase = match command {
                    9 => Phase::Finished,
                    0xa1 | 0xa2 => Phase::Read(command & 127),
                    _ => Phase::Write(command),
                };
                idle
            }
            Phase::Write(address) => {
                self.state.registers[Self::slot(address).unwrap()] = Some(frame.word as u8);
                self.state.phase = Phase::Finished;
                idle
            }
            Phase::Read(address) => {
                self.state.phase = Phase::Finished;
                self.revision[(address - 0x21) as usize] as u16
            }
            Phase::Finished => unreachable!("wire validates before accepting a frame"),
        };
        self.state.transfers += 1;
        response
    }
    fn save(&self) -> Value {
        json!({"revision":self.revision,"idle_high":self.idle_high,"state":self.state})
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        if v["revision"] != json!(self.revision) || v["idle_high"] != json!(self.idle_high) {
            return Err("BIS configuration inputs differ".into());
        }
        let state: State = serde_json::from_value(v["state"].clone()).map_err(|e| e.to_string())?;
        match state.phase {
            Phase::Write(a) if Self::slot(a).is_none() => {
                return Err("invalid BIS register checkpoint".into());
            }
            Phase::Read(a) if !matches!(a, 0x21 | 0x22) => {
                return Err("invalid BIS revision checkpoint".into());
            }
            _ => {}
        }
        self.state = state;
        Ok(())
    }
}
