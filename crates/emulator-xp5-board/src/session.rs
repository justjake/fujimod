//! External XP5 controls and GPIO inputs of an opened XP5 session.
pub use crate::keys::Key;
use fujimod_emulator_devices::xp5::pins::SharedPins;
use fujimod_emulator_ip::usb::dwc3::{Dwc3, Reply};
use fujimod_emulator_session::Session;

/// External XP5 controls retained after device construction, for the session
/// `boot::open` returned with them. Their modeled state is checkpointed by the
/// devices that consume them.
pub struct Inputs {
    pins: SharedPins,
    panel: Option<PanelInputs>,
    gpio_devices: Vec<usize>,
    usb: Option<UsbWiring>,
    /// The HDMI transmitter, which owns the sink's hot-plug line.
    hdmi: Option<usize>,
}

/// The devices one USB cable reaches: the DWC3 core (data lines), the
/// connection-status word (0xff90e300 bit 0 on the X-T5) and the SPI
/// controllers that may carry the power controller (supply detection in
/// register 0x12 bits 5:4).
pub(crate) struct UsbWiring {
    pub controller: usize,
    pub status: Option<usize>,
    pub serial: Vec<usize>,
}

/// Power-controller register 0x12 bits 5:4 read 0x10 while a data host's
/// supply is present (X-T5 4.31 0x13d5914: 0x10 is the host port case; 0x20
/// and 0x30 are charger types; 0 is no supply).
const VBUS_REGISTER: u8 = 0x12;
const VBUS_MASK: u16 = 0x30;
const VBUS_HOST: u16 = 0x10;

/// How key names and dial positions reach a session's switch panel.
pub struct PanelInputs {
    pub options: crate::ladders::Options,
    pub names: crate::panel::Names,
    pub ticks_per_second: u64,
}
impl PanelInputs {
    fn at(&self, at_ns: u64) -> u64 {
        (u128::from(at_ns) * u128::from(self.ticks_per_second) / 1_000_000_000) as u64
    }
    fn control(&self, key: Key) -> Result<&str, String> {
        self.names
            .keys
            .get(&key)
            .map(|(id, _)| id.as_str())
            .ok_or_else(|| format!("{key:?} is not a control of this body"))
    }
    /// The panel input for one key edge; a detent's release has none.
    fn input(&self, key: Key, pressed: bool) -> Result<Option<Scheduled>, String> {
        let rotary = |names: &Option<(String, String)>, steps| {
            names
                .as_ref()
                .map(|(id, _)| Scheduled::Rotate {
                    id: id.clone(),
                    steps,
                })
                .ok_or_else(|| format!("{key:?} is not a control of this body"))
        };
        let shutter = || {
            self.names
                .shutter
                .as_ref()
                .map(|((id, _), _)| id.clone())
                .ok_or_else(|| "this body has no shutter button".to_string())
        };
        Ok(Some(match (key, pressed) {
            (Key::RearPlus | Key::RearMinus | Key::FrontPlus | Key::FrontMinus, false) => {
                return Ok(None);
            }
            (Key::RearPlus, true) => rotary(&self.names.rear_dial, 1)?,
            (Key::RearMinus, true) => rotary(&self.names.rear_dial, -1)?,
            (Key::FrontPlus, true) => rotary(&self.names.front_dial, 1)?,
            (Key::FrontMinus, true) => rotary(&self.names.front_dial, -1)?,
            (Key::ShutterHalf, true) => Scheduled::Press {
                id: shutter()?,
                stage: Some("half".into()),
            },
            (Key::ShutterFull, true) => Scheduled::Press {
                id: shutter()?,
                stage: Some("full".into()),
            },
            (Key::ShutterHalf, false) => Scheduled::Release { id: shutter()? },
            (Key::ShutterFull, false) => Scheduled::Press {
                id: shutter()?,
                stage: Some("half".into()),
            },
            (key, true) => Scheduled::Press {
                id: self.control(key)?.into(),
                stage: None,
            },
            (key, false) => Scheduled::Release {
                id: self.control(key)?.into(),
            },
        }))
    }
    fn position(&self, dial: &str, position: &str) -> Result<Scheduled, String> {
        let wanted = dial.trim();
        if let Some(((id, _), _)) = self.names.eye_sensor.as_ref().filter(|((id, _), _)| {
            wanted.eq_ignore_ascii_case(id) || wanted.eq_ignore_ascii_case("eye")
        }) {
            return Ok(Scheduled::Position {
                id: id.clone(),
                position: position.trim().to_ascii_lowercase(),
            });
        }
        let (model, label) = if let Some(found) = self.options.dials.iter().find(|d| {
            d.name.eq_ignore_ascii_case(wanted)
                || d.aliases.iter().any(|a| a.eq_ignore_ascii_case(wanted))
        }) {
            let label = found
                .positions
                .iter()
                .find(|p| {
                    let name = position.trim();
                    std::iter::once(&p.label).chain(&p.aliases).any(|l| l.eq_ignore_ascii_case(name))
                        || matches!((p.label.parse::<f64>(), name.parse::<f64>()), (Ok(a), Ok(b)) if (a - b).abs() < 1e-9)
                })
                .ok_or_else(|| format!("{} has no position {position}", found.name))?;
            (found.name.clone(), label.label.clone())
        } else if let Some(found) = self
            .options
            .switches
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(wanted))
        {
            let label = [&found.set, &found.clear]
                .into_iter()
                .find(|l| l.eq_ignore_ascii_case(position.trim()))
                .ok_or_else(|| format!("{} has no position {position}", found.name))?;
            (found.name.clone(), label.clone())
        } else {
            return Err(format!("this body has no dial or switch {dial}"));
        };
        let (id, _) = self
            .names
            .positions
            .get(&model)
            .ok_or_else(|| format!("{model} has no catalog control"))?;
        Ok(Scheduled::Position {
            id: id.clone(),
            position: label,
        })
    }
}
use fujimod_emulator_session::contact_driver::Scheduled;

impl Inputs {
    /// Send key edges and position changes to the session's switch panel.
    pub fn use_panel(&mut self, panel: PanelInputs) {
        self.panel = Some(panel);
    }
    pub(crate) fn new(
        pins: SharedPins,
        gpio_devices: Vec<usize>,
        usb: Option<UsbWiring>,
        hdmi: Option<usize>,
    ) -> Self {
        Self {
            pins,
            panel: None,
            gpio_devices,
            usb,
            hdmi,
        }
    }

    /// Whether the board has an HDMI transmitter.
    pub fn has_hdmi_transmitter(&self) -> bool {
        self.hdmi.is_some()
    }

    /// Plug an HDMI sink into the body's HDMI connector, or unplug it, at the current device
    /// time. Refused on a board without a transmitter or a body without a connector.
    pub fn hdmi_cable(&self, session: &mut Session, connected: bool) -> Result<(), String> {
        use fujimod_emulator_xp5_io::hdmi_transmitter::HdmiTransmitter;
        let id = self
            .hdmi
            .ok_or("this XP5 board declares no HDMI transmitter")?;
        session
            .board
            .with_device_inputs::<HdmiTransmitter, _>(id, |transmitter, now| {
                transmitter.set_connected(now, connected)
            })
    }

    fn panel(&self) -> Result<&PanelInputs, String> {
        self.panel
            .as_ref()
            .ok_or_else(|| "this XP5 body has no physical controls; inputs are refused".into())
    }

    fn usb(&self) -> Result<&UsbWiring, String> {
        self.usb
            .as_ref()
            .ok_or_else(|| "this XP5 board declares no USB device controller".into())
    }

    /// Plug or unplug a USB cable from a data host: the connection-status
    /// word, the power controller's supply bits and the DWC3 data lines change
    /// together, at the current device time.
    pub fn usb_cable(&self, session: &mut Session, connected: bool) -> Result<(), String> {
        use fujimod_emulator_xp5_io::{connection_status::ConnectionStatus, spi::Spi};
        let usb = self.usb()?;
        if let Some(status) = usb.status {
            session
                .board
                .with_device_inputs::<ConnectionStatus, _>(status, |s, _| {
                    s.set_connected(connected);
                    Ok(())
                })?;
        }
        let mut power = None;
        for &id in &usb.serial {
            if session
                .board
                .with_device::<Spi, _>(id, |spi| spi.has_part("power-controller"))?
                && power.replace(id).is_some()
            {
                return Err("more than one XP5 power controller is attached".into());
            }
        }
        let power = power.ok_or("the XP5 board has no power controller for the USB supply")?;
        session
            .board
            .with_device_inputs::<Spi, _>(power, |spi, _| {
                spi.set_part_input(
                    "power-controller",
                    VBUS_REGISTER,
                    VBUS_MASK,
                    if connected { VBUS_HOST } else { 0 },
                )
            })?;
        self.usb_link(session, connected)
    }

    /// Only the DWC3 data lines, without the supply or status inputs.
    pub fn usb_link(&self, session: &mut Session, connected: bool) -> Result<(), String> {
        session
            .board
            .with_device_inputs::<Dwc3, _>(self.usb()?.controller, |usb, now| {
                usb.link(now, connected)
            })
    }

    pub fn usb_bus_reset(&self, session: &mut Session) -> Result<(), String> {
        session
            .board
            .with_device_inputs::<Dwc3, _>(self.usb()?.controller, |usb, now| usb.bus_reset(now))
    }

    /// Queue one host transaction on a DWC3 physical endpoint; returns its id.
    pub fn usb_transaction(
        &self,
        session: &mut Session,
        endpoint: usize,
        setup: bool,
        out: Option<Vec<u8>>,
    ) -> Result<u64, String> {
        session
            .board
            .with_device_inputs::<Dwc3, _>(self.usb()?.controller, |usb, now| {
                usb.transaction(now, endpoint, setup, out)
            })
    }

    pub fn usb_reply(&self, session: &Session, id: u64) -> Result<Option<Reply>, String> {
        session
            .board
            .with_device::<Dwc3, _>(self.usb()?.controller, |usb| usb.reply(id).cloned())
    }

    pub fn usb_consume(&self, session: &mut Session, id: u64) -> Result<(), String> {
        session
            .board
            .with_device_inputs::<Dwc3, _>(self.usb()?.controller, |usb, _| usb.consume_reply(id))
    }

    /// The DWC3 saved state (registers, endpoints, running) as JSON.
    pub fn usb_state(&self, session: &Session) -> Result<serde_json::Value, String> {
        use fujimod_emulator_devices::Device;
        session
            .board
            .with_device::<Dwc3, _>(self.usb()?.controller, |usb| usb.save())
    }

    /// Queue a physical XP5 key edge at an absolute guest nanosecond.
    pub fn key(
        &self,
        session: &mut Session,
        at_ns: u64,
        key: Key,
        pressed: bool,
    ) -> Result<(), String> {
        let panel = self.panel()?;
        match panel.input(key, pressed)? {
            Some(input) => session.schedule_input(panel.at(at_ns), input),
            None => Ok(()),
        }
    }

    /// Queue a latched dial or switch position change (`iso`, `ss`, `ev`,
    /// `drive`, `focus`, `mode`, `power` on the X-T5) at an absolute guest
    /// nanosecond.
    pub fn set_position(
        &self,
        session: &mut Session,
        at_ns: u64,
        dial: &str,
        position: &str,
    ) -> Result<(), String> {
        let panel = self.panel()?;
        session.schedule_input(panel.at(at_ns), panel.position(dial, position)?)
    }

    /// Drive one external XP5 GPIO input level and wake its registered devices.
    pub fn gpio(
        &self,
        session: &mut Session,
        port: usize,
        pin: usize,
        high: bool,
    ) -> Result<(), String> {
        if port >= 20 || pin >= 16 {
            return Err("XP5 GPIO input is outside the declared pin range".into());
        }
        self.pins
            .lock()
            .map_err(|_| "XP5 GPIO pins poisoned")?
            .set_input(port, pin, high);
        for &device in &self.gpio_devices {
            session.board.inputs_changed(device)?;
        }
        Ok(())
    }

    pub fn gpio_level(&self, port: usize, pin: usize) -> Result<bool, String> {
        if port >= 20 || pin >= 16 {
            return Err("XP5 GPIO input is outside the declared pin range".into());
        }
        Ok(self
            .pins
            .lock()
            .map_err(|_| "XP5 GPIO pins poisoned")?
            .level(port, pin)
            != 0)
    }

    pub fn gpio_input_levels(&self, port: usize) -> Result<u32, String> {
        if port >= 20 {
            return Err("XP5 GPIO port is outside the declared pin range".into());
        }
        Ok(self
            .pins
            .lock()
            .map_err(|_| "XP5 GPIO pins poisoned")?
            .word(port))
    }
}
