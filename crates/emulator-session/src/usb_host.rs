//! A USB host on a board's [`UsbPort`]: enumeration, control and bulk
//! transactions. Between packets the host waits for the device's controller
//! by advancing guest time; payloads stay opaque.
use super::Session;
use fujimod_emulator_devices::capability::{UsbHostWait, UsbPort};
use std::time::{Duration, Instant};

const CONTROL_PACKET: usize = 64;

/// The device's descriptors as enumeration read them.
#[derive(Clone, Debug)]
pub struct UsbDescriptors {
    pub device: Vec<u8>,
    pub configurations: Vec<Vec<u8>>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
}

/// The running camera a USB host drives.
pub trait UsbGuest {
    fn session(&self) -> &Session;
    fn session_mut(&mut self) -> &mut Session;
    /// Run a short slice of guest time while the host waits on the device.
    fn settle(&mut self) -> Result<(), String>;
}

/// A host transaction stream on one port.
pub struct UsbHost<'a> {
    guest: &'a mut dyn UsbGuest,
    node: usize,
    timeout: Duration,
}

impl Session {
    /// The session nodes that are USB device ports.
    pub fn usb_ports(&self) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|(_, id)| {
                self.board
                    .inspect_capability::<dyn UsbPort, _>(**id, |_| Ok(()))
                    .is_ok()
            })
            .map(|(name, _)| name.clone())
            .collect()
    }
}

impl<'a> UsbHost<'a> {
    pub fn new(guest: &'a mut dyn UsbGuest, port: &str, timeout: Duration) -> Result<Self, String> {
        let node = guest.session().node(port)?;
        Ok(Self {
            guest,
            node,
            timeout,
        })
    }

    fn inspect<R>(
        &self,
        observe: impl FnOnce(&dyn UsbPort) -> Result<R, String>,
    ) -> Result<R, String> {
        self.guest
            .session()
            .board
            .inspect_capability::<dyn UsbPort, _>(self.node, |port| {
                if !port.cable_connected() {
                    return Err("USB cable is disconnected".into());
                }
                observe(port)
            })
    }

    fn input<R>(
        &mut self,
        input: impl FnOnce(&mut dyn UsbPort, &mut fujimod_emulator_devices::Cx<'_>) -> Result<R, String>,
    ) -> Result<R, String> {
        self.guest
            .session_mut()
            .board
            .capability_input::<dyn UsbPort, _>(self.node, |port, cx| {
                if !port.cable_connected() {
                    return Err("USB cable is disconnected".into());
                }
                input(port, cx)
            })
    }

    fn wait(&mut self, wait: UsbHostWait) -> Result<(), String> {
        let deadline = Instant::now() + self.timeout;
        while !self.inspect(|port| port.host_ready(wait))? {
            if Instant::now() >= deadline {
                return Err(format!("USB device did not reach {wait:?}"));
            }
            self.guest.settle()?;
        }
        Ok(())
    }

    /// Take the next packet from an IN endpoint, sending IN tokens while the
    /// device waits for one.
    fn take(&mut self, endpoint: u8, size: usize) -> Result<Vec<u8>, String> {
        let deadline = Instant::now() + self.timeout;
        loop {
            if self.inspect(|port| port.host_ready(UsbHostWait::InPacket(endpoint)))? {
                return self.input(|port, cx| port.host_in(cx, endpoint, size));
            }
            if self.inspect(|port| port.host_in_wants_token(endpoint))? {
                self.input(|port, cx| port.host_in_token(cx, endpoint))?;
            }
            if Instant::now() >= deadline {
                return Err(format!("USB IN endpoint {endpoint:#04x} sent no packet"));
            }
            self.guest.settle()?;
        }
    }

    fn setup(&mut self, packet: [u8; 8]) -> Result<(), String> {
        self.wait(UsbHostWait::Idle)?;
        // A host inter-transaction gap lets the previous interrupt retire.
        self.guest.settle()?;
        self.input(|port, cx| port.host_setup(cx, &packet))?;
        self.wait(UsbHostWait::SetupTaken)
    }

    /// A control transfer whose data stage reads up to `length` bytes.
    pub fn control_in(&mut self, setup: [u8; 8], length: usize) -> Result<Vec<u8>, String> {
        if length == 0 || length > 0xffff || setup[0] & 0x80 == 0 {
            return Err("Control IN needs a device-to-host setup and a positive length".into());
        }
        let mut packet = setup;
        packet[6..8].copy_from_slice(&(length as u16).to_le_bytes());
        self.setup(packet)?;
        let mut data = Vec::new();
        while data.len() < length {
            let chunk = self.take(0x80, CONTROL_PACKET)?;
            let short = chunk.len() < CONTROL_PACKET;
            data.extend(chunk);
            if short {
                break;
            }
        }
        if data.len() > length {
            return Err("USB device answered more than the control request asked".into());
        }
        self.wait(UsbHostWait::Idle)?;
        self.input(|port, cx| port.host_out(cx, 0, &[]))?;
        self.wait(UsbHostWait::Idle)?;
        Ok(data)
    }

    /// A control transfer with an OUT data stage (possibly empty).
    pub fn control_out(&mut self, setup: [u8; 8], data: &[u8]) -> Result<usize, String> {
        if data.len() > 0xffff || setup[0] & 0x80 != 0 {
            return Err("Control OUT needs a host-to-device setup".into());
        }
        let mut packet = setup;
        packet[6..8].copy_from_slice(&(data.len() as u16).to_le_bytes());
        self.setup(packet)?;
        for chunk in data.chunks(CONTROL_PACKET) {
            self.input(|port, cx| port.host_out(cx, 0, chunk))?;
            self.wait(UsbHostWait::ControlOutTaken(chunk.len()))?;
        }
        if !self.take(0x80, CONTROL_PACKET)?.is_empty() {
            return Err("USB device's control status stage carried data".into());
        }
        self.wait(UsbHostWait::Idle)?;
        Ok(data.len())
    }

    pub fn bulk_out(&mut self, endpoint: u8, data: &[u8]) -> Result<usize, String> {
        let size = self.inspect(|port| port.packet_bytes())?;
        for chunk in data.chunks(size) {
            self.wait(UsbHostWait::OutTaken(endpoint))?;
            self.input(|port, cx| port.host_out(cx, endpoint, chunk))?;
            self.wait(UsbHostWait::OutTaken(endpoint))?;
        }
        Ok(data.len())
    }

    /// Read up to `length` bytes; a short packet ends the transfer.
    pub fn bulk_in(&mut self, endpoint: u8, length: usize) -> Result<Vec<u8>, String> {
        if length == 0 {
            return Err("USB IN needs a positive length".into());
        }
        let size = self.inspect(|port| port.packet_bytes())?;
        let mut data = Vec::with_capacity(length);
        while data.len() < length {
            let count = size.min(length - data.len());
            let packet = self.take(endpoint, count)?;
            let short = packet.len() < count;
            data.extend(packet);
            if short {
                break;
            }
        }
        Ok(data)
    }

    /// Reset the bus once the device is attached, address it and read its
    /// descriptors, as a host does when a cable is plugged in.
    pub fn enumerate(&mut self) -> Result<UsbDescriptors, String> {
        self.wait(UsbHostWait::Enabled)?;
        self.input(|port, cx| port.host_reset(cx))?;
        self.wait(UsbHostWait::Idle)?;
        let descriptor = |host: &mut Self, value: u16, index: u16, length: usize| {
            let mut setup = [0x80, 6, 0, 0, 0, 0, 0, 0];
            setup[2..4].copy_from_slice(&value.to_le_bytes());
            setup[4..6].copy_from_slice(&index.to_le_bytes());
            host.control_in(setup, length)
        };
        let device = descriptor(self, 0x100, 0, 18)?;
        if device.len() != 18 || device[0] != 18 || device[1] != 1 {
            return Err("USB device descriptor is invalid".into());
        }
        // SET_ADDRESS 1.
        self.control_out([0, 5, 1, 0, 0, 0, 0, 0], &[])?;
        let mut configurations = Vec::new();
        for index in 0..u16::from(device[17]) {
            let header = descriptor(self, 0x200 | index, 0, 9)?;
            if header.len() != 9 || header[1] != 2 {
                return Err("USB configuration descriptor is invalid".into());
            }
            let total = usize::from(u16::from_le_bytes([header[2], header[3]]));
            configurations.push(descriptor(self, 0x200 | index, 0, total)?);
        }
        let mut strings = [None, None, None];
        if device[14..17].iter().any(|&index| index != 0) {
            let languages = descriptor(self, 0x300, 0, 255)?;
            if languages.len() < 4 || languages[1] != 3 {
                return Err("USB language descriptor is invalid".into());
            }
            let language = u16::from_le_bytes([languages[2], languages[3]]);
            for (slot, &index) in strings.iter_mut().zip(&device[14..17]) {
                if index == 0 {
                    continue;
                }
                let raw = descriptor(self, 0x300 | u16::from(index), language, 255)?;
                if raw.len() < 2
                    || usize::from(raw[0]) != raw.len()
                    || raw[1] != 3
                    || raw.len() % 2 != 0
                {
                    return Err("USB string descriptor is invalid".into());
                }
                let units: Vec<u16> = raw[2..]
                    .chunks(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect();
                *slot = Some(String::from_utf16(&units).map_err(|e| e.to_string())?);
            }
        }
        let [manufacturer, product, serial] = strings;
        Ok(UsbDescriptors {
            device,
            configurations,
            manufacturer,
            product,
            serial,
        })
    }
}
