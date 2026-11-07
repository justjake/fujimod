//! Write-only 16-bit command/data register endpoint, identity unproved.
//! One chip-select transaction is a command word and a data word. Bit 15 of
//! the command marks a read and bits 14..0 name the register. Written values
//! are retained; a read is refused because the part's reply is not known.
//! MISO during writes is an explicit pin input, not a reply.
use super::serial::{Endpoint, Frame};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const READ: u16 = 0x8000;

#[derive(Default, Serialize, Deserialize)]
struct State {
    registers: BTreeMap<u16, u16>,
    command: Option<u16>,
    complete: bool,
    writes: u64,
}
pub struct CommandRegisters {
    state: State,
    mode: u8,
    miso_high: bool,
}
impl CommandRegisters {
    pub fn new(mode: u8, miso_high: bool) -> Result<Self, String> {
        if mode > 3 {
            return Err("serial command registers require SPI mode 0..3".into());
        }
        Ok(Self {
            state: State::default(),
            mode,
            miso_high,
        })
    }
    pub fn register(&self, register: u16) -> Option<u16> {
        self.state.registers.get(&register).copied()
    }
}
impl Endpoint for CommandRegisters {
    fn name(&self) -> &'static str {
        "serial-command-registers"
    }
    fn validate(&self, frame: Frame) -> Result<(), String> {
        if frame.bits != 16 || frame.mode != self.mode {
            return Err(format!(
                "serial command registers require 16-bit mode {} frames",
                self.mode
            ));
        }
        if self.state.complete {
            return Err("serial command transaction already has its data word".into());
        }
        if self.state.command.is_none() && frame.word & READ != 0 {
            return Err(format!(
                "serial command register read {:#06x} has no known reply",
                frame.word & !READ
            ));
        }
        Ok(())
    }
    fn reset_transaction(&mut self) {
        self.state.command = None;
        self.state.complete = false;
    }
    fn transfer(&mut self, frame: Frame, _: u64) -> u16 {
        match self.state.command {
            None => self.state.command = Some(frame.word),
            Some(register) => {
                self.state.registers.insert(register, frame.word);
                self.state.complete = true;
                self.state.writes += 1;
            }
        }
        if self.miso_high { u16::MAX } else { 0 }
    }
    fn save(&self) -> Value {
        json!({"mode":self.mode,"miso_high":self.miso_high,"registers":self.state})
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        if state["mode"].as_u64() != Some(u64::from(self.mode))
            || state["miso_high"].as_bool() != Some(self.miso_high)
        {
            return Err("serial command register wiring differs from checkpoint".into());
        }
        let decoded: State =
            serde_json::from_value(state["registers"].clone()).map_err(|e| e.to_string())?;
        if decoded.command.is_some_and(|word| word & READ != 0)
            || (decoded.complete && decoded.command.is_none())
            || decoded.registers.keys().any(|register| register & READ != 0)
        {
            return Err("serial command register checkpoint is inconsistent".into());
        }
        self.state = decoded;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(word: u16) -> Frame {
        Frame {
            bits: 16,
            mode: 0,
            word,
        }
    }
    fn write(part: &mut CommandRegisters, register: u16, value: u16) {
        part.reset_transaction();
        for word in [register, value] {
            part.validate(frame(word)).unwrap();
            part.transfer(frame(word), 0);
        }
    }

    #[test]
    fn transactions_retain_written_registers() {
        let mut part = CommandRegisters::new(0, false).unwrap();
        write(&mut part, 0x000f, 0x6243);
        write(&mut part, 0x0408, 0x8125);
        write(&mut part, 0x000f, 0x0001);
        assert_eq!(part.register(0x000f), Some(0x0001));
        assert_eq!(part.register(0x0408), Some(0x8125));
        assert_eq!(part.state.writes, 3);
    }

    #[test]
    fn a_read_command_is_refused_before_any_reply() {
        let mut part = CommandRegisters::new(0, false).unwrap();
        part.reset_transaction();
        let error = part.validate(frame(0x8019)).unwrap_err();
        assert!(error.contains("0x0019"), "{error}");
    }

    #[test]
    fn a_third_word_in_one_transaction_is_refused() {
        let mut part = CommandRegisters::new(0, false).unwrap();
        write(&mut part, 0x0019, 1);
        assert!(part.validate(frame(0x001d)).is_err());
    }

    #[test]
    fn mismatched_frames_are_refused() {
        let part = CommandRegisters::new(3, false).unwrap();
        assert!(part.validate(frame(0x0019)).is_err());
        assert!(CommandRegisters::new(4, false).is_err());
    }

    #[test]
    fn miso_level_during_writes_is_the_declared_input() {
        let mut part = CommandRegisters::new(0, true).unwrap();
        part.reset_transaction();
        assert_eq!(part.transfer(frame(0x0019), 0), u16::MAX);
    }

    #[test]
    fn checkpoint_restores_a_partial_transaction() {
        let mut part = CommandRegisters::new(0, false).unwrap();
        write(&mut part, 0x0019, 7);
        part.reset_transaction();
        part.transfer(frame(0x001d), 0);
        let saved = part.save();
        let mut restored = CommandRegisters::new(0, false).unwrap();
        restored.restore(&saved).unwrap();
        restored.transfer(frame(0x0042), 0);
        assert_eq!(restored.register(0x001d), Some(0x0042));
        assert_eq!(restored.register(0x0019), Some(7));
        assert!(CommandRegisters::new(1, false).unwrap().restore(&saved).is_err());
    }
}
