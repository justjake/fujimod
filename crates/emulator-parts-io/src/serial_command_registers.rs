//! Write-only command/data register endpoint, identity unproved.
//! One chip-select transaction is a command word followed by data words, all
//! of the declared width and SPI mode. Where the part encodes reads in the
//! command word, the declared read flag bits mark a read and the rest name the
//! register; a part without one reads through frames of another format, which
//! this endpoint does not accept. Written data is
//! retained per command word, in the order it was sent; no register
//! auto-increment is assumed. A read is refused because the part's reply is not
//! known. MISO during writes is an explicit pin input, not a reply.
use super::serial::{Endpoint, Frame};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The declared wire format of one attached part.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wiring {
    /// Word width, 8 or 16 bits.
    pub bits: u8,
    /// Motorola SPI mode 0..3.
    pub mode: u8,
    /// Command bits that mark a read, if the command word encodes one.
    pub read_flag: Option<u16>,
    /// Data words one transaction carries at most; `None` for a burst of any length.
    pub data_words: Option<u16>,
    /// MISO level while the part is written.
    pub miso_high: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    registers: BTreeMap<u16, Vec<u16>>,
    command: Option<u16>,
    data_words: u16,
    writes: u64,
}
pub struct CommandRegisters {
    state: State,
    wiring: Wiring,
}
impl CommandRegisters {
    pub fn new(wiring: Wiring) -> Result<Self, String> {
        if wiring.mode > 3 {
            return Err("serial command registers require SPI mode 0..3".into());
        }
        if !matches!(wiring.bits, 8 | 16) {
            return Err("serial command registers require 8- or 16-bit words".into());
        }
        if wiring
            .read_flag
            .is_some_and(|flag| flag == 0 || u32::from(flag) >> wiring.bits != 0)
        {
            return Err("serial command read flag must be nonzero bits of the word".into());
        }
        if wiring.data_words == Some(0) {
            return Err("serial command transactions carry at least one data word".into());
        }
        Ok(Self {
            state: State::default(),
            wiring,
        })
    }
    fn reads(&self, command: u16) -> bool {
        self.wiring.read_flag.is_some_and(|flag| command & flag != 0)
    }
    /// The data words last written after this command word.
    pub fn register(&self, command: u16) -> Option<&[u16]> {
        self.state.registers.get(&command).map(Vec::as_slice)
    }
    fn consistent(&self, state: &State) -> bool {
        let read = |word: u16| self.reads(word);
        let limit = self.wiring.data_words.unwrap_or(u16::MAX);
        !state.command.is_some_and(read)
            && (state.command.is_some() || state.data_words == 0)
            && state.data_words <= limit
            && state
                .registers
                .iter()
                .all(|(&command, data)| !read(command) && !data.is_empty())
    }
}
impl Endpoint for CommandRegisters {
    fn name(&self) -> &'static str {
        "serial-command-registers"
    }
    fn validate(&self, frame: Frame) -> Result<(), String> {
        if frame.bits != self.wiring.bits || frame.mode != self.wiring.mode {
            return Err(format!(
                "serial command registers require {}-bit mode {} frames",
                self.wiring.bits, self.wiring.mode
            ));
        }
        match self.state.command {
            None if self.reads(frame.word) => Err(format!(
                "serial command register read {:#06x} has no known reply",
                frame.word & !self.wiring.read_flag.unwrap_or(0)
            )),
            Some(_) if Some(self.state.data_words) == self.wiring.data_words => {
                Err("serial command transaction already has all its data words".into())
            }
            _ => Ok(()),
        }
    }
    fn reset_transaction(&mut self) {
        self.state.command = None;
        self.state.data_words = 0;
    }
    fn transfer(&mut self, frame: Frame, _: u64) -> u16 {
        match self.state.command {
            None => self.state.command = Some(frame.word),
            Some(command) => {
                let data = self.state.registers.entry(command).or_default();
                if self.state.data_words == 0 {
                    data.clear();
                    self.state.writes += 1;
                }
                data.push(frame.word);
                self.state.data_words += 1;
            }
        }
        if self.wiring.miso_high {
            ((1u32 << self.wiring.bits) - 1) as u16
        } else {
            0
        }
    }
    fn save(&self) -> Value {
        json!({"wiring":self.wiring,"registers":self.state})
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        let wiring: Wiring =
            serde_json::from_value(state["wiring"].clone()).map_err(|e| e.to_string())?;
        if wiring != self.wiring {
            return Err("serial command register wiring differs from checkpoint".into());
        }
        let decoded: State =
            serde_json::from_value(state["registers"].clone()).map_err(|e| e.to_string())?;
        if !self.consistent(&decoded) {
            return Err("serial command register checkpoint is inconsistent".into());
        }
        self.state = decoded;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORD: Wiring = Wiring {
        bits: 16,
        mode: 0,
        read_flag: Some(0x8000),
        data_words: Some(1),
        miso_high: false,
    };
    const BURST: Wiring = Wiring {
        bits: 8,
        mode: 0,
        read_flag: Some(0x01),
        data_words: None,
        miso_high: false,
    };

    fn frame(wiring: Wiring, word: u16) -> Frame {
        Frame {
            bits: wiring.bits,
            mode: wiring.mode,
            word,
        }
    }
    fn send(part: &mut CommandRegisters, words: &[u16]) {
        part.reset_transaction();
        for &word in words {
            let frame = frame(part.wiring, word);
            part.validate(frame).unwrap();
            part.transfer(frame, 0);
        }
    }

    #[test]
    fn transactions_retain_written_registers() {
        let mut part = CommandRegisters::new(WORD).unwrap();
        send(&mut part, &[0x000f, 0x6243]);
        send(&mut part, &[0x0408, 0x8125]);
        send(&mut part, &[0x000f, 0x0001]);
        assert_eq!(part.register(0x000f), Some(&[0x0001][..]));
        assert_eq!(part.register(0x0408), Some(&[0x8125][..]));
        assert_eq!(part.state.writes, 3);
    }

    #[test]
    fn a_burst_keeps_its_data_words_in_order_under_the_command() {
        let mut part = CommandRegisters::new(BURST).unwrap();
        send(&mut part, &[0x0a, 0x11, 0x22, 0x33]);
        assert_eq!(part.register(0x0a), Some(&[0x11, 0x22, 0x33][..]));
        send(&mut part, &[0x0a, 0x44]);
        assert_eq!(part.register(0x0a), Some(&[0x44][..]));
        assert_eq!(part.register(0x0c), None);
        assert_eq!(part.state.writes, 2);
    }

    #[test]
    fn a_read_command_is_refused_before_any_reply() {
        let mut part = CommandRegisters::new(WORD).unwrap();
        part.reset_transaction();
        let error = part.validate(frame(WORD, 0x8019)).unwrap_err();
        assert!(error.contains("0x0019"), "{error}");
        let error = CommandRegisters::new(BURST)
            .unwrap()
            .validate(frame(BURST, 0x0b))
            .unwrap_err();
        assert!(error.contains("0x000a"), "{error}");
    }

    #[test]
    fn without_a_read_flag_every_command_word_is_a_write() {
        let mut part = CommandRegisters::new(Wiring {
            mode: 2,
            read_flag: None,
            ..BURST
        })
        .unwrap();
        send(&mut part, &[0x81, 0x05]);
        assert_eq!(part.register(0x81), Some(&[0x05][..]));
        let read = Frame { bits: 16, mode: 2, word: 0x0081 };
        assert!(part.validate(read).unwrap_err().contains("8-bit mode 2"));
    }

    #[test]
    fn a_word_past_the_declared_data_words_is_refused() {
        let mut part = CommandRegisters::new(WORD).unwrap();
        send(&mut part, &[0x0019, 1]);
        assert!(part.validate(frame(WORD, 0x001d)).is_err());
    }

    #[test]
    fn mismatched_frames_and_wiring_are_refused() {
        let part = CommandRegisters::new(Wiring { mode: 3, ..WORD }).unwrap();
        assert!(part.validate(frame(WORD, 0x0019)).is_err());
        assert!(
            CommandRegisters::new(BURST)
                .unwrap()
                .validate(frame(WORD, 0x0a))
                .is_err()
        );
        assert!(CommandRegisters::new(Wiring { mode: 4, ..WORD }).is_err());
        assert!(CommandRegisters::new(Wiring { bits: 12, ..WORD }).is_err());
        assert!(CommandRegisters::new(Wiring { read_flag: Some(0), ..WORD }).is_err());
        assert!(CommandRegisters::new(Wiring { read_flag: Some(0x100), ..BURST }).is_err());
        assert!(CommandRegisters::new(Wiring { data_words: Some(0), ..WORD }).is_err());
    }

    #[test]
    fn miso_level_during_writes_is_the_declared_input() {
        for (wiring, level) in [(WORD, 0xffff), (BURST, 0xff)] {
            let mut part = CommandRegisters::new(Wiring {
                miso_high: true,
                ..wiring
            })
            .unwrap();
            part.reset_transaction();
            assert_eq!(part.transfer(frame(wiring, 0x0018), 0), level);
        }
    }

    #[test]
    fn checkpoint_restores_a_partial_transaction() {
        let mut part = CommandRegisters::new(WORD).unwrap();
        send(&mut part, &[0x0019, 7]);
        part.reset_transaction();
        part.transfer(frame(WORD, 0x001d), 0);
        let saved = part.save();
        let mut restored = CommandRegisters::new(WORD).unwrap();
        restored.restore(&saved).unwrap();
        restored.transfer(frame(WORD, 0x0042), 0);
        assert_eq!(restored.register(0x001d), Some(&[0x0042][..]));
        assert_eq!(restored.register(0x0019), Some(&[7][..]));
        assert!(
            CommandRegisters::new(Wiring { mode: 1, ..WORD })
                .unwrap()
                .restore(&saved)
                .is_err()
        );
        let mut read = saved.clone();
        read["registers"]["command"] = json!(0x8001);
        assert!(CommandRegisters::new(WORD).unwrap().restore(&read).is_err());
    }
}
