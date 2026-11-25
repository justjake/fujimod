//! Checked board wiring for I2C controllers and their declared targets.
use fujimod_emulator_ip::socionext::i2c::{Controller, Options};
use fujimod_emulator_parts_io::i2c::{Registers, RetainedRegisters, Target, Targets};
use fujimod_emulator_parts_io::{
    lsm6ds_i2c::Lsm6ds, power_controller::FuelGauge, tmd2772::Proximity,
};
use serde::Deserialize;
use std::collections::BTreeSet;

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
    Tmd2772Proximity {
        interrupt: Option<u32>,
    },
    /// LSM6DS-class accelerometer; `acceleration` is a declared raw sample (x, y, z), default zero.
    Lsm6ds {
        #[serde(default)]
        acceleration: [i16; 3],
    },
}

pub fn build(base: u64, size: u64, options: &toml::Table) -> Result<Controller<Targets>, String> {
    if size != 0x1000 {
        return Err("I2C controller window must be 0x1000 bytes".into());
    }
    let wiring: Wiring = toml::Value::Table(options.clone())
        .try_into()
        .map_err(|e| format!("I2C wiring: {e}"))?;
    let mut lines = BTreeSet::from([wiring.interrupt]);
    for connection in &wiring.targets {
        if let Part::Tmd2772Proximity {
            interrupt: Some(line),
        } = connection.part
        {
            if !(32..1020).contains(&line) || !lines.insert(line) {
                return Err("I2C target needs a distinct shared interrupt line".into());
            }
        }
    }
    let targets = wiring
        .targets
        .into_iter()
        .map(|connection| Target {
            address: connection.address,
            registers: match connection.part {
                Part::FuelGauge => Box::new(FuelGauge::new()) as Box<dyn Registers>,
                Part::RetainedRegisters => Box::new(RetainedRegisters::default()),
                Part::Tmd2772Proximity { interrupt: None } => Box::new(Proximity::default()),
                Part::Tmd2772Proximity {
                    interrupt: Some(line),
                } => Box::new(Proximity::with_interrupt(line)),
                Part::Lsm6ds { acceleration } => Box::new(Lsm6ds::new(acceleration)),
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
    fn target_interrupt_wiring_admits_only_the_declared_proximity_sample() {
        use fujimod_emulator_devices::Device;

        let wiring = "interrupt = 289\nbyte_ns = 1\n\
            [[targets]]\naddress = 0x39\npart = { kind = 'tmd2772-proximity', interrupt = 122 }";
        let controller = build(0xff925000, 0x1000, &options(wiring)).unwrap();
        let input = controller.proximity_input().unwrap();
        assert!(input.admit_sample(0x39, 1023).is_ok());
        assert!(input.admit_sample(0x36, 1023).is_err());
        assert!(input.admit_sample(0x39, 1024).is_err());
        assert!(build(0xff925000, 0x1000, &options(&wiring.replace("122", "289"))).is_err());
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
