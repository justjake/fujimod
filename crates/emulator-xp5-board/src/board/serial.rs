//! Checked board wiring for serial controllers and their attached parts.
use fujimod_emulator_devices::xp5::pins::SharedPins;
use fujimod_emulator_parts_io::{
    power_controller::PowerController, serial::Endpoint, serial_calibration::CalibrationRegisters,
};
use fujimod_emulator_xp5_io::spi::{Attachment, Select, Spi};
use serde::Deserialize;

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
pub(super) enum Part {
    PowerController {
        rtc_seconds: u32,
        rtc_valid: bool,
        /// Bring-up: initial register overrides (address -> value), for
        /// example register 0 bit 7, which the X-T5 stage1 polls at boot.
        #[serde(default)]
        registers: std::collections::BTreeMap<String, u16>,
    },
    SerialCalibrationRegisters {
        miso_high: bool,
    },
    SerialCommandRegisters(fujimod_emulator_parts_io::serial_command_registers::Wiring),
    BisConfigurationRegisters {
        revision: [u8; 2],
        samples: [u16; 2],
        idle_high: bool,
        #[serde(default)]
        block: Option<[u8; 6]>,
    },
    Lsm6dsConfigurationRegisters {
        temperature: i16,
        #[serde(default)]
        motion: Option<fujimod_emulator_parts_io::lsm6ds::Motion>,
    },
    FramedCommandPeer {
        initial_frame: u32,
    },
    /// Bring-up: an unidentified chip. Accepts every frame, records the
    /// last words written and answers with a fixed MISO level.
    GenericSink {
        miso_high: bool,
    },
}
/// Bring-up: endpoint for bring-up of bodies whose serial parts are not yet identified.
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
    fujimod_emulator_devices::checkpoint_fields!(miso_high: bool, transfers: u64, last: Vec<u16>);
    fn save(&self) -> serde_json::Value {
        serde_json::json!({"miso_high": self.miso_high, "transfers": self.transfers, "last": self.last})
    }
    fn restore(&mut self, state: &serde_json::Value) -> Result<(), String> {
        self.transfers = state["transfers"].as_u64().unwrap_or(0);
        self.last = state["last"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_u64().map(|v| v as u16))
                    .collect()
            })
            .unwrap_or_default();
        Ok(())
    }
}

/// A power controller that counts command-dial detents in one register. The
/// count loads when the guest starts to read the register, and that read
/// consumes the count it returns: bits 2..0 are the magnitude and bit 3 the
/// sign, so one read reports at most seven detents either way.
struct CountingDial {
    inner: PowerController,
    register: u8,
    count: i32,
}
/// Bound of detents the host may queue ahead of the guest's reads.
const COUNT_LIMIT: i32 = 1 << 16;
impl Endpoint for CountingDial {
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
        let command = frame.word as u8;
        if self.inner.transfer_position() == 0
            && command & 0x80 == 0
            && command & 0x7f == self.register
        {
            let steps = self.count.clamp(-8, 7);
            self.count -= steps;
            self.inner.set_register(self.register, (steps as u16) & 0xf);
        }
        self.inner.transfer(frame, now_ns)
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        if self.count != 0 {
            return Err("command-dial detents are waiting for the guest; run before saving".into());
        }
        self.inner.checkpoint()
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.inner.validate_checkpoint(saved)
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.count = 0;
        self.inner.restore_checkpoint(saved)
    }
    fn save(&self) -> serde_json::Value {
        self.inner.save()
    }
    fn restore(&mut self, state: &serde_json::Value) -> Result<(), String> {
        self.count = 0;
        self.inner.restore(state)
    }
    fn set_input_bits(&mut self, register: u8, mask: u16, value: u16) -> Result<(), String> {
        self.inner.set_input_bits(register, mask, value)
    }
    fn input_bits(&self, register: u8) -> Result<u16, String> {
        self.inner.input_bits(register)
    }
    fn admit_steps(&self, steps: i32) -> Result<(), String> {
        match self.count.checked_add(steps) {
            Some(count) if count.abs() <= COUNT_LIMIT => Ok(()),
            _ => Err("command-dial detents exceed their bound".into()),
        }
    }
    fn count_steps(&mut self, steps: i32) -> Result<(), String> {
        self.admit_steps(steps)?;
        self.count += steps;
        Ok(())
    }
}

pub fn build(
    base: u64,
    pins: SharedPins,
    options: &toml::Table,
    counting_dial: Option<u8>,
) -> Result<Spi, String> {
    let wiring: Wiring = toml::Value::Table(options.clone())
        .try_into()
        .map_err(|e| format!("SPI wiring: {e}"))?;
    Spi::new(
        base,
        pins,
        wiring.source_hz,
        attachments(wiring.endpoints, counting_dial)?,
    )
}
/// Parse `XT5_POWER_REGISTERS="REGISTER=VALUE,..."` (numbers decimal or `0x` hexadecimal):
/// power-controller registers a run sets at power-on in place of the part's defaults. A host input
/// like `XT5_ADC`, kept outside the manifest. On the X-T5, register 0x32 is the battery
/// temperature in 0.1 K (2982 = 25.0 C); it reads 0 with no battery data, and the battery driver
/// then reports external power.
fn parse_power_registers(spec: &str) -> Result<Vec<(u8, u16)>, String> {
    fn number(text: &str) -> Option<u32> {
        let text = text.trim();
        match text.strip_prefix("0x") {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => text.parse().ok(),
        }
    }
    spec.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            let (register, value) = item
                .split_once('=')
                .ok_or("XT5_POWER_REGISTERS needs REGISTER=VALUE[,...]")?;
            let register = number(register)
                .filter(|register| *register < 0x80)
                .ok_or_else(|| format!("XT5_POWER_REGISTERS: invalid register in {item}"))?;
            let value = number(value)
                .and_then(|value| u16::try_from(value).ok())
                .ok_or_else(|| format!("XT5_POWER_REGISTERS: invalid value in {item}"))?;
            Ok((register as u8, value))
        })
        .collect()
}
fn endpoint(part: Part, counting_dial: Option<u8>) -> Result<Box<dyn Endpoint>, String> {
    Ok(match part {
        Part::PowerController {
            rtc_seconds,
            rtc_valid,
            registers,
        } => {
            let mut part = PowerController::new();
            part.configure_rtc(rtc_seconds, rtc_valid);
            // Each address appears once and names a plain storage register; the
            // RTC latch is refused.
            let mut seen = [false; 0x80];
            for (address, value) in registers {
                let address = u8::from_str_radix(address.trim_start_matches("0x"), 16)
                    .map_err(|e| format!("power-controller register {address}: {e}"))?;
                if address >= 0x80 || address == 0x27 {
                    return Err(format!(
                        "power controller register {address:#x} is not a declarable storage register"
                    ));
                }
                if std::mem::replace(&mut seen[address as usize], true) {
                    return Err(format!(
                        "power controller register {address:#x} is declared twice"
                    ));
                }
                part.set_register(address, value);
            }
            if let Ok(spec) = std::env::var("XT5_POWER_REGISTERS") {
                for (address, value) in parse_power_registers(&spec)? {
                    part.set_register(address, value);
                }
            }
            match counting_dial {
                Some(register) => Box::new(CountingDial {
                    inner: part,
                    register,
                    count: 0,
                }),
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
            samples,
            idle_high,
            block,
        } => Box::new(
            fujimod_emulator_parts_io::bis_configuration::Registers::with_block(
                revision, samples, idle_high, block,
            )?,
        ),
        Part::Lsm6dsConfigurationRegisters {
            temperature,
            motion,
        } => Box::new(fujimod_emulator_parts_io::lsm6ds::Registers::new(
            temperature,
            motion,
        )),
        Part::FramedCommandPeer { initial_frame } => Box::new(
            fujimod_emulator_parts_io::framed_command_peer::Peer::new(initial_frame)?,
        ),
        Part::GenericSink { miso_high } => Box::new(GenericSink {
            miso_high,
            transfers: 0,
            last: vec![],
        }),
    })
}
fn attachments(
    connections: Vec<Connection>,
    counting_dial: Option<u8>,
) -> Result<Vec<Attachment>, String> {
    connections
        .into_iter()
        .map(|connection| {
            Ok(Attachment::new(
                connection.select,
                endpoint(connection.part, counting_dial)?,
            ))
        })
        .collect()
}
pub(super) fn dedicated_wire(
    part: Part,
) -> Result<Box<dyn fujimod_emulator_parts_io::serial::Wire>, String> {
    Ok(Box::new(fujimod_emulator_parts_io::serial::Dedicated(
        endpoint(part, None)?,
    )))
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

    #[test]
    fn power_register_inputs_parse_decimal_and_hexadecimal() {
        assert_eq!(
            parse_power_registers("0x32=2982, 3=600").unwrap(),
            [(0x32, 2982), (3, 600)]
        );
        assert_eq!(parse_power_registers("").unwrap(), []);
        assert!(
            parse_power_registers("0x32")
                .unwrap_err()
                .contains("REGISTER=VALUE")
        );
        assert!(
            parse_power_registers("0x80=1")
                .unwrap_err()
                .contains("invalid register")
        );
        assert!(
            parse_power_registers("3=0x10000")
                .unwrap_err()
                .contains("invalid value")
        );
    }
    use fujimod_emulator_parts_io::serial::Frame;

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

    #[test]
    fn the_dial_count_reaches_the_power_controller_and_clears_on_read() {
        let mut endpoint = endpoint(
            Part::PowerController {
                registers: Default::default(),
                rtc_seconds: 0,
                rtc_valid: true,
            },
            Some(0x36),
        )
        .unwrap();
        assert_eq!(read(endpoint.as_mut(), 0x36), 0);
        endpoint.count_steps(-3).unwrap();
        // -3 as a signed nibble, then cleared by that read.
        assert_eq!(read(endpoint.as_mut(), 0x36), 0xd);
        assert_eq!(read(endpoint.as_mut(), 0x36), 0);
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
            let frame = Frame {
                bits: 8,
                mode: 0,
                word,
            };
            part.validate(frame).unwrap();
            part.transfer(frame, 0);
        }
        assert_eq!(
            part.save()["registers"]["registers"]["10"],
            serde_json::json!([0x11, 0x22])
        );
        assert!(
            connection("{ kind = 'serial-command-registers', mode = 0, miso_high = false }")
                .is_err()
        );
        assert!(
            connection(
                "{ kind = 'serial-command-registers', bits = 8, mode = 0, read_flag = 1, miso_high = false, idle = 1 }"
            )
            .is_err()
        );
    }

    #[test]
    fn declared_registers_override_reset_values() {
        let power_controller = |registers: &str| {
            endpoint(
                Part::PowerController {
                    rtc_seconds: 0,
                    rtc_valid: false,
                    registers: toml::from_str(registers).unwrap(),
                },
                None,
            )
        };
        let mut default_part = power_controller("").unwrap();
        assert_eq!(read(default_part.as_mut(), 0), 0x200, "stock default");
        assert_eq!(read(default_part.as_mut(), 0) & 0x8000, 0);

        let mut ready_part = power_controller("\"0\" = 0x8200\n\"0x10\" = 2").unwrap();
        assert_eq!(read(ready_part.as_mut(), 0), 0x8200, "bit-15 candidate");
        assert_eq!(read(ready_part.as_mut(), 0x10), 2);
        for registers in ["\"27\" = 0x200", "\"80\" = 0", "\"10\" = 1\n\"0x10\" = 2"] {
            assert!(power_controller(registers).is_err(), "{registers}");
        }
    }
}
