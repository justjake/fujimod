//! Checked board wiring for I2C controllers and their declared targets.
use fujimod_emulator_ip::socionext::i2c::{Controller, Options};
use fujimod_emulator_parts_io::i2c::{Registers, RetainedRegisters, Target, Targets};
use fujimod_emulator_parts_io::{power_controller::FuelGauge, tmd2772::Proximity};
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
            [[targets]]\naddress = 0x66\npart = { kind = 'retained-registers' }\n";
        assert!(build(0xff925000, 0x1000, &options(wiring)).is_ok());
        assert!(build(0xff925000, 0x2000, &options(wiring)).is_err());
        assert!(build(0xff926000, 0x1000, &options("interrupt = 290\nbyte_ns = 1000000")).is_ok());
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
