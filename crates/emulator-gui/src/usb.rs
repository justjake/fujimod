//! The camera's emulated USB port, shared in its room as the room's one USB
//! device. Controllers speak the USB share protocol to it; their operations
//! run as host transactions on the session thread between guest slices.
use crate::server::Run;
use fujimod_emulator_session::Session;
use fujimod_emulator_session::usb_host::{UsbDescriptors, UsbGuest, UsbHost};
use fujimod_usb::share_owner::{DeviceShare, SharedDevice};
use fujimod_usb_proxy::{
    Completion, Operation,
    protocol::{Device, Endpoint, Interface, Recipient, RequestType, Setup},
};
use std::{sync::mpsc, time::Duration};

/// How long the host waits for the device between packets.
const DEVICE_TIMEOUT: Duration = Duration::from_secs(10);

type Request = (Operation, async_channel::Sender<Completion>);

struct EmulatedPort {
    descriptor: Device,
    requests: mpsc::Sender<Request>,
}

impl SharedDevice for EmulatedPort {
    fn descriptor(&self) -> &Device {
        &self.descriptor
    }
    fn execute(
        &self,
        op: Operation,
    ) -> std::pin::Pin<Box<dyn Future<Output = Completion> + Send + '_>> {
        Box::pin(async move {
            let (reply, completion) = async_channel::bounded(1);
            if self.requests.send((op, reply)).is_err() {
                return failed("Emulated camera stopped");
            }
            completion
                .recv()
                .await
                .unwrap_or_else(|_| failed("Emulated camera stopped"))
        })
    }
}

fn failed(error: impl Into<String>) -> Completion {
    Completion {
        error: Some(error.into()),
        ..Default::default()
    }
}

/// A running camera seen by the USB host.
struct Guest<'a>(&'a mut dyn Run);
impl UsbGuest for Guest<'_> {
    fn session(&self) -> &Session {
        self.0.session()
    }
    fn session_mut(&mut self) -> &mut Session {
        self.0.session_mut()
    }
    fn settle(&mut self) -> Result<(), String> {
        self.0.settle()
    }
}

/// The shared USB port of one camera.
pub(crate) struct UsbShare {
    pub(crate) share: DeviceShare,
    port: String,
    requests: mpsc::Receiver<Request>,
    configuration: Option<u8>,
}

impl UsbShare {
    /// Enumerate the camera as a host would when its cable is plugged in, and
    /// share it on the room `link` names.
    pub(crate) fn connect(run: &mut dyn Run, link: &str) -> Result<Self, String> {
        let ports = run.session().usb_ports();
        let [port] = ports.as_slice() else {
            return Err(format!("this camera has {} USB ports to share", ports.len()));
        };
        let port = port.clone();
        let descriptors =
            UsbHost::new(&mut Guest(run), &port, DEVICE_TIMEOUT)?.enumerate()?;
        let (send, requests) = mpsc::channel();
        let device = EmulatedPort {
            descriptor: descriptor(&descriptors)?,
            requests: send,
        };
        Ok(Self {
            share: DeviceShare::new(std::sync::Arc::new(device), link)
                .map_err(|error| error.to_string())?,
            port,
            requests,
            configuration: None,
        })
    }

    /// Run the operations controllers have sent.
    pub(crate) fn service(&mut self, run: &mut dyn Run) {
        while let Ok((op, reply)) = self.requests.try_recv() {
            let completion = self
                .execute(run, op)
                .unwrap_or_else(failed);
            let _ = reply.send_blocking(completion);
        }
    }

    fn execute(&mut self, run: &mut dyn Run, op: Operation) -> Result<Completion, String> {
        let mut guest = Guest(run);
        let mut host = UsbHost::new(&mut guest, &self.port, DEVICE_TIMEOUT)?;
        let mut completion = Completion::default();
        match op {
            Operation::Open {
                configuration,
                interface,
                alternate,
            } => {
                if self.configuration != Some(configuration) {
                    host.control_out([0, 9, configuration, 0, 0, 0, 0, 0], &[])?;
                    self.configuration = Some(configuration);
                }
                if alternate != 0 {
                    host.control_out([1, 11, alternate, 0, interface, 0, 0, 0], &[])?;
                }
            }
            Operation::Close => {}
            Operation::ControlIn { setup, length } => {
                completion.data = host.control_in(setup_packet(&setup, true), length)?;
                completion.transferred = completion.data.len();
            }
            Operation::ControlOut { setup, data } => {
                completion.transferred = host.control_out(setup_packet(&setup, false), &data)?;
            }
            Operation::TransferIn { endpoint, length } => {
                completion.data = host.bulk_in(endpoint, length)?;
                completion.transferred = completion.data.len();
            }
            Operation::TransferOut { endpoint, data } => {
                completion.transferred = host.bulk_out(endpoint, &data)?;
            }
            Operation::ClearHalt { .. } => {
                return Err("The emulated USB port has no endpoint halt".into());
            }
            Operation::Probe
            | Operation::List
            | Operation::OwnerStatus { .. }
            | Operation::OwnerAttention { .. } => {
                return Err("The share owner answers this operation".into());
            }
        }
        Ok(completion)
    }
}

fn setup_packet(setup: &Setup, input: bool) -> [u8; 8] {
    let kind = match setup.request_type {
        RequestType::Standard => 0,
        RequestType::Class => 0x20,
        RequestType::Vendor => 0x40,
    };
    let recipient = match setup.recipient {
        Recipient::Device => 0,
        Recipient::Interface => 1,
        Recipient::Endpoint => 2,
        Recipient::Other => 3,
    };
    let mut packet = [0; 8];
    packet[0] = if input { 0x80 } else { 0 } | kind | recipient;
    packet[1] = setup.request;
    packet[2..4].copy_from_slice(&setup.value.to_le_bytes());
    packet[4..6].copy_from_slice(&setup.index.to_le_bytes());
    packet
}

/// The share inventory's view of the enumerated descriptors.
fn descriptor(descriptors: &UsbDescriptors) -> Result<Device, String> {
    let device = &descriptors.device;
    let mut interfaces = Vec::new();
    for raw in &descriptors.configurations {
        let configuration = *raw.get(5).ok_or("USB configuration descriptor is short")?;
        let mut offset = 0;
        while offset + 2 <= raw.len() {
            let length = usize::from(raw[offset]);
            if length < 2 || offset + length > raw.len() {
                return Err("USB descriptor extent is invalid".into());
            }
            let entry = &raw[offset..offset + length];
            match entry[1] {
                4 if length >= 9 => interfaces.push(Interface {
                    configuration,
                    interface: entry[2],
                    alternate: entry[3],
                    endpoints: Vec::new(),
                }),
                5 if length >= 7 => {
                    let interface = interfaces
                        .last_mut()
                        .ok_or("USB endpoint descriptor outside an interface")?;
                    interface.endpoints.push(Endpoint {
                        address: entry[2],
                        kind: match entry[3] & 3 {
                            0 => "control",
                            1 => "isochronous",
                            2 => "bulk",
                            _ => "interrupt",
                        }
                        .into(),
                        packet_size: u16::from_le_bytes([entry[4], entry[5]]) & 0x7ff,
                    });
                }
                _ => {}
            }
            offset += length;
        }
    }
    Ok(Device {
        id: "emulated-usb".into(),
        vendor_id: u16::from_le_bytes([device[8], device[9]]),
        product_id: u16::from_le_bytes([device[10], device[11]]),
        manufacturer: descriptors.manufacturer.clone(),
        product: descriptors.product.clone(),
        serial: descriptors.serial.clone(),
        interfaces,
        status: None,
    })
}
