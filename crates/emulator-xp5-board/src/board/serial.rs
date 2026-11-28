//! Checked board wiring for serial controllers and their attached parts.
use fujimod_emulator_devices::{checkpoint::Checkpoint, xp5::pins::SharedPins};
use fujimod_emulator_parts_io::{
    power_controller::PowerController,
    power_controller_programmer::{Config as ProgrammerConfig, Programmer as PowerProgrammer},
    serial::Endpoint,
    serial_calibration::CalibrationRegisters,
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
#[serde(deny_unknown_fields)]
pub(super) struct PowerControllerProgrammer {
    mode: Select,
    flash_bytes: usize,
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
        #[serde(default)]
        programmer: Option<PowerControllerProgrammer>,
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
    /// With a front dial in bits 7..4 of the same register, its count. Turns
    /// arrive as edges: a multiple of [`FRONT_EDGES`] is front detents, any
    /// other count is rear detents (see `ladders::CountingDial`).
    front: Option<i32>,
}
/// Edges per front-dial detent on a register shared with the rear dial.
pub const FRONT_EDGES: i32 = 64;
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
            let mut value = (steps as u16) & 0xf;
            if let Some(front) = &mut self.front {
                let steps = (*front).clamp(-8, 7);
                *front -= steps;
                value |= ((steps as u16) & 0xf) << 4;
            }
            self.inner.set_register(self.register, value);
        }
        self.inner.transfer(frame, now_ns)
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        if self.count != 0 || self.front.is_some_and(|front| front != 0) {
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
        if let Some(front) = &mut self.front {
            *front = 0;
        }
        self.inner.restore_checkpoint(saved)
    }
    fn save(&self) -> serde_json::Value {
        self.inner.save()
    }
    fn restore(&mut self, state: &serde_json::Value) -> Result<(), String> {
        self.count = 0;
        if let Some(front) = &mut self.front {
            *front = 0;
        }
        self.inner.restore(state)
    }
    fn set_input_bits(&mut self, register: u8, mask: u16, value: u16) -> Result<(), String> {
        self.inner.set_input_bits(register, mask, value)
    }
    fn input_bits(&self, register: u8) -> Result<u16, String> {
        self.inner.input_bits(register)
    }
    fn admit_steps(&self, steps: i32) -> Result<(), String> {
        let (count, steps) = match self.front {
            Some(front) if steps != 0 && steps % FRONT_EDGES == 0 => (front, steps / FRONT_EDGES),
            _ => (self.count, steps),
        };
        match count.checked_add(steps) {
            Some(count) if count.abs() <= COUNT_LIMIT => Ok(()),
            _ => Err("command-dial detents exceed their bound".into()),
        }
    }
    fn count_steps(&mut self, steps: i32) -> Result<(), String> {
        self.admit_steps(steps)?;
        match &mut self.front {
            Some(front) if steps != 0 && steps % FRONT_EDGES == 0 => *front += steps / FRONT_EDGES,
            _ => self.count += steps,
        }
        Ok(())
    }
}

/// Power controller with an external mode-select GPIO pin that switches the
/// shared SPI wire between normal register packets and three-byte flash programming.
struct ModeSwitchedPowerController {
    normal: Box<dyn Endpoint>,
    programmer: PowerProgrammer,
    pins: SharedPins,
    mode: Select,
    was_programming: bool,
}
impl ModeSwitchedPowerController {
    fn is_programming(&self) -> bool {
        let pins = self.pins.lock().expect("GPIO pins poisoned");
        self.mode.active(&pins)
    }
}
impl Endpoint for ModeSwitchedPowerController {
    fn name(&self) -> &'static str {
        self.normal.name()
    }
    fn validate(&self, frame: fujimod_emulator_parts_io::serial::Frame) -> Result<(), String> {
        self.normal.validate(frame)
    }
    fn reset_transaction(&mut self) {
        self.normal.reset_transaction();
        self.programmer.reset_transaction();
    }
    fn transfer(&mut self, frame: fujimod_emulator_parts_io::serial::Frame, now_ns: u64) -> u16 {
        let programming = self.is_programming();
        if programming != self.was_programming {
            self.was_programming = programming;
            if programming {
                self.programmer.reset_session();
            } else {
                self.normal.reset_transaction();
            }
        }
        if programming {
            self.programmer.transfer(frame, now_ns)
        } else {
            self.normal.transfer(frame, now_ns)
        }
    }
    fn set_input_bits(&mut self, register: u8, mask: u16, value: u16) -> Result<(), String> {
        self.normal.set_input_bits(register, mask, value)
    }
    fn input_bits(&self, register: u8) -> Result<u16, String> {
        self.normal.input_bits(register)
    }
    fn admit_steps(&self, steps: i32) -> Result<(), String> {
        self.normal.admit_steps(steps)
    }
    fn count_steps(&mut self, steps: i32) -> Result<(), String> {
        self.normal.count_steps(steps)
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Ok(Checkpoint::encode(
            "power-controller-with-programmer",
            &(self.mode, self.was_programming),
        )?
        .with_children(vec![
            self.normal.checkpoint()?,
            self.programmer.checkpoint()?,
        ]))
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let (mode, _was_programming): (Select, bool) =
            saved.decode("power-controller-with-programmer")?;
        if mode != self.mode || saved.children().len() != 2 {
            return Err("invalid power-controller programmer mode checkpoint".into());
        }
        self.normal.validate_checkpoint(&saved.children()[0])?;
        self.programmer.validate_checkpoint(&saved.children()[1])
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (_mode, was_programming): (Select, bool) =
            saved.decode("power-controller-with-programmer")?;
        self.normal.restore_checkpoint(&saved.children()[0])?;
        self.programmer.restore_checkpoint(&saved.children()[1])?;
        self.was_programming = was_programming;
        Ok(())
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!({
            "normal": self.normal.save(),
            "mode": self.mode,
            "was_programming": self.was_programming,
            "programmer": self.programmer.diagnostic_state(),
        })
    }
    fn restore(&mut self, _state: &serde_json::Value) -> Result<(), String> {
        Err("power-controller programmer binary flash requires a typed checkpoint".into())
    }
}

pub fn build(
    base: u64,
    pins: SharedPins,
    options: &toml::Table,
    counting_dial: Option<(u8, bool)>,
) -> Result<Spi, String> {
    let wiring: Wiring = toml::Value::Table(options.clone())
        .try_into()
        .map_err(|e| format!("SPI wiring: {e}"))?;
    Spi::new(
        base,
        pins.clone(),
        wiring.source_hz,
        attachments(wiring.endpoints, counting_dial, Some(pins))?,
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
fn endpoint(
    part: Part,
    counting_dial: Option<(u8, bool)>,
    pins: Option<SharedPins>,
) -> Result<Box<dyn Endpoint>, String> {
    Ok(match part {
        Part::PowerController {
            rtc_seconds,
            rtc_valid,
            registers,
            programmer,
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
            let normal: Box<dyn Endpoint> = match counting_dial {
                Some((register, front)) => Box::new(CountingDial {
                    inner: part,
                    register,
                    count: 0,
                    front: front.then_some(0),
                }),
                None => Box::new(part),
            };
            match programmer {
                Some(cfg) => {
                    if cfg.mode.port >= 20 || cfg.mode.pin >= 16 {
                        return Err(
                            "power-controller programmer mode pin outside GPIO range".into()
                        );
                    }
                    let pins = pins.ok_or("power-controller programmer requires GPIO pins")?;
                    let programmer = PowerProgrammer::new(ProgrammerConfig {
                        flash_bytes: cfg.flash_bytes,
                    })?;
                    Box::new(ModeSwitchedPowerController {
                        normal,
                        programmer,
                        pins,
                        mode: cfg.mode,
                        was_programming: false,
                    })
                }
                None => normal,
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
    counting_dial: Option<(u8, bool)>,
    pins: Option<SharedPins>,
) -> Result<Vec<Attachment>, String> {
    connections
        .into_iter()
        .map(|connection| {
            Ok(Attachment::new(
                connection.select,
                endpoint(connection.part, counting_dial, pins.clone())?,
            ))
        })
        .collect()
}
pub(super) fn dedicated_wire(
    part: Part,
) -> Result<Box<dyn fujimod_emulator_parts_io::serial::Wire>, String> {
    Ok(Box::new(fujimod_emulator_parts_io::serial::Dedicated(
        endpoint(part, None, None)?,
    )))
}
pub(super) fn gpio_wire(
    pins: SharedPins,
    connections: Vec<Connection>,
) -> Result<Box<dyn fujimod_emulator_parts_io::serial::Wire>, String> {
    Ok(Box::new(fujimod_emulator_xp5_io::spi::GpioWire::new(
        pins.clone(),
        attachments(connections, None, Some(pins))?,
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
                programmer: None,
            },
            Some((0x36, false)),
            None,
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
        let mut part = endpoint(burst.part, None, None).unwrap();
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
                    programmer: None,
                },
                None,
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

    #[test]
    fn mode_gpio_switches_power_controller_between_registers_and_programming_mode() {
        use fujimod_emulator_devices::xp5::pins::Pins;
        let pins = Pins::shared();
        let make = || {
            endpoint(
                Part::PowerController {
                    registers: [("0".to_string(), 0x8200)].into_iter().collect(),
                    rtc_seconds: 0,
                    rtc_valid: true,
                    programmer: Some(PowerControllerProgrammer {
                        mode: Select {
                            port: 8,
                            pin: 1,
                            active_high: true,
                        },
                        flash_bytes: 0x1_0000,
                    }),
                },
                Some((0x36, false)),
                Some(pins.clone()),
            )
            .unwrap()
        };
        let xfer3 = |ep: &mut dyn Endpoint, b0: u8, b1: u8, b2: u8| -> (u8, u8) {
            let mut f = |w: u8| {
                ep.transfer(
                    Frame {
                        bits: 8,
                        mode: 3,
                        word: w.into(),
                    },
                    0,
                ) as u8
            };
            let _ = f(b0);
            (f(b1), f(b2))
        };

        let mut ep = make();
        assert_eq!(ep.name(), "power-controller");
        // Normal register mode while GPIO 8.1 is low.
        assert_eq!(read(ep.as_mut(), 0x00), 0x8200);
        ep.set_input_bits(0x12, 0x30, 0x10).unwrap();
        assert_eq!(ep.input_bits(0x12).unwrap(), 0x10);
        assert_eq!(read(ep.as_mut(), 0x12), 0x10);
        ep.admit_steps(2).unwrap();
        ep.count_steps(2).unwrap();
        assert_eq!(read(ep.as_mut(), 0x36), 2);

        // Drive GPIO 8.1 high as output -> enters programming mode.
        pins.lock().unwrap().ports[8].registers[0] |= 1 << 1;
        assert_eq!(xfer3(ep.as_mut(), 0xab, 0x63, 0x59), (0, 0));
        assert_eq!(xfer3(ep.as_mut(), 0xa4, 0x55, 0x0d), (0, 0));
        assert_eq!(xfer3(ep.as_mut(), 0x01, 0, 0), (0x06, 0xe0));
        assert_eq!(xfer3(ep.as_mut(), 0x1a, 0, 8), (0, 0));
        assert_eq!(xfer3(ep.as_mut(), 0x01, 0, 0), (0x06, 0xc0));
        assert_eq!(xfer3(ep.as_mut(), 0x1a, 0, 0), (0, 0));
        for cmd in [0x96, 0x98, 0x9a, 0x9e, 0x9c] {
            xfer3(ep.as_mut(), cmd, 0xff, 0xff);
        }
        assert_eq!(xfer3(ep.as_mut(), 0x91, 0, 0), (0, 0));
        xfer3(ep.as_mut(), 0xce, 0, 1);
        xfer3(ep.as_mut(), 0xc0, 0, 1);
        xfer3(ep.as_mut(), 0xc6, 0, 0);
        xfer3(ep.as_mut(), 0xc8, 0, 0);
        xfer3(ep.as_mut(), 0xc2, 0, 5);
        assert_eq!(xfer3(ep.as_mut(), 0xc5, 0, 0), (0, 0));
        xfer3(ep.as_mut(), 0xce, 0, 0);

        xfer3(ep.as_mut(), 0xce, 0, 1);
        xfer3(ep.as_mut(), 0xc0, 0, 5);
        xfer3(ep.as_mut(), 0xc6, 0, 0);
        xfer3(ep.as_mut(), 0xc8, 0, 0);
        xfer3(ep.as_mut(), 0xc2, 0, 4);
        xfer3(ep.as_mut(), 0xd2, 0x78, 0x56);
        xfer3(ep.as_mut(), 0xca, 0x34, 0x12);
        assert_eq!(xfer3(ep.as_mut(), 0xc5, 0, 0), (0, 0));
        xfer3(ep.as_mut(), 0xce, 0, 0);

        // Checkpoint round-trip preserves both normal registers and programmed flash.
        let cp = ep.checkpoint().unwrap();
        let saved = ep.save();
        assert_eq!(
            saved["programmer"]["flash"],
            serde_json::json!({ "bytes": 0x1_0000 })
        );
        assert!(ep.restore(&saved).is_err());

        let mut restored = make();
        restored.restore_checkpoint(&cp).unwrap();
        xfer3(restored.as_mut(), 0xc0, 0, 3);
        xfer3(restored.as_mut(), 0xc6, 0, 0);
        xfer3(restored.as_mut(), 0xc8, 0, 0);
        xfer3(restored.as_mut(), 0xc2, 0, 2);
        xfer3(restored.as_mut(), 0xe6, 0, 3);
        xfer3(restored.as_mut(), 0xe4, 0x78, 0x56);
        xfer3(restored.as_mut(), 0xe2, 0x34, 0x12);
        assert_eq!(xfer3(restored.as_mut(), 0xe7, 0, 0), (0, 3));

        // Lower GPIO 8.1 -> resumes normal power-controller protocol with registers intact.
        pins.lock().unwrap().ports[8].registers[0] &= !(1 << 1);
        assert_eq!(read(restored.as_mut(), 0x00), 0x8200);
        assert_eq!(read(restored.as_mut(), 0x12), 0x10);
    }
}
