//! External XP5 controls and GPIO inputs of an opened XP5 session.
use fujimod_emulator_session::Session;
use fujimod_emulator_devices::xp5::pins::SharedPins;
pub use fujimod_emulator_parts_controls::key_input::Key;
use fujimod_emulator_parts_controls::key_input::KeyInput;
use fujimod_emulator_ip::usb::dwc3::{Dwc3, Reply};
use std::time::Duration;

/// External XP5 controls retained after device construction, for the session
/// `boot::open` returned with them. Their modeled state is checkpointed by the
/// devices that consume them.
pub struct Inputs {
    pins: SharedPins,
    controls: Option<std::sync::Arc<dyn KeyInput>>,
    gpio_devices: Vec<usize>,
    control_source: usize,
    usb: Option<UsbWiring>,
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

impl Inputs {
    pub(crate) fn new(
        pins: SharedPins,
        controls: Option<std::sync::Arc<dyn KeyInput>>,
        gpio_devices: Vec<usize>,
        control_source: usize,
        usb: Option<UsbWiring>,
    ) -> Self {
        Self {
            pins,
            controls,
            gpio_devices,
            control_source,
            usb,
        }
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
            {
                if power.replace(id).is_some() {
                    return Err("more than one XP5 power controller is attached".into());
                }
            }
        }
        let power = power.ok_or("the XP5 board has no power controller for the USB supply")?;
        session.board.with_device_inputs::<Spi, _>(power, |spi, _| {
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
            .with_device_inputs::<Dwc3, _>(self.usb()?.controller, |usb, now| usb.link(now, connected))
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
        self.controls
            .as_ref()
            .ok_or("this XP5 body has no physical key model; key edges are refused")?
            .enqueue(Duration::from_nanos(at_ns), key, pressed)?;
        session.board.inputs_changed(self.control_source)
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
        self.controls
            .as_ref()
            .ok_or("this XP5 body has no physical key model; position changes are refused")?
            .set_position(Duration::from_nanos(at_ns), dial, position)?;
        session.board.inputs_changed(self.control_source)
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
