//! Declared register-file parts on one X-Processor Pro I2C bus.
//!
//! The first byte written after a write address, masked by `select_mask`,
//! selects a register (a mask keeps command bits out of the address); later
//! bytes write it and reads return it, `width` bytes per register in the
//! declared byte order before the selection moves to the next register.
//! Register values are declared inputs, and a read of an undeclared register
//! is unsupported. Addresses without a part are NACKed. Written values read
//! back; no part behavior beyond the register file is modeled. The last
//! transfers are kept for inspection.
use fujimod_emulator_ip::dw_i2c::Bus;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};

const KEPT_TRANSFERS: usize = 64;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Part {
    pub address: u16,
    /// Bits of the first written byte that select the register.
    pub select_mask: u8,
    /// Bytes per register: 1 or 2.
    pub width: u8,
    pub little_endian: bool,
    pub registers: BTreeMap<u8, u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transfer {
    pub address: u16,
    pub read: bool,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    registers: Vec<BTreeMap<u8, u16>>,
    active: Option<usize>,
    selected: Option<u8>,
    byte: u8,
    transfers: VecDeque<Transfer>,
}

pub struct Parts {
    parts: Vec<Part>,
    state: State,
}

impl Parts {
    pub fn new(parts: Vec<Part>) -> Result<Self, String> {
        let mut addresses: Vec<_> = parts.iter().map(|p| p.address).collect();
        addresses.sort_unstable();
        addresses.dedup();
        if addresses.len() != parts.len()
            || parts.iter().any(|p| {
                p.address > 0x7f
                    || !matches!(p.width, 1 | 2)
                    || p.width == 1 && p.registers.values().any(|v| *v > 0xff)
            })
        {
            return Err("Invalid I2C part address, width or register value".into());
        }
        let state = State {
            registers: parts.iter().map(|p| p.registers.clone()).collect(),
            ..State::default()
        };
        Ok(Self { parts, state })
    }
    pub fn transfers(&self) -> impl Iterator<Item = &Transfer> {
        self.state.transfers.iter()
    }
    fn log(&mut self, byte: u8) {
        if let Some(transfer) = self.state.transfers.back_mut() {
            transfer.bytes.push(byte);
        }
    }
    /// Byte `n` of a register value in the part's byte order.
    fn shift(part: &Part, n: u8) -> u8 {
        8 * if part.little_endian {
            n
        } else {
            part.width - 1 - n
        }
    }
    fn next_byte(&mut self, part: &Part) {
        let s = &mut self.state;
        s.byte += 1;
        if s.byte == part.width {
            s.byte = 0;
            s.selected = s.selected.map(|r| r.wrapping_add(1) & part.select_mask);
        }
    }
}

impl Bus for Parts {
    fn start(&mut self, address: u16, read: bool) -> bool {
        let s = &mut self.state;
        s.active = self.parts.iter().position(|p| p.address == address);
        s.byte = 0;
        if !read {
            s.selected = None;
        }
        if s.transfers.len() == KEPT_TRANSFERS {
            s.transfers.pop_front();
        }
        s.transfers.push_back(Transfer {
            address,
            read,
            bytes: Vec::new(),
        });
        s.active.is_some()
    }
    fn write(&mut self, byte: u8) {
        self.log(byte);
        let Some(index) = self.state.active else {
            return;
        };
        let part = self.parts[index].clone();
        let Some(register) = self.state.selected else {
            self.state.selected = Some(byte & part.select_mask);
            return;
        };
        let shift = Self::shift(&part, self.state.byte);
        let value = self.state.registers[index].entry(register).or_default();
        *value = *value & !(0xff << shift) | u16::from(byte) << shift;
        self.next_byte(&part);
    }
    fn read(&mut self) -> Result<u8, String> {
        let Some(index) = self.state.active else {
            return Err("I2C read without an addressed part".into());
        };
        let part = self.parts[index].clone();
        let register = self.state.selected.unwrap_or(0);
        let Some(value) = self.state.registers[index].get(&register).copied() else {
            return Err(format!(
                "I2C part {:#x} register {register:#x} is not declared",
                part.address
            ));
        };
        let byte = (value >> Self::shift(&part, self.state.byte)) as u8;
        self.next_byte(&part);
        self.log(byte);
        Ok(byte)
    }
    fn stop(&mut self) {
        self.state.active = None;
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone())
            .map_err(|e| format!("invalid I2C parts checkpoint: {e}"))?;
        if state.registers.len() != self.parts.len()
            || state.active.is_some_and(|i| i >= self.parts.len())
        {
            return Err("I2C parts checkpoint differs from the declared parts".into());
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gauge() -> Parts {
        Parts::new(vec![Part {
            address: 0x36,
            select_mask: 0xff,
            width: 2,
            little_endian: true,
            registers: [(0x10, 0x1234)].into(),
        }])
        .unwrap()
    }

    #[test]
    fn word_registers_select_read_and_write_in_byte_order() {
        let mut parts = gauge();
        assert!(!parts.start(0x37, false));
        assert!(parts.start(0x36, false));
        parts.write(0x10);
        assert!(parts.start(0x36, true));
        assert_eq!(parts.read(), Ok(0x34));
        assert_eq!(parts.read(), Ok(0x12));
        assert!(parts.read().unwrap_err().contains("register 0x11"));
        parts.stop();
        let saved = parts.save();
        assert!(parts.start(0x36, false));
        for byte in [0x18, 0xbb, 0x03] {
            parts.write(byte);
        }
        assert!(parts.start(0x36, false));
        parts.write(0x18);
        assert!(parts.start(0x36, true));
        assert_eq!(parts.read(), Ok(0xbb));
        assert_eq!(parts.read(), Ok(0x03));
        assert_eq!(parts.transfers().count(), 6);
        parts.restore(&saved).unwrap();
        assert_eq!(parts.transfers().count(), 3);
        assert!(
            Parts::new(vec![Part {
                address: 0x80,
                select_mask: 0xff,
                width: 1,
                little_endian: true,
                registers: BTreeMap::new(),
            }])
            .is_err()
        );
    }

    #[test]
    fn a_select_mask_keeps_command_bits_out_of_the_register_address() {
        // X-T20 2.11 0xc76484: TMD2772-style command byte 0xa0 | address.
        let mut parts = Parts::new(vec![Part {
            address: 0x39,
            select_mask: 0x1f,
            width: 1,
            little_endian: true,
            registers: [(0x18, 0x34), (0x19, 0x02)].into(),
        }])
        .unwrap();
        assert!(parts.start(0x39, false));
        parts.write(0xb8);
        assert!(parts.start(0x39, true));
        assert_eq!(parts.read(), Ok(0x34));
        assert_eq!(parts.read(), Ok(0x02));
    }
}
