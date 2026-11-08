//! Checked board wiring for serial controllers and their attached parts.
use fujimod_emulator_parts_io::{power_controller::PowerController, serial::Endpoint, serial_calibration::CalibrationRegisters};
use fujimod_emulator_devices::xp5::pins::SharedPins;
use fujimod_emulator_xp5_io::spi::{Attachment, Select, Spi};
use fujimod_emulator_devices::parts::controls::Controls;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wiring {
    source_hz: u64,
    endpoints: Vec<Connection>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Connection {
    select: Select,
    part: Part,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Part {
    PowerController {
        rtc_seconds: u32,
        rtc_valid: bool,
        /// xt5-bringup: initial register overrides (address -> value), for
        /// example register 0 bit 7, which the X-T5 stage1 polls at boot.
        #[serde(default)]
        registers: std::collections::BTreeMap<String, u16>,
    },
    SerialCalibrationRegisters { miso_high: bool },
    SerialCommandRegisters(fujimod_emulator_parts_io::serial_command_registers::Wiring),
    BisConfigurationRegisters { revision: [u8; 2], idle_high: bool },
    Lsm6dsConfigurationRegisters,
    /// xt5-bringup: an unidentified chip. Accepts every frame, records the
    /// last words written and answers with a fixed MISO level.
    GenericSink { miso_high: bool },
}
/// xt5-bringup: endpoint for bring-up of bodies whose serial parts are not yet identified.
struct GenericSink {
    miso_high: bool,
    transfers: u64,
    last: Vec<u16>,
}
impl Endpoint for GenericSink {
    fn name(&self) -> &'static str {
        "generic-sink"
    }
    fn validate(&self, _: fujimod_emulator_parts_io::serial::Frame) -> Result<(), String> {
        Ok(())
    }
    fn reset_transaction(&mut self) {}
    fn transfer(&mut self, frame: fujimod_emulator_parts_io::serial::Frame, _: u64) -> u16 {
        self.transfers += 1;
        self.last.push(frame.word);
        if self.last.len() > 16 {
            self.last.remove(0);
        }
        if self.miso_high { u16::MAX } else { 0 }
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!({"miso_high": self.miso_high, "transfers": self.transfers, "last": self.last})
    }
    fn restore(&mut self, state: &serde_json::Value) -> Result<(), String> {
        self.transfers = state["transfers"].as_u64().unwrap_or(0);
        self.last = state["last"].as_array().map(|a| a.iter().filter_map(|v| v.as_u64().map(|v| v as u16)).collect()).unwrap_or_default();
        Ok(())
    }
}
/// A power controller whose key-ladder registers are driven by the body's
/// controls. Their value is refreshed before every transfer, so the guest reads
/// the held key's ladder value, or the declared released value, and a guest
/// write to one of these registers does not stick.
struct LadderDriven {
    inner: PowerController,
    controls: Arc<dyn Controls>,
    registers: Vec<u8>,
}
impl LadderDriven {
    fn refresh(&mut self) {
        for &register in &self.registers {
            if let Some(value) = self.controls.register_sample(register) {
                self.inner.set_register(register, value);
            }
        }
    }
}
impl Endpoint for LadderDriven {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn validate(&self, frame: fujimod_emulator_parts_io::serial::Frame) -> Result<(), String> {
        self.inner.validate(frame)
    }
    fn reset_transaction(&mut self) {
        self.inner.reset_transaction()
    }
    fn transfer(&mut self, frame: fujimod_emulator_parts_io::serial::Frame, now_ns: u64) -> u16 {
        self.refresh();
        self.inner.transfer(frame, now_ns)
    }
    fn save(&self) -> serde_json::Value {
        self.inner.save()
    }
    fn restore(&mut self, state: &serde_json::Value) -> Result<(), String> {
        self.inner.restore(state)?;
        self.refresh();
        Ok(())
    }
}

pub fn build(
    base: u64,
    pins: SharedPins,
    options: &toml::Table,
    controls: Arc<dyn Controls>,
) -> Result<Spi, String> {
    let wiring: Wiring = toml::Value::Table(options.clone())
        .try_into()
        .map_err(|e| format!("SPI wiring: {e}"))?;
    Spi::new(
        base,
        pins,
        wiring.source_hz,
        attachments(wiring.endpoints, Some(controls))?,
    )
}
fn endpoint(part: Part, controls: Option<Arc<dyn Controls>>) -> Result<Box<dyn Endpoint>, String> {
    Ok(match part {
        Part::PowerController {
            rtc_seconds,
            rtc_valid,
            registers,
        } => {
            let mut part = PowerController::new();
            part.configure_rtc(rtc_seconds, rtc_valid);
            for (address, value) in registers {
                let address = u8::from_str_radix(address.trim_start_matches("0x"), 16)
                    .map_err(|e| format!("power-controller register {address}: {e}"))?;
                part.set_register(address, value);
            }
            match controls
                .map(|controls| (controls.register_ladders(), controls))
                .filter(|(registers, _)| !registers.is_empty())
            {
                Some((registers, controls)) => {
                    let mut driven = LadderDriven {
                        inner: part,
                        controls,
                        registers,
                    };
                    driven.refresh();
                    Box::new(driven)
                }
                None => Box::new(part),
            }
        }
        Part::SerialCalibrationRegisters { miso_high } => {
            Box::new(CalibrationRegisters::new(miso_high))
        }
        Part::SerialCommandRegisters(wiring) => Box::new(
            fujimod_emulator_parts_io::serial_command_registers::CommandRegisters::new(wiring)?,
        ),
        Part::BisConfigurationRegisters {
            revision,
            idle_high,
        } => Box::new(fujimod_emulator_parts_io::bis_configuration::Registers::new(
            revision, idle_high,
        )?),
        Part::Lsm6dsConfigurationRegisters => {
            Box::new(fujimod_emulator_parts_io::lsm6ds::Registers::default())
        }
        Part::GenericSink { miso_high } => Box::new(GenericSink { miso_high, transfers: 0, last: vec![] }),
    })
}
fn attachments(
    connections: Vec<Connection>,
    controls: Option<Arc<dyn Controls>>,
) -> Result<Vec<Attachment>, String> {
    connections
        .into_iter()
        .map(|connection| {
            Ok(Attachment::new(
                connection.select,
                endpoint(connection.part, controls.clone())?,
            ))
        })
        .collect()
}
pub(super) fn gpio_wire(
    pins: SharedPins,
    connections: Vec<Connection>,
) -> Result<Box<dyn fujimod_emulator_parts_io::serial::Wire>, String> {
    Ok(Box::new(fujimod_emulator_xp5_io::spi::GpioWire::new(
        pins,
        attachments(connections, None)?,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_parts_controls::ladder_keys::{Ladder, LadderKeys, Options, Rung, Wire};
    use fujimod_emulator_parts_controls::resistor_keys::Key;
    use fujimod_emulator_parts_io::serial::Frame;
    use std::time::Duration;

    // Synthetic fixture: bands and samples are test inputs, not camera values.
    fn keys() -> Arc<LadderKeys> {
        let rung = |key, sample| Rung {
            key,
            band_low: sample,
            band_high: sample,
            sample,
            basis: "test fixture".into(),
        };
        Arc::new(
            LadderKeys::new(Options {
                ladders: vec![Ladder {
                    wire: Wire::PowerControllerRegister {
                        register: 0x34,
                        released: 1023,
                    },
                    levels: 1024,
                    rungs: vec![rung(Key::Menu, 77)],
                }],
            })
            .unwrap(),
        )
    }

    fn read(endpoint: &mut dyn Endpoint, register: u8) -> u16 {
        let request = [register, 0, 0, !register];
        let reply: Vec<u8> = request
            .into_iter()
            .map(|byte| {
                endpoint.transfer(
                    Frame {
                        bits: 8,
                        mode: 3,
                        word: byte.into(),
                    },
                    0,
                ) as u8
            })
            .collect();
        u16::from_be_bytes([reply[1], reply[2]])
    }

    fn attach(controls: Arc<dyn Controls>) -> Box<dyn Endpoint> {
        endpoint(
            Part::PowerController {
                registers: Default::default(),
                rtc_seconds: 0,
                rtc_valid: true,
            },
            Some(controls),
        )
        .unwrap()
    }

    #[test]
    fn declared_register_ladders_reach_the_power_controller_registers() {
        let keys = keys();
        let mut endpoint = attach(keys.clone());
        assert_eq!(read(endpoint.as_mut(), 0x34), 1023);
        keys.enqueue(Duration::from_nanos(5), Key::Menu, true).unwrap();
        assert_eq!(read(endpoint.as_mut(), 0x34), 1023, "edge is not due");
        keys.advance(5);
        assert_eq!(read(endpoint.as_mut(), 0x34), 77);
        // Registers the controls do not drive keep the controller's own value.
        assert_eq!(read(endpoint.as_mut(), 0x35), 0);
        keys.enqueue(Duration::from_nanos(9), Key::Menu, false).unwrap();
        keys.advance(9);
        assert_eq!(read(endpoint.as_mut(), 0x34), 1023);
    }

    #[test]
    fn command_register_endpoints_take_their_declared_wire_format() {
        let connection = |part: &str| {
            toml::from_str::<Connection>(&format!(
                "select = {{ port = 12, pin = 8, active_high = false }}\npart = {part}"
            ))
        };
        let burst = connection(
            "{ kind = 'serial-command-registers', bits = 8, mode = 0, read_flag = 1, miso_high = false }",
        )
        .unwrap();
        let mut part = endpoint(burst.part, None).unwrap();
        for word in [0x0a, 0x11, 0x22] {
            let frame = Frame { bits: 8, mode: 0, word };
            part.validate(frame).unwrap();
            part.transfer(frame, 0);
        }
        assert_eq!(part.save()["registers"]["registers"]["10"], serde_json::json!([0x11, 0x22]));
        assert!(connection("{ kind = 'serial-command-registers', mode = 0, miso_high = false }").is_err());
        assert!(
            connection(
                "{ kind = 'serial-command-registers', bits = 8, mode = 0, read_flag = 1, miso_high = false, idle = 1 }"
            )
            .is_err()
        );
    }

    #[test]
    fn controls_without_register_ladders_leave_the_controller_unwrapped() {
        let controls: Arc<dyn Controls> =
            Arc::new(fujimod_emulator_parts_controls::absent::AbsentControls);
        let mut endpoint = attach(controls);
        assert_eq!(read(endpoint.as_mut(), 0x34), 0);
    }
}
