//! LSM6DS-class accelerometer and gyroscope on I2C (ST DS12140 register map), the part at
//! seven-bit address 0x6a that the X-H2S level reader drives. Written configuration is kept and
//! reads back; the WHO_AM_I register (0x0f) reads 0x6c, the value upstream's SPI LSM6DS model
//! uses; CTRL3_C (0x12) resets to 0x04 (register auto-increment) and a write with SW_RESET (bit 0)
//! restores every register to its reset value, bit 0 clearing itself in the same step (the
//! internal reset latency is not modeled). Gyroscope and temperature outputs (0x20..=0x27) read
//! zero. Acceleration outputs (0x28..=0x2d) read the declared raw sample, zero unless the board
//! declares one: a declared external input, not a measurement and not tuned to any firmware poll.
//! The identity, reset values and output layout are datasheet knowledge of the family; that this
//! X-H2S part is an LSM6DS variant is inferred from the register traffic (the stock level reader
//! resets through register 0x12 and reads six bytes from 0x28), not from a part marking.
use crate::i2c::Registers;
use fujimod_emulator_devices::checkpoint::Checkpoint;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const WHO_AM_I: u8 = 0x0f;
const CTRL3_C: u8 = 0x12;
const SW_RESET: u8 = 1;
const OUTPUTS: std::ops::RangeInclusive<u8> = 0x1e..=0x2d;
const ACCELERATION: u8 = 0x28;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lsm6ds {
    registers: Vec<u8>,
    /// Declared raw acceleration samples (x, y, z), little-endian in registers 0x28..=0x2d.
    acceleration: [i16; 3],
}
impl Lsm6ds {
    pub fn new(acceleration: [i16; 3]) -> Self {
        let mut part = Self {
            registers: vec![0; 256],
            acceleration,
        };
        part.reset();
        part
    }
    fn reset(&mut self) {
        self.registers.fill(0);
        self.registers[WHO_AM_I as usize] = 0x6c;
        self.registers[CTRL3_C as usize] = 0x04;
    }
}
impl Default for Lsm6ds {
    fn default() -> Self {
        Self::new([0; 3])
    }
}
impl Registers for Lsm6ds {
    fn name(&self) -> &'static str {
        "lsm6ds"
    }
    fn write(&mut self, register: u8, value: u8) -> Result<(), String> {
        match register {
            // Identity and the output block are read only.
            WHO_AM_I => {}
            r if OUTPUTS.contains(&r) => {}
            CTRL3_C if value & SW_RESET != 0 => self.reset(),
            r => self.registers[r as usize] = value,
        }
        Ok(())
    }
    fn read(&mut self, register: u8) -> Result<u8, String> {
        Ok(match register {
            r if (ACCELERATION..ACCELERATION + 6).contains(&r) => {
                let index = (r - ACCELERATION) as usize;
                self.acceleration[index / 2].to_le_bytes()[index % 2]
            }
            // STATUS_REG and the temperature and gyroscope outputs: nothing pending, zero.
            r if OUTPUTS.contains(&r) => 0,
            r => self.registers[r as usize],
        })
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), self)
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let state: Self = saved.decode(self.name())?;
        if state.registers.len() != 256 {
            return Err("invalid LSM6DS checkpoint".into());
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        *self = saved.decode(self.name())?;
        Ok(())
    }
    fn save(&self) -> Value {
        serde_json::to_value(self).unwrap()
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        let restored: Self = serde_json::from_value(state.clone()).map_err(|e| e.to_string())?;
        if restored.registers.len() != 256 {
            return Err("invalid LSM6DS checkpoint".into());
        }
        *self = restored;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i2c::{Target, Targets};
    use fujimod_emulator_devices::parts::i2c::Bus;

    fn bus(acceleration: [i16; 3]) -> Targets {
        Targets::new(vec![Target {
            address: 0x6a,
            registers: Box::new(Lsm6ds::new(acceleration)),
        }])
        .unwrap()
    }
    fn read(bus: &mut Targets, register: u8, count: usize) -> Vec<u8> {
        assert!(bus.start(0x6a, false));
        bus.write(register).unwrap();
        assert!(bus.start(0x6a, true));
        let bytes = (0..count).map(|_| bus.read().unwrap()).collect();
        bus.stop();
        bytes
    }

    /// The transaction the stock level reader performs: reset through CTRL3_C, poll it until
    /// SW_RESET clears, configure, then read six bytes from 0x28 behind a register pointer.
    #[test]
    fn reset_poll_configure_and_output_block_read() {
        let mut bus = bus([0x0123, -2, 0x4000]);
        assert_eq!(read(&mut bus, WHO_AM_I, 1), [0x6c]);
        assert_eq!(read(&mut bus, CTRL3_C, 1), [0x04]);
        assert!(bus.start(0x6a, false));
        bus.write(CTRL3_C).unwrap();
        bus.write(1).unwrap();
        bus.stop();
        assert_eq!(read(&mut bus, CTRL3_C, 1)[0] & SW_RESET, 0);
        assert!(bus.start(0x6a, false));
        bus.write(0x10).unwrap();
        bus.write(0x60).unwrap();
        bus.stop();
        assert_eq!(read(&mut bus, 0x10, 1), [0x60]);
        assert_eq!(
            read(&mut bus, ACCELERATION, 6),
            [0x23, 0x01, 0xfe, 0xff, 0x00, 0x40]
        );
        // A reset restores configuration; identity and outputs are read only.
        assert!(bus.start(0x6a, false));
        bus.write(CTRL3_C).unwrap();
        bus.write(1).unwrap();
        bus.write(WHO_AM_I).unwrap();
        bus.stop();
        assert_eq!(read(&mut bus, 0x10, 1), [0]);
        assert_eq!(read(&mut bus, WHO_AM_I, 1), [0x6c]);
    }

    #[test]
    fn undeclared_acceleration_reads_zero_and_other_addresses_are_not_acknowledged() {
        let mut bus = bus([0; 3]);
        assert_eq!(read(&mut bus, ACCELERATION, 6), [0; 6]);
        assert!(!bus.start(0x6b, false));
    }
}
