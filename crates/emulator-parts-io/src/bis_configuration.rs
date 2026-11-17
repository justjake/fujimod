//! Unidentified BIS serial configuration part: bounded byte-register contract.
//! Copied-image ecf0..edfa writes 01/02/0b/1c/05, emits single-byte command09,
//! and reads low revision nibbles at21/22. Physical identity and analog effects
//! are unproved. Revision, conversion words, the 0a block and idle MISO are
//! explicit external scenario inputs.
//!
//! Reads are `0x80 | address` followed by data frames that auto-increment the
//! address. Once the IBIS servo runs (X-T5 4.31 M-core task at `0x1e0fc`, state
//! machine at `0x1e23c`), it polls this part with burst reads `8a` (6 bytes),
//! `a8` (2), `c8` (2) and `a0` (6) from the command table at M-core RAM
//! `0x326fc`. Commands a8 and c8 each return a two-byte conversion word, most
//! significant byte first (copied-image a388..a418 of a later body reads them as
//! a left-aligned 12-bit value); a third data byte refuses. Command 8a reads six
//! bytes from 0a..0f in one transaction (X-H2S companion sampler table 0x22ba0:
//! 8a, length 7); the bytes answer only when the scenario declares them (a body
//! at rest declares zeros). Every other register than the revision nibbles
//! reads zero: a body at rest with zero sensor output, which is a scenario
//! input, not a measurement.
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
    Sample { channel: u8, byte: u8 },
    Block(u8),
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
    samples: [u16; 2],
    idle_high: bool,
    block: Option<[u8; 6]>,
}
impl Registers {
    pub fn new(revision: [u8; 2], samples: [u16; 2], idle_high: bool) -> Result<Self, String> {
        Self::with_block(revision, samples, idle_high, None)
    }
    /// `block` answers the six-byte read of 0a..0f (command 8a), in wire order.
    pub fn with_block(
        revision: [u8; 2],
        samples: [u16; 2],
        idle_high: bool,
        block: Option<[u8; 6]>,
    ) -> Result<Self, String> {
        if revision.iter().any(|&v| v > 15) {
            return Err("BIS revision inputs must be nibbles".into());
        }
        Ok(Self {
            state: State::default(),
            revision,
            samples,
            idle_high,
            block,
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
            Phase::Command
                if matches!(frame.word, 1 | 2 | 5 | 9 | 0xb | 0x1c | 0x80..=0xff)
                    && (frame.word != 0x8a || self.block.is_some()) =>
            {
                Ok(())
            }
            Phase::Write(_) | Phase::Read(_) | Phase::Sample { .. } | Phase::Block(_) => Ok(()),
            Phase::Command => Err(format!(
                "BIS configuration command {:#04x} is unmodeled",
                frame.word
            )),
            Phase::Finished => Err(format!(
                "BIS configuration transaction continues past its last byte (word {:#04x})",
                frame.word
            )),
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
                    0xa8 => Phase::Sample { channel: 0, byte: 0 },
                    0xc8 => Phase::Sample { channel: 1, byte: 0 },
                    0x8a => Phase::Block(0),
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
            Phase::Sample { channel, byte } => {
                let word = self.samples[channel as usize];
                self.state.phase = if byte == 0 {
                    Phase::Sample { channel, byte: 1 }
                } else {
                    Phase::Finished
                };
                if byte == 0 { word >> 8 } else { word & 255 }
            }
            Phase::Block(index) => {
                let block = self.block.expect("command 8a is accepted only with a block");
                self.state.phase = if index == 5 { Phase::Finished } else { Phase::Block(index + 1) };
                block[index as usize] as u16
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
            || matches!(state.phase, Phase::Read(a) if a > 127)
            || matches!(state.phase, Phase::Sample { channel, byte } if channel > 1 || byte > 1)
            || matches!(state.phase, Phase::Block(index) if index > 5 || self.block.is_none()) {
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
        json!({"revision":self.revision,"samples":self.samples,"idle_high":self.idle_high,"block":self.block,"state":self.state})
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        if v["revision"] != json!(self.revision)
            || v["samples"] != json!(self.samples)
            || v["idle_high"] != json!(self.idle_high)
            || v["block"] != json!(self.block)
        {
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
            Phase::Sample { channel, byte } if channel > 1 || byte > 1 => {
                return Err("invalid BIS conversion checkpoint".into());
            }
            Phase::Block(index) if index > 5 || self.block.is_none() => {
                return Err("invalid BIS block checkpoint".into());
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
    fn exchange(part: &mut Registers, words: &[u16]) -> Result<Vec<u16>, String> {
        part.reset_transaction();
        let mut out = Vec::new();
        for &word in words {
            part.validate(frame(word))?;
            out.push(part.transfer(frame(word), 0));
        }
        Ok(out)
    }

    #[test]
    fn conversion_commands_return_their_word_most_significant_byte_first() {
        let mut part = Registers::new([3, 12], [0x1234, 0xabc0], false).unwrap();
        assert_eq!(exchange(&mut part, &[0xa8, 0, 0]).unwrap(), [0, 0x12, 0x34]);
        assert_eq!(exchange(&mut part, &[0xc8, 0, 0]).unwrap(), [0, 0xab, 0xc0]);
    }

    #[test]
    fn conversion_refuses_a_third_data_byte_and_unknown_commands() {
        let mut part = Registers::new([0, 0], [0, 0], true).unwrap();
        let error = exchange(&mut part, &[0xa8, 0, 0, 0]).unwrap_err();
        assert!(error.contains("past its last byte"), "{error}");
        let error = exchange(&mut part, &[0x30]).unwrap_err();
        assert!(error.contains("command 0x30"), "{error}");
    }

    #[test]
    fn checkpoint_carries_a_conversion_in_progress() {
        let mut part = Registers::new([0, 0], [0x0102, 0], false).unwrap();
        part.reset_transaction();
        part.transfer(frame(0xa8), 0);
        part.transfer(frame(0), 0);
        let saved = part.save();
        let mut copy = Registers::new([0, 0], [0x0102, 0], false).unwrap();
        copy.restore(&saved).unwrap();
        assert_eq!(copy.transfer(frame(0), 0), 2);
        let mut other = Registers::new([0, 0], [0x0103, 0], false).unwrap();
        assert!(other.restore(&saved).is_err());
    }

    #[test]
    fn command_8a_reads_the_declared_block_and_refuses_without_it() {
        let block = [1, 2, 3, 4, 5, 6];
        let mut part = Registers::with_block([0, 0], [0, 0], false, Some(block)).unwrap();
        assert_eq!(exchange(&mut part, &[0x8a, 0, 0, 0, 0, 0, 0]).unwrap(), [0, 1, 2, 3, 4, 5, 6]);
        let error = exchange(&mut part, &[0x8a, 0, 0, 0, 0, 0, 0, 0]).unwrap_err();
        assert!(error.contains("past its last byte"), "{error}");
        let mut bare = Registers::new([0, 0], [0, 0], false).unwrap();
        let error = exchange(&mut bare, &[0x8a]).unwrap_err();
        assert!(error.contains("0x8a"), "{error}");
        assert!(bare.restore(&part.save()).is_err());
    }

    #[test]
    fn revision_reads_and_servo_burst_reads() {
        let mut part = Registers::with_block([3, 5], [0, 0], false, Some([0; 6])).unwrap();
        assert_eq!(exchange(&mut part, &[0xa1, 0]).unwrap(), vec![0, 3]);
        assert_eq!(exchange(&mut part, &[0xa2, 0]).unwrap(), vec![0, 5]);
        // Burst read across the revision nibbles auto-increments.
        assert_eq!(exchange(&mut part, &[0xa0, 0, 0, 0]).unwrap(), vec![0, 0, 3, 5]);
        // The servo's polling commands read zero data.
        assert_eq!(exchange(&mut part, &[0x8a, 0, 0, 0, 0, 0, 0]).unwrap(), vec![0; 7]);
        assert_eq!(exchange(&mut part, &[0xa8, 0, 0]).unwrap(), vec![0; 3]);
        assert_eq!(exchange(&mut part, &[0xc8, 0, 0]).unwrap(), vec![0; 3]);
        // Writes stay single-register; unknown write commands refuse.
        assert_eq!(exchange(&mut part, &[0x1c, 0x4b]).unwrap(), vec![0, 0]);
        assert!(part.validate(frame(0)).is_err());
        part.reset_transaction();
        assert!(part.validate(frame(0x30)).is_err());
        let saved = part.checkpoint().unwrap();
        let mut fresh = Registers::with_block([3, 5], [0, 0], false, Some([0; 6])).unwrap();
        fresh.restore_checkpoint(&saved).unwrap();
        assert_eq!(fresh.checkpoint().unwrap(), saved);
    }
}
