//! LSM6DSO-compatible SPI configuration-register subset (ST DS12140).
//! No FIFO samples, data-ready interrupt or motion is supplied. The output
//! registers (STATUS_REG 0x1e, OUT_TEMP 0x20..0x21, gyroscope 0x22..0x27,
//! accelerometer 0x28..0x2d) read as a still body: data ready, zero rate, zero
//! acceleration and 25 C. The X-T5 4.31 IBIS servo polls them with burst read
//! `a0` (M-core command table 0x32714) once it runs. Gravity is not applied;
//! the still-body outputs are a scenario input, not a measurement.
//! Software reset commits atomically with its register write on the guest wire;
//! internal reset latency and the exact silicon identity are unmeasured.
use fujimod_emulator_devices::checkpoint::Checkpoint;
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
}
impl Default for Registers {
    fn default() -> Self {
        let mut s = Self {
            state: State::default(),
        };
        s.reset();
        s
    }
}
impl Registers {
    fn reset(&mut self) {
        self.state.registers = [0; 32];
        self.state.registers[0x12] = 4;
        self.state.offsets = [0; 3];
    }
    fn supported(address: u8) -> bool {
        matches!(address,1|0xd|0xf|0x10..=0x17|0x73..=0x75) || Self::output(address)
    }
    /// Read-only status and output registers.
    fn output(address: u8) -> bool {
        matches!(address, 0x1e | 0x20..=0x2d)
    }
    fn value(&self, address: u8) -> u8 {
        match address {
            0xf => 0x6c,
            // XLDA, GDA and TDA: every output holds a fresh still-body sample.
            0x1e => 7,
            0x20..=0x2d => 0,
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
                return Err("LSM6DS data/status or register is unmodeled".into());
            }
            if !self.state.read
                && (address == 0xf
                    || Self::output(address)
                    || address == 1 && word != 0
                    || address == 0xd && word != 0
                    || address == 0x12 && word & !0x75 != 0)
            {
                return Err("LSM6DS bank, interrupt or serial mode is unmodeled".into());
            }
        } else if !Self::supported(word & 127) {
            return Err("LSM6DS data/status or register is unmodeled".into());
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
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), &self.state)
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let state: State = saved.decode(self.name())?;
        if state.address.is_some_and(|a| a > 0x7f) || state.registers[0x12] & !0x74 != 0
            || state.registers[1] != 0 || state.registers[0xd] != 0 {
            return Err("invalid LSM6DS checkpoint".into());
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        self.state = saved.decode(self.name())?;
        Ok(())
    }
    fn save(&self) -> Value {
        json!(self.state)
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
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
    fn exchange(part: &mut Registers, words: &[u16]) -> Result<Vec<u16>, String> {
        part.reset_transaction();
        words
            .iter()
            .map(|&w| {
                let frame = Frame { bits: 8, mode: 3, word: w };
                part.validate(frame)?;
                Ok(part.transfer(frame, 0))
            })
            .collect()
    }
    #[test]
    fn still_body_outputs_read_back_and_refuse_writes() {
        let mut part = Registers::default();
        assert_eq!(exchange(&mut part, &[0x8f, 0]).unwrap(), vec![0, 0x6c]);
        // The servo's burst: temperature, then gyroscope, auto-incremented.
        assert_eq!(exchange(&mut part, &[0xa0, 0, 0, 0, 0, 0, 0]).unwrap(), vec![0; 7]);
        assert_eq!(exchange(&mut part, &[0x9e, 0]).unwrap(), vec![0, 7]);
        assert!(exchange(&mut part, &[0x22, 1]).is_err());
        assert!(exchange(&mut part, &[0xb0, 0]).is_err());
    }
}
