//! Checked board wiring for I2C controllers and their declared targets.
use fujimod_emulator_ip::socionext::i2c::{Controller, Options};
use fujimod_emulator_parts_io::i2c::{Registers, RetainedRegisters, Target, Targets};
use fujimod_emulator_parts_io::{
    lsm6ds_i2c::Lsm6ds, power_controller::FuelGauge, tmd2772::Proximity,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wiring {
    interrupt: u32,
    byte_ns: u64,
    #[serde(default)]
    targets: Vec<Connection>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Connection {
    address: u8,
    part: Part,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Part {
    FuelGauge,
    RetainedRegisters,
    Tmd2772Proximity,
    /// LSM6DS-class accelerometer; `acceleration` is a declared raw sample (x, y, z), default zero.
    Lsm6ds {
        #[serde(default)]
        acceleration: [i16; 3],
    },
    /// The GFX100RF touch panel controller, with no finger on it.
    TouchPanel,
}

/// Touch panel controller as the GFX100RF 1.13 touch task 0x14fd51c reads it (I2C client 11,
/// address 0x55). The part itself is not identified; only what the task uses is answered:
///  - writes (the configuration blocks of 0x14fcec0) are accepted and dropped;
///  - a read that starts at 0x10 gives two bytes, the finger count and a status whose bits
///    0x18 mean an error (0 here);
///  - a read that starts at 0x11 gives one 7-byte finger record: byte 0 bit 7 = touching, low
///    nibble = finger number (1), bytes 1..2 X, bytes 3..4 Y, little endian (0x14fd1d0). The
///    task reads it twice, once per finger; the second record is empty here;
///  - a read that starts at 0x12 gives one byte the task's watchdog (0x14fd4d8) expects to
///    change between polls: a counter.
/// No finger is modeled: every record reads not touching.
#[derive(Default)]
struct TouchPanel {
    start: std::cell::Cell<u8>,
    second: std::cell::Cell<bool>,
    counter: u8,
}
impl Registers for TouchPanel {
    fn name(&self) -> &'static str {
        "touch-panel"
    }
    fn select(&self, byte: u8) -> u8 {
        if byte == 0x10 {
            self.second.set(false);
        }
        self.start.set(byte);
        byte
    }
    fn write(&mut self, _: u8, _: u8) -> Result<(), String> {
        Ok(())
    }
    fn read(&mut self, register: u8) -> Result<u8, String> {
        let (touching, x, y) = (false, 0u16, 0u16);
        let offset = register.wrapping_sub(self.start.get());
        Ok(match self.start.get() {
            0x10 => [1, 0].get(offset as usize).copied().unwrap_or(0),
            0x11 => {
                let second = self.second.get();
                if offset == 6 {
                    self.second.set(true);
                }
                let record = [
                    u8::from(touching) << 7 | 1,
                    x as u8,
                    (x >> 8) as u8,
                    y as u8,
                    (y >> 8) as u8,
                    0,
                    0,
                ];
                if second {
                    0
                } else {
                    record.get(offset as usize).copied().unwrap_or(0)
                }
            }
            0x12 => {
                self.counter = self.counter.wrapping_add(1);
                self.counter
            }
            _ => 0,
        })
    }
    fujimod_emulator_devices::checkpoint_fields!(start: std::cell::Cell<u8>, second: std::cell::Cell<bool>, counter: u8);
    fn save(&self) -> serde_json::Value {
        serde_json::json!([self.start.get(), self.second.get(), self.counter])
    }
    fn restore(&mut self, state: &serde_json::Value) -> Result<(), String> {
        let (start, second, counter): (u8, bool, u8) = serde_json::from_value(state.clone())
            .map_err(|e| format!("touch panel checkpoint: {e}"))?;
        self.start.set(start);
        self.second.set(second);
        self.counter = counter;
        Ok(())
    }
}

pub fn build(base: u64, size: u64, options: &toml::Table) -> Result<Controller<Targets>, String> {
    if size != 0x1000 {
        return Err("I2C controller window must be 0x1000 bytes".into());
    }
    let wiring: Wiring = toml::Value::Table(options.clone())
        .try_into()
        .map_err(|e| format!("I2C wiring: {e}"))?;
    let targets = wiring
        .targets
        .into_iter()
        .map(|connection| Target {
            address: connection.address,
            registers: match connection.part {
                Part::FuelGauge => Box::new(FuelGauge::new()) as Box<dyn Registers>,
                Part::RetainedRegisters => Box::new(RetainedRegisters::default()),
                Part::Tmd2772Proximity => Box::new(Proximity::default()),
                Part::Lsm6ds { acceleration } => Box::new(Lsm6ds::new(acceleration)),
                Part::TouchPanel => Box::new(TouchPanel::default()),
            },
        })
        .collect();
    Controller::new(
        Options {
            base,
            irq: wiring.interrupt,
            byte_ns: wiring.byte_ns,
        },
        Targets::new(targets)?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(text: &str) -> toml::Table {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn declared_targets_take_their_part_kinds_and_unknown_fields_refuse() {
        let wiring = "interrupt = 289\nbyte_ns = 1000000\n\
            [[targets]]\naddress = 0x36\npart = { kind = 'fuel-gauge' }\n\
            [[targets]]\naddress = 0x39\npart = { kind = 'tmd2772-proximity' }\n\
            [[targets]]\naddress = 0x66\npart = { kind = 'retained-registers' }\n\
            [[targets]]\naddress = 0x6a\npart = { kind = 'lsm6ds', acceleration = [1, 2, 3] }\n";
        assert!(build(0xff925000, 0x1000, &options(wiring)).is_ok());
        assert!(build(0xff925000, 0x2000, &options(wiring)).is_err());
        assert!(
            build(
                0xff926000,
                0x1000,
                &options("interrupt = 290\nbyte_ns = 1000000")
            )
            .is_ok()
        );
        assert!(build(0xff926000, 0x1000, &options("interrupt = 290")).is_err());
        assert!(
            build(
                0xff926000,
                0x1000,
                &options("interrupt = 290\nbyte_ns = 1\n[[targets]]\naddress = 0x6a\npart = { kind = 'accelerometer' }")
            )
            .is_err()
        );
        assert!(
            build(
                0xff926000,
                0x1000,
                &options("interrupt = 290\nbyte_ns = 1\n[[targets]]\naddress = 0x66\npart = { kind = 'retained-registers' }\n[[targets]]\naddress = 0x66\npart = { kind = 'retained-registers' }")
            )
            .is_err()
        );
    }
}
