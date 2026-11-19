//! Unidentified BIS serial configuration part: bounded byte-register contract.
//! Copied-image ecf0..edfa writes 01/02/0b/1c/05, emits single-byte command09,
//! and reads low revision nibbles at21/22. Physical identity and analog effects
//! are unproved. Revision and idle MISO are explicit external scenario inputs.
//!
//! Reads are `0x80 | address` followed by data frames that auto-increment the
//! address. Once the IBIS servo runs (X-T5 4.31 M-core task at `0x1e0fc`, state
//! machine at `0x1e23c`), it polls this part with burst reads `8a` (6 bytes),
//! `a8` (2), `c8` (2) and `a0` (6) from the command table at M-core RAM
//! `0x326fc`. Every register other than the revision nibbles reads zero: a body
//! at rest with zero sensor output, which is a scenario input, not a measurement.
use fujimod_emulator_devices::checkpoint::Checkpoint;
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
            Phase::Command if matches!(frame.word, 1 | 2 | 5 | 9 | 0xb | 0x1c | 0x80..=0xff) => {
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
                    0x80..=0xff => Phase::Read(command & 127),
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
                self.state.phase = Phase::Read((address + 1) & 127);
                match address {
                    0x21 | 0x22 => self.revision[(address - 0x21) as usize] as u16,
                    _ => 0,
                }
            }
            Phase::Finished => unreachable!("wire validates before accepting a frame"),
        };
        self.state.transfers += 1;
        response
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), &self.state)
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let state: State = saved.decode(self.name())?;
        if matches!(state.phase, Phase::Write(a) if Self::slot(a).is_none())
            || matches!(state.phase, Phase::Read(a) if a > 127) {
            return Err("invalid BIS configuration checkpoint".into());
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        self.state = saved.decode(self.name())?;
        Ok(())
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
            Phase::Read(a) if a > 127 => {
                return Err("invalid BIS read checkpoint".into());
            }
            _ => {}
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(word: u16) -> Frame {
        Frame { bits: 8, mode: 3, word }
    }
    fn exchange(part: &mut Registers, words: &[u16]) -> Vec<u16> {
        part.reset_transaction();
        words
            .iter()
            .map(|&w| {
                part.validate(frame(w)).unwrap();
                part.transfer(frame(w), 0)
            })
            .collect()
    }
    #[test]
    fn revision_reads_and_servo_burst_reads() {
        let mut part = Registers::new([3, 5], false).unwrap();
        assert_eq!(exchange(&mut part, &[0xa1, 0]), vec![0, 3]);
        assert_eq!(exchange(&mut part, &[0xa2, 0]), vec![0, 5]);
        // Burst read across the revision nibbles auto-increments.
        assert_eq!(exchange(&mut part, &[0xa0, 0, 0, 0]), vec![0, 0, 3, 5]);
        // The servo's polling commands read zero data.
        assert_eq!(exchange(&mut part, &[0x8a, 0, 0, 0, 0, 0, 0]), vec![0; 7]);
        assert_eq!(exchange(&mut part, &[0xa8, 0, 0]), vec![0; 3]);
        assert_eq!(exchange(&mut part, &[0xc8, 0, 0]), vec![0; 3]);
        // Writes stay single-register; unknown write commands refuse.
        assert_eq!(exchange(&mut part, &[0x1c, 0x4b]), vec![0, 0]);
        assert!(part.validate(frame(0)).is_err());
        part.reset_transaction();
        assert!(part.validate(frame(0x30)).is_err());
        let saved = part.checkpoint().unwrap();
        let mut fresh = Registers::new([3, 5], false).unwrap();
        fresh.restore_checkpoint(&saved).unwrap();
        assert_eq!(fresh.checkpoint().unwrap(), saved);
    }
}
