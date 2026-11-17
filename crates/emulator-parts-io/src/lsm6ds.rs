//! LSM6DSO-compatible SPI configuration-register subset (ST DS12140).
//! OUT_TEMP (20h/21h) returns an explicit scenario input; no synthetic
//! acceleration, angular rate, FIFO samples, data-ready or interrupt is supplied.
//! Software reset commits atomically with its register write on the guest wire;
//! internal reset latency and the exact silicon identity are unmeasured.
use super::serial::{Endpoint, Frame};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Default, Serialize, Deserialize)]
struct State {
    registers: [u8; 32],
    offsets: [u8; 3],
    address: Option<u8>,
    read: bool,
    transfers: u64,
}
pub struct Registers {
    state: State,
    temperature: i16,
}
impl Registers {
    /// `temperature` is the raw two's-complement OUT_TEMP word.
    pub fn new(temperature: i16) -> Self {
        let mut s = Self {
            state: State::default(),
            temperature,
        };
        s.reset();
        s
    }
    fn reset(&mut self) {
        self.state.registers = [0; 32];
        self.state.registers[0x12] = 4;
        self.state.offsets = [0; 3];
    }
    fn supported(address: u8) -> bool {
        matches!(address, 1 | 0xd | 0xf | 0x10..=0x17 | 0x20 | 0x21 | 0x73..=0x75)
    }
    fn value(&self, address: u8) -> u8 {
        match address {
            0xf => 0x6c,
            0x20 => self.temperature as u8,
            0x21 => (self.temperature >> 8) as u8,
            0x73..=0x75 => self.state.offsets[(address - 0x73) as usize],
            _ => self.state.registers[address as usize],
        }
    }
}
impl Endpoint for Registers {
    fn name(&self) -> &'static str {
        "lsm6ds-configuration-registers"
    }
    fn validate(&self, frame: Frame) -> Result<(), String> {
        if frame.bits != 8 || frame.mode != 3 || frame.word > 255 {
            return Err("LSM6DS SPI requires 8-bit mode3".into());
        }
        let word = frame.word as u8;
        if let Some(address) = self.state.address {
            if !Self::supported(address) {
                return Err(format!(
                    "LSM6DS data/status or register {address:#04x} is unmodeled"
                ));
            }
            if !self.state.read && matches!(address, 0x20 | 0x21) {
                return Err(format!("LSM6DS output register {address:#04x} is read-only"));
            }
            if !self.state.read
                && (address == 0xf
                    || address == 1 && word != 0
                    || address == 0xd && word != 0
                    || address == 0x12 && word & !0x75 != 0)
            {
                return Err("LSM6DS bank, interrupt or serial mode is unmodeled".into());
            }
        } else if !Self::supported(word & 127) {
            return Err(format!(
                "LSM6DS data/status or register {:#04x} is unmodeled",
                word & 127
            ));
        }
        Ok(())
    }
    fn reset_transaction(&mut self) {
        self.state.address = None;
        self.state.read = false;
    }
    fn transfer(&mut self, frame: Frame, _: u64) -> u16 {
        let word = frame.word as u8;
        self.state.transfers += 1;
        let Some(address) = self.state.address else {
            self.state.read = word & 128 != 0;
            self.state.address = Some(word & 127);
            return 0;
        };
        let response = if self.state.read {
            self.value(address)
        } else {
            if address == 0x12 && word & 1 != 0 {
                self.reset();
            } else if matches!(address, 0x73..=0x75) {
                self.state.offsets[(address - 0x73) as usize] = word;
            } else {
                self.state.registers[address as usize] = word;
            }
            0
        };
        if self.state.registers[0x12] & 4 != 0 {
            self.state.address = Some(address.wrapping_add(1));
        }
        response as u16
    }
    fn save(&self) -> Value {
        json!({"temperature": self.temperature, "state": self.state})
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        if v["temperature"] != json!(self.temperature) {
            return Err("LSM6DS temperature input differs".into());
        }
        let state: State =
            serde_json::from_value(v["state"].clone()).map_err(|e| e.to_string())?;
        if state.address.is_some_and(|a| a > 0x7f)
            || state.registers[0x12] & !0x74 != 0
            || state.registers[1] != 0
            || state.registers[0xd] != 0
        {
            return Err("invalid LSM6DS checkpoint state".into());
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
    fn temperature_reads_low_byte_first_with_auto_increment() {
        let mut part = Registers::new(-0x1234);
        exchange(&mut part, &[0x12, 0x44]).unwrap();
        let raw = (-0x1234i16) as u16;
        assert_eq!(
            exchange(&mut part, &[0xa0, 0, 0]).unwrap(),
            [0, raw & 255, raw >> 8]
        );
    }

    #[test]
    fn temperature_refuses_writes_and_the_following_gyroscope_output() {
        let mut part = Registers::new(0);
        let error = exchange(&mut part, &[0x20, 1]).unwrap_err();
        assert!(error.contains("read-only"), "{error}");
        let error = exchange(&mut part, &[0xa0, 0, 0, 0]).unwrap_err();
        assert!(error.contains("0x22"), "{error}");
    }

    #[test]
    fn checkpoint_requires_the_same_temperature_input() {
        let part = Registers::new(5);
        assert!(Registers::new(5).restore(&part.save()).is_ok());
        assert!(Registers::new(6).restore(&part.save()).is_err());
    }
}
