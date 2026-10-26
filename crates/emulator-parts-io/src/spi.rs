//! Timed serial endpoints with mechanical effects and explicit receive clocks.
//! A controller owns its endpoint; calls cross no runtime or language boundary.
use fujimod_emulator_devices::Cx;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shift {
    pub word: u16,
    pub bits: u8,
    pub receive: bool,
}

pub trait Endpoint: Send {
    fn status_value(&self, _register: u8) -> Result<u8, String> {
        Err("serial endpoint has no external register status input".into())
    }
    /// Explicit external status input; command registers and clock bytes are
    /// owned by their protocol and refuse this physical-input capability.
    fn status_input(&mut self, _register: u8, _value: u8) -> Result<(), String> {
        Err("serial endpoint has no external register status input".into())
    }
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        Err(format!("{} has no binary snapshot", self.name()))
    }
    fn validate_checkpoint(&self, _: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        Err(format!("{} has no binary snapshot", self.name()))
    }
    fn restore_checkpoint(&mut self, _: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        Err(format!("{} has no binary snapshot", self.name()))
    }

    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Err("serial endpoint has no complete state fingerprint".into())
    }
    /// Observational preflight before accepting a frame and at the final edge.
    fn validate(&self, shift: Shift) -> Result<(), String>;
    /// Called only at the final serial edge, after controller completion status.
    fn transfer(&mut self, cx: &mut Cx<'_>, shift: Shift, at: u64) -> Result<u16, String>;
    fn advance(&mut self, _cx: &mut Cx<'_>, _at: u64) -> Result<(), String> {
        Ok(())
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        None
    }
    fn state(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClockOptions {
    pub registers: [u8; 4],
    pub initial_seconds: u32,
    pub ticks_per_second: u64,
    pub maximum_seconds: u32,
    pub started: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterOptions {
    pub registers: Vec<(u8, u8)>,
    pub readable: bool,
    pub status_registers: Vec<u8>,
    pub clock: Option<ClockOptions>,
}
/// Byte-addressed external register protocol: write bit15, address14:8,
/// value7:0. Zero write acknowledgment and reset values are declared inputs.
pub struct RegisterFile {
    values: [u8; 128],
    present: [bool; 128],
    status: [bool; 128],
    readable: bool,
    clock: Option<ClockOptions>,
}
impl RegisterFile {
    pub fn new(options: RegisterOptions) -> Result<Self, String> {
        let mut values = [0; 128];
        let mut present = [false; 128];
        let mut status = [false; 128];
        for (address, value) in options.registers {
            if address >= 128 || present[address as usize] {
                return Err("invalid serial register bank".into());
            }
            present[address as usize] = true;
            values[address as usize] = value;
        }
        for address in options.status_registers {
            if address >= 128 || !present[address as usize] {
                return Err("unknown serial status register".into());
            }
            status[address as usize] = true;
        }
        if let Some(clock) = &options.clock {
            if clock.ticks_per_second == 0
                || clock.initial_seconds > clock.maximum_seconds
                || clock
                    .registers
                    .iter()
                    .any(|&r| r >= 128 || !present[r as usize])
            {
                return Err("invalid serial register clock input".into());
            }
        }
        Ok(Self {
            values,
            present,
            status,
            readable: options.readable,
            clock: options.clock,
        })
    }
}
impl Endpoint for RegisterFile {
    fujimod_emulator_devices::checkpoint_fields!(#[serde(with="fujimod_emulator_devices::checkpoint::array")] values:[u8;128],#[serde(with="fujimod_emulator_devices::checkpoint::array")] present:[bool;128],#[serde(with="fujimod_emulator_devices::checkpoint::array")] status:[bool;128],readable:bool,clock:Option<ClockOptions>);

    fn status_value(&self, register: u8) -> Result<u8, String> {
        let index = usize::from(register);
        if index >= self.values.len() || !self.present[index] || !self.status[index] {
            return Err("serial input requires a declared external status register".into());
        }
        if self
            .clock
            .as_ref()
            .is_some_and(|c| c.registers.contains(&register))
        {
            return Err("serial clock register is driven by its declared clock".into());
        }
        Ok(self.values[index])
    }
    fn status_input(&mut self, register: u8, value: u8) -> Result<(), String> {
        self.status_value(register)?;
        self.values[usize::from(register)] = value;
        Ok(())
    }
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"values":self.values.as_slice(),"present":self.present.as_slice(),
            "status":self.status.as_slice(),"readable":self.readable,"clock":self.clock}),
        )
    }
    fn state(&self) -> serde_json::Value {
        serde_json::json!({"registers": self.values.iter().enumerate()
            .filter(|(address, _)| self.present[*address])
            .map(|(address, value)| (address as u8, *value))
            .collect::<Vec<_>>()})
    }
    fn validate(&self, shift: Shift) -> Result<(), String> {
        let address = (shift.word >> 8 & 127) as usize;
        if shift.bits != 16
            || !self.present[address]
            || shift.word & 0x8000 == 0 && (!self.readable || shift.word & 255 != 0)
        {
            return Err("unsupported serial register command".into());
        }
        Ok(())
    }
    fn transfer(&mut self, _: &mut Cx<'_>, shift: Shift, at: u64) -> Result<u16, String> {
        self.validate(shift)?;
        let address = (shift.word >> 8 & 127) as usize;
        if shift.word & 0x8000 != 0 {
            if !self.status[address] {
                self.values[address] = shift.word as u8;
                if let Some(clock) = &mut self.clock {
                    if let Some(byte) = clock.registers.iter().position(|&r| r as usize == address)
                    {
                        let seconds = (u64::from(clock.initial_seconds)
                            + at.saturating_sub(clock.started) / clock.ticks_per_second)
                            .min(u64::from(u32::MAX)) as u32;
                        let shift_bits = byte * 8;
                        clock.initial_seconds = (seconds & !(255 << shift_bits))
                            | (u32::from(shift.word as u8) << shift_bits);
                        clock.started = at;
                    }
                }
            }
            return Ok(0);
        }
        if let Some(clock) = &self.clock {
            if let Some(byte) = clock.registers.iter().position(|&r| r as usize == address) {
                let seconds = (u64::from(clock.initial_seconds)
                    + at.saturating_sub(clock.started) / clock.ticks_per_second)
                    .min(u64::from(clock.maximum_seconds));
                return Ok((seconds >> (byte * 8) & 255) as u16);
            }
        }
        Ok(u16::from(self.values[address]))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PanelOptions {
    pub used_mask: u16,
}
pub struct Panel {
    mask: u16,
    received: u64,
    words: Vec<u16>,
}
impl Panel {
    pub fn new(options: PanelOptions) -> Self {
        Self {
            mask: options.used_mask,
            received: 0,
            words: Vec::new(),
        }
    }
}
impl Endpoint for Panel {
    fujimod_emulator_devices::checkpoint_fields!(mask:u16, received:u64, words:Vec<u16>);
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(self.state())
    }
    fn state(&self) -> serde_json::Value {
        serde_json::json!({"mask":self.mask,"received":self.received,"words":self.words})
    }
    fn validate(&self, shift: Shift) -> Result<(), String> {
        if shift.bits != 16 || shift.receive || shift.word & !self.mask != 0 {
            Err("unsupported panel serial frame".into())
        } else {
            Ok(())
        }
    }
    fn transfer(&mut self, _: &mut Cx<'_>, shift: Shift, _: u64) -> Result<u16, String> {
        self.validate(shift)?;
        self.received += 1;
        if self.words.len() == 512 {
            self.words.remove(0);
        }
        self.words.push(shift.word);
        Ok(0)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixedOptions {
    pub bits: u8,
    pub word: u16,
}
pub struct FixedInput(FixedOptions);
impl FixedInput {
    pub fn new(options: FixedOptions) -> Result<Self, String> {
        if !matches!(options.bits, 8 | 16) || u32::from(options.word) >= 1 << options.bits {
            return Err("invalid fixed serial wire input".into());
        }
        Ok(Self(options))
    }
}
impl Endpoint for FixedInput {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(self.name(), &self.0)
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let _: FixedOptions = saved.decode(self.name())?;
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.0 = saved.decode(self.name())?;
        Ok(())
    }

    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!(self.0))
    }
    fn validate(&self, shift: Shift) -> Result<(), String> {
        if shift.bits == self.0.bits {
            Ok(())
        } else {
            Err("fixed serial input width differs".into())
        }
    }
    fn transfer(&mut self, _: &mut Cx<'_>, shift: Shift, _: u64) -> Result<u16, String> {
        self.validate(shift)?;
        Ok(self.0.word)
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;
    use fujimod_emulator_devices::test_host::TestHost;

    #[test]
    fn clock_register_writes_set_the_running_counter_and_restore_exactly() {
        let mut part = RegisterFile::new(RegisterOptions {
            registers: (0x41..=0x44).map(|register| (register, 0)).collect(),
            readable: true,
            status_registers: vec![],
            clock: Some(ClockOptions {
                registers: [0x41, 0x42, 0x43, 0x44],
                initial_seconds: 0x0012_3456,
                ticks_per_second: 100,
                maximum_seconds: u32::MAX,
                started: 0,
            }),
        })
        .unwrap();
        let mut host = TestHost::new();
        let read = |part: &mut RegisterFile, host: &mut TestHost, at| {
            (0..4).fold(0, |seconds, byte| {
                let value = part
                    .transfer(
                        &mut host.cx(),
                        Shift {
                            word: (0x41 + byte) << 8,
                            bits: 16,
                            receive: true,
                        },
                        at,
                    )
                    .unwrap();
                seconds | (u32::from(value) << (byte * 8))
            })
        };
        assert_eq!(read(&mut part, &mut host, 250), 0x0012_3458);
        let wanted = 0x0034_12fe_u32;
        for (byte, value) in wanted.to_le_bytes().into_iter().enumerate() {
            part.transfer(
                &mut host.cx(),
                Shift {
                    word: 0x8000 | ((0x41 + byte as u16) << 8) | u16::from(value),
                    bits: 16,
                    receive: false,
                },
                250,
            )
            .unwrap();
            if byte == 0 {
                assert_eq!(read(&mut part, &mut host, 250), 0x0012_34fe);
            }
        }
        let saved = part.checkpoint().unwrap();
        assert_eq!(read(&mut part, &mut host, 550), wanted + 3);
        part.transfer(
            &mut host.cx(),
            Shift {
                word: 0xc100,
                bits: 16,
                receive: false,
            },
            550,
        )
        .unwrap();
        assert_ne!(read(&mut part, &mut host, 550), wanted + 3);
        part.restore_checkpoint(&saved).unwrap();
        assert_eq!(read(&mut part, &mut host, 550), wanted + 3);
    }

    #[test]
    fn external_status_is_not_a_command_or_clock_write_and_restores_exactly() {
        let mut part = RegisterFile::new(RegisterOptions {
            registers: vec![
                (0x41, 0),
                (0x42, 0),
                (0x43, 0),
                (0x44, 0),
                (0x50, 1),
                (0x60, 0x43),
            ],
            readable: true,
            status_registers: vec![0x41, 0x60],
            clock: Some(ClockOptions {
                registers: [0x41, 0x42, 0x43, 0x44],
                initial_seconds: 0,
                ticks_per_second: 100,
                maximum_seconds: 1000,
                started: 0,
            }),
        })
        .unwrap();
        let initial = part.fingerprint().unwrap();
        for register in [0x41, 0x50, 0x7f, 0xff] {
            assert!(part.status_input(register, 0).is_err());
            assert_eq!(part.fingerprint().unwrap(), initial);
        }
        let saved = part.checkpoint().unwrap();
        part.status_input(0x60, 0x42).unwrap();
        let mut host = TestHost::new();
        let read = Shift {
            word: 0x6000,
            bits: 16,
            receive: true,
        };
        assert_eq!(part.transfer(&mut host.cx(), read, 0).unwrap(), 0x42);
        part.transfer(
            &mut host.cx(),
            Shift {
                word: 0xe0f0,
                ..read
            },
            0,
        )
        .unwrap();
        assert_eq!(part.transfer(&mut host.cx(), read, 0).unwrap(), 0x42);
        part.restore_checkpoint(&saved).unwrap();
        assert_eq!(part.fingerprint().unwrap(), initial);
        assert_eq!(part.transfer(&mut host.cx(), read, 0).unwrap(), 0x43);
        let mut panel = Panel::new(PanelOptions { used_mask: 0xffff });
        assert!(panel.status_input(0x60, 0).is_err());
    }
}

#[cfg(test)]
mod panel_observation_tests {
    use super::*;
    use fujimod_emulator_devices::test_host::TestHost;

    #[test]
    fn panel_observes_only_completed_words_and_restores_the_bounded_history() {
        let mut panel = Panel::new(PanelOptions { used_mask: 0xffff });
        let mut host = TestHost::new();
        let shift = Shift {
            bits: 16,
            word: 0x1b00,
            receive: false,
        };
        let before = panel.fingerprint().unwrap();
        panel.validate(shift).unwrap();
        assert_eq!(panel.fingerprint().unwrap(), before);
        panel.transfer(&mut host.cx(), shift, 1).unwrap();
        let saved = panel.checkpoint().unwrap();
        for word in 0..520 {
            panel
                .transfer(&mut host.cx(), Shift { word, ..shift }, 2)
                .unwrap();
        }
        let state = panel.state();
        assert_eq!(state["received"], 521);
        assert_eq!(state["words"].as_array().unwrap().len(), 512);
        assert_eq!(state["words"][0], 8);
        panel.restore_checkpoint(&saved).unwrap();
        assert_eq!(panel.state()["received"], 1);
        assert_eq!(panel.state()["words"], serde_json::json!([0x1b00]));
        let invalid = Shift { bits: 8, ..shift };
        let stable = panel.fingerprint().unwrap();
        assert!(panel.transfer(&mut host.cx(), invalid, 3).is_err());
        assert_eq!(panel.fingerprint().unwrap(), stable);
    }
}
