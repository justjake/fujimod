//! ICM-42688-compatible SPI register subset (TDK InvenSense ICM-42688-P,
//! DS-000347), as the X-S20 3.30 companion drives it on SSP7.
//!
//! Identified from the companion's own command sequence (X-S20 3.30 draft run,
//! `--watch-writes` on its transmit buffer): in bank 0 it writes DRIVE_CONFIG
//! (13h), GYRO_CONFIG0..ACCEL_CONFIG1 (4Fh..53h); it selects bank 1 through
//! REG_BANK_SEL (76h) and writes GYRO_CONFIG_STATIC2 (0Bh) and
//! GYRO_CONFIG_STATIC10 (13h); it selects bank 4 and clears OFFSET_USER0..8
//! (77h..7Fh); it returns to bank 0, writes PWR_MGMT0 (4Eh) and then burst-reads
//! from TEMP_DATA1 (1Dh). Every address and bank matches the ICM-42688 map; the
//! LSM6DSO map has none of them.
//!
//! Outputs are big-endian (INTF_CONFIG0 reset: SENSOR_DATA_ENDIAN = 1).
//! TEMP_DATA is an explicit scenario input. Accelerometer and gyroscope
//! outputs answer only when the scenario declares a constant motion input; a
//! sensor that PWR_MGMT0 leaves off reads -32768, the part's "no data" value.
//! Reset values come from the datasheet register map, not from a camera.
//! Unlisted registers, banks 2 and 3, the FIFO, interrupts and the APEX
//! features are refused. Soft reset (DEVICE_CONFIG bit 0) commits with its
//! write; its 1 ms latency is not modeled. The exact part (ICM-42688-P or a
//! register-compatible sibling) is not confirmed on hardware.
use super::serial::{Endpoint, Frame};
use fujimod_emulator_devices::checkpoint::Checkpoint;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const REG_BANK_SEL: u8 = 0x76;
const WHO_AM_I: u8 = 0x75;
const DEVICE_CONFIG: u8 = 0x11;
const PWR_MGMT0: u8 = 0x4e;
/// TEMP_DATA1 .. GYRO_DATA_Z0: temperature, accelerometer X/Y/Z, gyroscope X/Y/Z.
const OUTPUTS: std::ops::RangeInclusive<u8> = 0x1d..=0x2a;
/// Banks the model holds, each 128 registers.
const BANKS: [u8; 3] = [0, 1, 4];

/// A constant physical motion input: raw two's-complement output words for
/// the gyroscope (GYRO_DATA_X/Y/Z) and accelerometer (ACCEL_DATA_X/Y/Z).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Motion {
    pub angular_rate: [i16; 3],
    pub acceleration: [i16; 3],
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    bank: u8,
    /// Registers of banks 0, 1 and 4, 128 each, in that order.
    registers: Vec<u8>,
    address: Option<u8>,
    read: bool,
    transfers: u64,
}

pub struct Registers {
    state: State,
    temperature: i16,
    motion: Option<Motion>,
}

/// Index of a bank in `State::registers`.
fn slot(bank: u8) -> Option<usize> {
    BANKS.iter().position(|&b| b == bank)
}

/// Registers writable in each bank. REG_BANK_SEL is in every bank.
fn writable(bank: u8, address: u8) -> bool {
    address == REG_BANK_SEL
        || match bank {
            0 => matches!(address, DEVICE_CONFIG | 0x13 | 0x4e..=0x53),
            1 => matches!(address, 0x0b | 0x13),
            4 => matches!(address, 0x77..=0x7f),
            _ => false,
        }
}

impl Registers {
    /// `temperature` is the raw two's-complement TEMP_DATA word.
    pub fn new(temperature: i16, motion: Option<Motion>) -> Self {
        let mut s = Self {
            state: State::default(),
            temperature,
            motion,
        };
        s.reset();
        s
    }

    fn reset(&mut self) {
        let mut registers = vec![0u8; 128 * BANKS.len()];
        let mut set = |bank: u8, address: u8, value: u8| {
            registers[slot(bank).unwrap() * 128 + address as usize] = value;
        };
        // Bank 0 reset values (DS-000347 register map).
        set(0, 0x13, 0x05); // DRIVE_CONFIG
        set(0, 0x4f, 0x06); // GYRO_CONFIG0
        set(0, 0x50, 0x06); // ACCEL_CONFIG0
        set(0, 0x51, 0x16); // GYRO_CONFIG1
        set(0, 0x52, 0x11); // GYRO_ACCEL_CONFIG0
        set(0, 0x53, 0x0d); // ACCEL_CONFIG1
        // Bank 1.
        set(1, 0x0b, 0xa0); // GYRO_CONFIG_STATIC2
        set(1, 0x13, 0x11); // GYRO_CONFIG_STATIC10
        self.state.registers = registers;
        self.state.bank = 0;
    }

    fn register(&self, address: u8) -> u8 {
        let base = slot(self.state.bank).expect("bank is validated") * 128;
        self.state.registers[base + address as usize]
    }

    fn readable(&self, address: u8) -> bool {
        writable(self.state.bank, address)
            || self.state.bank == 0
                && (address == WHO_AM_I
                    || matches!(address, 0x1d | 0x1e)
                    || self.motion.is_some() && OUTPUTS.contains(&address))
    }

    fn output_word(&self, index: usize) -> i16 {
        // index 0: temperature, 1..=3: accelerometer, 4..=6: gyroscope.
        if index == 0 {
            return self.temperature;
        }
        let motion = self
            .motion
            .expect("motion outputs are readable only with motion");
        let power = self.register(PWR_MGMT0);
        if index <= 3 {
            if power & 0x03 == 0 {
                return i16::MIN;
            }
            motion.acceleration[index - 1]
        } else {
            if power & 0x0c == 0 {
                return i16::MIN;
            }
            motion.angular_rate[index - 4]
        }
    }

    fn value(&self, address: u8) -> u8 {
        if self.state.bank == 0 {
            if address == WHO_AM_I {
                return 0x47;
            }
            if OUTPUTS.contains(&address) {
                let offset = (address - 0x1d) as usize;
                let word = self.output_word(offset / 2);
                // Big-endian: the even offset is the high byte.
                return if offset & 1 == 0 {
                    (word >> 8) as u8
                } else {
                    word as u8
                };
            }
        }
        if address == REG_BANK_SEL {
            return self.state.bank;
        }
        self.register(address)
    }

    fn check_state(state: &State) -> Result<(), String> {
        if slot(state.bank).is_none()
            || state.registers.len() != 128 * BANKS.len()
            || state.address.is_some_and(|a| a > 0x7f)
        {
            return Err("invalid ICM-42688 state".into());
        }
        Ok(())
    }
}

impl Endpoint for Registers {
    fn name(&self) -> &'static str {
        "icm42688-configuration-registers"
    }

    fn validate(&self, frame: Frame) -> Result<(), String> {
        if frame.bits != 8 || !matches!(frame.mode, 0 | 3) || frame.word > 255 {
            return Err("ICM-42688 SPI requires 8-bit mode 0 or 3".into());
        }
        let word = frame.word as u8;
        let Some(address) = self.state.address else {
            let address = word & 0x7f;
            let read = word & 0x80 != 0;
            let known = if read {
                self.readable(address)
            } else {
                writable(self.state.bank, address)
            };
            if !known {
                return Err(format!(
                    "ICM-42688 bank {} register {address:#04x} {} is unmodeled",
                    self.state.bank,
                    if read { "read" } else { "write" }
                ));
            }
            return Ok(());
        };
        // A data byte: auto-increment may have moved the address on.
        if self.state.read {
            if !self.readable(address) {
                return Err(format!(
                    "ICM-42688 bank {} register {address:#04x} read is unmodeled",
                    self.state.bank
                ));
            }
        } else {
            if !writable(self.state.bank, address) {
                return Err(format!(
                    "ICM-42688 bank {} register {address:#04x} write is unmodeled",
                    self.state.bank
                ));
            }
            if address == REG_BANK_SEL && slot(word & 7).is_none() {
                return Err(format!("ICM-42688 bank {} is unmodeled", word & 7));
            }
            if address == REG_BANK_SEL && word & !7 != 0 {
                return Err("ICM-42688 REG_BANK_SEL reserved bits are set".into());
            }
            if self.state.bank == 0 && address == DEVICE_CONFIG && word & !1 != 0 {
                return Err("ICM-42688 DEVICE_CONFIG SPI mode bit is unmodeled".into());
            }
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
            self.state.read = word & 0x80 != 0;
            self.state.address = Some(word & 0x7f);
            return 0;
        };
        let response = if self.state.read {
            self.value(address)
        } else {
            if address == REG_BANK_SEL {
                self.state.bank = word & 7;
            } else if self.state.bank == 0 && address == DEVICE_CONFIG && word & 1 != 0 {
                self.reset();
            } else {
                let base = slot(self.state.bank).expect("bank is validated") * 128;
                self.state.registers[base + address as usize] = word;
            }
            0
        };
        self.state.address = Some(address.wrapping_add(1) & 0x7f);
        response as u16
    }

    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), &self.state)
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let state: State = saved.decode(self.name())?;
        Self::check_state(&state)
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        let state: State = saved.decode(self.name())?;
        Self::check_state(&state)?;
        self.state = state;
        Ok(())
    }
    fn save(&self) -> Value {
        json!({"temperature": self.temperature, "motion": self.motion, "state": self.state})
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        if v["temperature"] != json!(self.temperature) {
            return Err("ICM-42688 temperature input differs".into());
        }
        if v["motion"] != json!(self.motion) {
            return Err("ICM-42688 motion input differs".into());
        }
        let state: State = serde_json::from_value(v["state"].clone()).map_err(|e| e.to_string())?;
        Self::check_state(&state)?;
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(word: u16) -> Frame {
        Frame {
            bits: 8,
            mode: 3,
            word,
        }
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
    const STILL: Motion = Motion {
        angular_rate: [0; 3],
        acceleration: [0, 0, 0x4000],
    };

    /// The X-S20 3.30 companion's start-up sequence, one register per frame.
    fn xs20_setup(part: &mut Registers) {
        for pair in [
            [0x13, 0x03],
            [0x4f, 0x83],
            [0x50, 0x63],
            [0x51, 0x00],
            [0x52, 0x00],
            [0x53, 0x00],
            [0x76, 0x01],
            [0x0b, 0xa1],
            [0x13, 0x11],
            [0x76, 0x04],
            [0x77, 0],
            [0x78, 0],
            [0x79, 0],
            [0x7a, 0],
            [0x7b, 0],
            [0x7c, 0],
            [0x7d, 0],
            [0x7e, 0],
            [0x7f, 0],
            [0x76, 0x00],
            [0x4e, 0x0f],
        ] {
            exchange(part, &pair).unwrap();
        }
    }

    #[test]
    fn the_xs20_setup_sequence_is_accepted_and_reads_back() {
        let mut part = Registers::new(0, Some(STILL));
        xs20_setup(&mut part);
        assert_eq!(exchange(&mut part, &[0xcf, 0]).unwrap(), [0, 0x83]);
        assert_eq!(exchange(&mut part, &[0xce, 0]).unwrap(), [0, 0x0f]);
        assert_eq!(exchange(&mut part, &[0xf5, 0]).unwrap(), [0, 0x47]);
        exchange(&mut part, &[0x76, 1]).unwrap();
        assert_eq!(exchange(&mut part, &[0x8b, 0]).unwrap(), [0, 0xa1]);
    }

    #[test]
    fn the_output_burst_is_big_endian_temperature_then_accel_then_gyro() {
        let motion = Motion {
            angular_rate: [1, -2, 0x0304],
            acceleration: [0x0506, 0, 0x4000],
        };
        let mut part = Registers::new(0x0102, Some(motion));
        xs20_setup(&mut part);
        let mut burst = vec![0x9d];
        burst.extend([0; 14]);
        let out = exchange(&mut part, &burst).unwrap();
        assert_eq!(out, [0, 1, 2, 5, 6, 0, 0, 0x40, 0, 0, 1, 0xff, 0xfe, 3, 4]);
    }

    #[test]
    fn sensors_left_off_read_no_data() {
        let mut part = Registers::new(0, Some(STILL));
        let out = exchange(&mut part, &[0x9f, 0, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(out, [0, 0x80, 0, 0x80, 0, 0x80, 0]);
    }

    #[test]
    fn unknown_registers_and_banks_are_refused() {
        let mut part = Registers::new(0, None);
        assert!(
            exchange(&mut part, &[0x40, 0])
                .unwrap_err()
                .contains("0x40")
        );
        assert!(
            exchange(&mut part, &[0x76, 2])
                .unwrap_err()
                .contains("bank 2")
        );
        // Without motion only the temperature answers.
        assert!(exchange(&mut part, &[0x9f, 0]).is_err());
        assert_eq!(exchange(&mut part, &[0x9d, 0, 0]).unwrap(), [0, 0, 0]);
        // Bank 4 offsets are not writable in bank 0.
        assert!(exchange(&mut part, &[0x77, 0]).is_err());
    }

    #[test]
    fn soft_reset_restores_reset_values_and_bank_zero() {
        let mut part = Registers::new(0, None);
        exchange(&mut part, &[0x4f, 0x83]).unwrap();
        exchange(&mut part, &[0x11, 0x01]).unwrap();
        assert_eq!(exchange(&mut part, &[0xcf, 0]).unwrap(), [0, 0x06]);
    }

    #[test]
    fn save_and_restore_require_the_same_inputs() {
        let mut part = Registers::new(5, Some(STILL));
        xs20_setup(&mut part);
        let saved = part.save();
        assert!(Registers::new(5, Some(STILL)).restore(&saved).is_ok());
        assert!(Registers::new(6, Some(STILL)).restore(&saved).is_err());
        assert!(Registers::new(5, None).restore(&saved).is_err());
    }
}
