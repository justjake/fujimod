//! TMD2772-class ambient-light and proximity sensor (AMS TMD2772 register map),
//! as an eye sensor beside a viewfinder.
//!
//! Command bytes carry the register in bits 4:0 (bit 7 marks a command, bits
//! 6:5 the transaction type). Written configuration is kept and reads back.
//! The ID register reads 0x39, the TMD27723 value; STATUS reports valid
//! ambient-light and proximity samples with no interrupt pending; the channel
//! and proximity data registers read zero: dark, nothing in front of the
//! finder.
use crate::i2c::Registers;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proximity {
    registers: Vec<u8>,
}
impl Default for Proximity {
    fn default() -> Self {
        Self {
            registers: vec![0; 0x20],
        }
    }
}
impl Proximity {
    const ID: u8 = 0x12;
    const STATUS: u8 = 0x13;
}
impl Registers for Proximity {
    fn name(&self) -> &'static str {
        "tmd2772-proximity"
    }
    fn select(&self, command: u8) -> u8 {
        command & 0x1f
    }
    fn write(&mut self, register: u8, value: u8) -> Result<(), String> {
        if let Some(slot) = self.registers.get_mut(register as usize) {
            *slot = value;
        }
        Ok(())
    }
    fn read(&mut self, register: u8) -> Result<u8, String> {
        Ok(match register {
            Self::ID => 0x39,
            // AVALID | PVALID; no threshold interrupt pending.
            Self::STATUS => 0x03,
            // CDATA/IRDATA/PDATA: dark, nothing near the finder.
            0x14..=0x19 => 0,
            r => self.registers.get(r as usize).copied().unwrap_or(0),
        })
    }
    fn save(&self) -> Value {
        serde_json::to_value(self).unwrap()
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        let restored: Self = serde_json::from_value(state.clone()).map_err(|e| e.to_string())?;
        if restored.registers.len() != 0x20 {
            return Err("invalid TMD2772 checkpoint".into());
        }
        *self = restored;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eye_sensor_reports_identity_and_an_empty_finder() {
        let mut sensor = Proximity::default();
        // Command bytes are 0xA0 | register (auto-increment type).
        assert_eq!(sensor.select(0xa0 | 0x12), 0x12);
        assert_eq!(sensor.read(0x12).unwrap(), 0x39);
        assert_eq!(sensor.read(0x13).unwrap() & 0x22, 0x02, "PVALID set, PINT clear");
        assert_eq!([sensor.read(0x18).unwrap(), sensor.read(0x19).unwrap()], [0, 0]);
        // Configuration writes are retained and read back.
        sensor.write(0x0e, 0x0c).unwrap();
        assert_eq!(sensor.read(0x0e).unwrap(), 0x0c);
    }
}
