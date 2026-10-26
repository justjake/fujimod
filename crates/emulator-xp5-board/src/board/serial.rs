//! Checked board wiring for serial controllers and their attached parts.
use fujimod_emulator_parts_io::{power_controller::PowerController, serial::Endpoint, serial_calibration::CalibrationRegisters};
use fujimod_emulator_devices::xp5::pins::SharedPins;
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
enum Part {
    PowerController { rtc_seconds: u32, rtc_valid: bool },
    SerialCalibrationRegisters { miso_high: bool },
    BisConfigurationRegisters { revision: [u8; 2], idle_high: bool },
    Lsm6dsConfigurationRegisters,
}
pub fn build(base: u64, pins: SharedPins, options: &toml::Table) -> Result<Spi, String> {
    let wiring: Wiring = toml::Value::Table(options.clone())
        .try_into()
        .map_err(|e| format!("SPI wiring: {e}"))?;
    Spi::new(base, pins, wiring.source_hz, attachments(wiring.endpoints)?)
}
fn attachments(connections: Vec<Connection>) -> Result<Vec<Attachment>, String> {
    connections
        .into_iter()
        .map(|connection| {
            let endpoint: Box<dyn Endpoint> = match connection.part {
                Part::PowerController {
                    rtc_seconds,
                    rtc_valid,
                } => {
                    let mut part = PowerController::new();
                    part.configure_rtc(rtc_seconds, rtc_valid);
                    Box::new(part)
                }
                Part::SerialCalibrationRegisters { miso_high } => {
                    Box::new(CalibrationRegisters::new(miso_high))
                }
                Part::BisConfigurationRegisters {
                    revision,
                    idle_high,
                } => Box::new(fujimod_emulator_parts_io::bis_configuration::Registers::new(
                    revision, idle_high,
                )?),
                Part::Lsm6dsConfigurationRegisters => {
                    Box::new(fujimod_emulator_parts_io::lsm6ds::Registers::default())
                }
            };
            Ok(Attachment::new(connection.select, endpoint))
        })
        .collect()
}
pub(super) fn gpio_wire(
    pins: SharedPins,
    connections: Vec<Connection>,
) -> Result<Box<dyn fujimod_emulator_parts_io::serial::Wire>, String> {
    Ok(Box::new(fujimod_emulator_xp5_io::spi::GpioWire::new(
        pins,
        attachments(connections)?,
    )?))
}
