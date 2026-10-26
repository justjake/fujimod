//! nusb device discovery and explicit USB interface selection.
use crate::{
    Error, Result,
    error::{TransferStage, check_limit},
    ff80::{Session, framing::RequestKind},
};
use nusb::{
    Endpoint, Interface,
    transfer::{Buffer, Bulk, ControlOut, ControlType, In, Out, Recipient},
};
use std::{num::NonZeroU32, time::Duration};

/// USB discovery scope; model/firmware identity is established separately.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceFilter {
    Fujifilm,
    Diagnostic,
}
/// A caller-selected local device or a descriptor advertised by a proxy exporter.
#[derive(Clone, Debug)]
pub enum Device {
    Local(nusb::DeviceInfo),
    Owned(std::sync::Arc<crate::owner::DeviceOwner>),
    Proxy(fujimod_usb_proxy::RemoteDevice),
}
impl Device {
    pub fn vendor_id(&self) -> u16 {
        match self {
            Self::Local(d) => d.vendor_id(),
            Self::Owned(d) => d.descriptor.vendor_id,
            Self::Proxy(d) => d.descriptor.vendor_id,
        }
    }
    pub fn product_id(&self) -> u16 {
        match self {
            Self::Local(d) => d.product_id(),
            Self::Owned(d) => d.descriptor.product_id,
            Self::Proxy(d) => d.descriptor.product_id,
        }
    }
    /// Proxy bus IDs are relay routes, not invented host topology.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn bus_id(&self) -> &str {
        match self {
            Self::Local(d) => d.bus_id(),
            Self::Owned(d) => d
                .info
                .as_ref()
                .map_or(&d.descriptor.id, |info| info.bus_id()),
            Self::Proxy(d) => &d.descriptor.id,
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn device_address(&self) -> u8 {
        match self {
            Self::Local(d) => d.device_address(),
            Self::Owned(d) => d.info.as_ref().map_or(0, |info| info.device_address()),
            Self::Proxy(_) => 0,
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn port_chain(&self) -> &[u8] {
        match self {
            Self::Local(d) => d.port_chain(),
            Self::Owned(d) => d.info.as_ref().map_or(&[], |info| info.port_chain()),
            Self::Proxy(_) => &[],
        }
    }
    pub fn manufacturer_string(&self) -> Option<&str> {
        match self {
            Self::Local(d) => d.manufacturer_string(),
            Self::Owned(d) => d.descriptor.manufacturer.as_deref(),
            Self::Proxy(d) => d.descriptor.manufacturer.as_deref(),
        }
    }
    pub fn product_string(&self) -> Option<&str> {
        match self {
            Self::Local(d) => d.product_string(),
            Self::Owned(d) => d.descriptor.product.as_deref(),
            Self::Proxy(d) => d.descriptor.product.as_deref(),
        }
    }
    pub fn serial_number(&self) -> Option<&str> {
        match self {
            Self::Local(d) => d.serial_number(),
            Self::Owned(d) => d.descriptor.serial.as_deref(),
            Self::Proxy(d) => d.descriptor.serial.as_deref(),
        }
    }
}
fn matches_filter(d: &Device, filter: DeviceFilter) -> bool {
    d.vendor_id() == 0x04cb && (filter == DeviceFilter::Fujifilm || d.product_id() == 0xff80)
}
pub async fn discover(filter: DeviceFilter) -> Result<Vec<Device>> {
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(url) = std::env::var("FUJIMOD_USB_PROXY_URL") {
        let token = if fujimod_usb_proxy::share::Ticket::from_url(&url)?.is_some() {
            String::new()
        } else {
            std::env::var("FUJIMOD_USB_PROXY_TOKEN").map_err(|_| {
                fujimod_usb_proxy::Error("FUJIMOD_USB_PROXY_TOKEN is required".into())
            })?
        };
        return discover_proxy(&url, &token, filter).await;
    }
    Ok(nusb::list_devices()
        .await?
        .map(Device::Local)
        .filter(|d| matches_filter(d, filter))
        .collect())
}
/// Explicit remote discovery works in Rust, native Python and browser Wasm.
/// Failure never falls back to a local camera.
pub async fn discover_proxy(url: &str, token: &str, filter: DeviceFilter) -> Result<Vec<Device>> {
    Ok(fujimod_usb_proxy::Client::discover(url, token)
        .await?
        .into_iter()
        .map(Device::Proxy)
        .filter(|d| matches_filter(d, filter))
        .collect())
}

/// Descriptor inspection only; does not claim or configure an interface.
pub async fn interfaces(info: &Device) -> Result<Vec<InterfaceSelection>> {
    if let Device::Owned(owner) = info {
        return Ok(owner
            .descriptor
            .interfaces
            .iter()
            .filter(|i| {
                bulk_pair(
                    &i.endpoints
                        .iter()
                        .filter(|e| e.kind == "bulk")
                        .map(|e| e.address)
                        .collect::<Vec<_>>(),
                )
                .is_ok()
            })
            .map(|i| InterfaceSelection {
                configuration: i.configuration,
                interface: i.interface,
                alternate: i.alternate,
            })
            .collect());
    }
    let Device::Local(info) = info else {
        let Device::Proxy(remote) = info else {
            unreachable!()
        };
        return Ok(remote
            .descriptor
            .interfaces
            .iter()
            .filter(|i| {
                bulk_pair(
                    &i.endpoints
                        .iter()
                        .filter(|e| e.kind == "bulk")
                        .map(|e| e.address)
                        .collect::<Vec<_>>(),
                )
                .is_ok()
            })
            .map(|i| InterfaceSelection {
                configuration: i.configuration,
                interface: i.interface,
                alternate: i.alternate,
            })
            .collect());
    };
    let device = info.open().await?;
    let mut selections = Vec::new();
    for config in device.configurations() {
        for interface in config.interface_alt_settings() {
            let addresses: Vec<_> = interface
                .endpoints()
                .filter(|e| e.transfer_type() == nusb::descriptors::TransferType::Bulk)
                .map(|e| e.address())
                .collect();
            if bulk_pair(&addresses).is_ok() {
                selections.push(InterfaceSelection {
                    configuration: config.configuration_value(),
                    interface: interface.interface_number(),
                    alternate: interface.alternate_setting(),
                });
            }
        }
    }
    Ok(selections)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterfaceSelection {
    pub configuration: u8,
    pub interface: u8,
    pub alternate: u8,
}

/// Host resource policy, independent of firmware command limits.
#[derive(Clone, Copy, Debug)]
pub struct TransferOptions {
    /// Per USB transfer, not a deadline for an entire camera operation.
    pub timeout: Duration,
    pub max_reply_bytes: NonZeroU32,
}

pub struct Connection {
    io: Backend,
    max_reply_bytes: u32,
}
impl Connection {
    /// Opens the caller-selected device/configuration/interface. No ff80 commands
    /// or driver detach/reset requests are sent here.
    pub async fn open(
        info: &Device,
        selection: InterfaceSelection,
        options: TransferOptions,
    ) -> Result<Self> {
        if options.timeout.is_zero() {
            return Err(Error::InvalidTimeout);
        }
        check_limit(8, options.max_reply_bytes.get())?;
        if let Device::Owned(owner) = info {
            let selection = crate::owner::Selection {
                configuration: selection.configuration,
                interface: selection.interface,
                alternate: selection.alternate,
            };
            let descriptor = owner
                .descriptor
                .interfaces
                .iter()
                .find(|i| {
                    i.configuration == selection.configuration
                        && i.interface == selection.interface
                        && i.alternate == selection.alternate
                })
                .ok_or(Error::InvalidInterface)?;
            let (input_address, output_address) = bulk_pair(
                &descriptor
                    .endpoints
                    .iter()
                    .filter(|e| e.kind == "bulk")
                    .map(|e| e.address)
                    .collect::<Vec<_>>(),
            )?;
            let (lease, interface) = owner.local(selection).await?;
            let input = interface.endpoint::<Bulk, In>(input_address)?;
            let output = interface.endpoint::<Bulk, Out>(output_address)?;
            return Ok(Self {
                io: Backend::Usb(UsbIo {
                    interface,
                    input_address,
                    output_address,
                    input,
                    output,
                    timeout: options.timeout,
                    lease: Some(lease),
                }),
                max_reply_bytes: options.max_reply_bytes.get(),
            });
        }
        let Device::Local(info) = info else {
            let Device::Proxy(remote) = info else {
                unreachable!()
            };
            let descriptor = remote
                .descriptor
                .interfaces
                .iter()
                .find(|i| {
                    i.configuration == selection.configuration
                        && i.interface == selection.interface
                        && i.alternate == selection.alternate
                })
                .ok_or(Error::InvalidInterface)?;
            let endpoints: Vec<_> = descriptor
                .endpoints
                .iter()
                .filter(|e| e.kind == "bulk")
                .collect();
            let (input, output) =
                bulk_pair(&endpoints.iter().map(|e| e.address).collect::<Vec<_>>())?;
            let packet_size = usize::from(
                endpoints
                    .iter()
                    .find(|e| e.address == input)
                    .ok_or(Error::InvalidInterface)?
                    .packet_size,
            );
            if packet_size == 0 {
                return Err(Error::InvalidInterface);
            }
            let mut client = remote.connect().await?;
            let completion = client
                .request(
                    &remote.descriptor.id,
                    fujimod_usb_proxy::Operation::Open {
                        configuration: selection.configuration,
                        interface: selection.interface,
                        alternate: selection.alternate,
                    },
                    options.timeout,
                )
                .await?;
            if let Some(error) = completion.error {
                return Err(fujimod_usb_proxy::Error(error).into());
            }
            return Ok(Self {
                io: Backend::Proxy(crate::proxy::ProxyIo {
                    client,
                    device: remote.descriptor.id.clone(),
                    input,
                    output,
                    packet_size,
                    timeout: options.timeout,
                    unsynced: Vec::new(),
                    observation: None,
                    observed: std::collections::BTreeMap::new(),
                }),
                max_reply_bytes: options.max_reply_bytes.get(),
            });
        };
        let device = info.open().await?;
        let descriptor = device
            .configurations()
            .find(|c| c.configuration_value() == selection.configuration)
            .and_then(|c| {
                c.interface_alt_settings().find(|i| {
                    i.interface_number() == selection.interface
                        && i.alternate_setting() == selection.alternate
                })
            })
            .ok_or(Error::InvalidInterface)?;
        let addresses: Vec<_> = descriptor
            .endpoints()
            .filter(|e| e.transfer_type() == nusb::descriptors::TransferType::Bulk)
            .map(|e| e.address())
            .collect();
        let (input, output) = bulk_pair(&addresses)?;
        if device
            .active_configuration()
            .map(|c| c.configuration_value())
            .ok()
            != Some(selection.configuration)
        {
            device.set_configuration(selection.configuration).await?;
        }
        let interface = device.claim_interface(selection.interface).await?;
        if interface.get_alt_setting() != selection.alternate {
            interface.set_alt_setting(selection.alternate).await?;
        }
        let input_address = input;
        let output_address = output;
        let input = interface.endpoint::<Bulk, In>(input)?;
        let output = interface.endpoint::<Bulk, Out>(output)?;
        Ok(Self {
            io: Backend::Usb(UsbIo {
                interface,
                input_address,
                output_address,
                input,
                output,
                timeout: options.timeout,
                lease: None,
            }),
            max_reply_bytes: options.max_reply_bytes.get(),
        })
    }

    /// Label the current local owner lease before its first FF80 exchange.
    pub fn set_tool(&self, tool: String) {
        if let Backend::Usb(io) = &self.io
            && let Some(lease) = &io.lease
        {
            lease.set_tool(tool);
        }
    }
    /// Attach controller-side state and Rust protocol observations to a proxy.
    pub fn observe_remote(&mut self, lease: crate::remote::Lease) {
        if let Backend::Proxy(io) = &mut self.io {
            io.observation = Some(lease);
        }
    }
    /// Begins FF80; explicitly close the session after normal completion.
    /// Drop never sends a camera close command.
    pub async fn begin_session(self, trace: Option<crate::trace::Trace>) -> Result<Session> {
        Session::open(self.io, self.max_reply_bytes, trace).await
    }
}

fn bulk_pair(addresses: &[u8]) -> Result<(u8, u8)> {
    let inputs: Vec<_> = addresses
        .iter()
        .copied()
        .filter(|a| a & 0x80 != 0 && a & 0x0f != 0)
        .collect();
    let outputs: Vec<_> = addresses
        .iter()
        .copied()
        .filter(|a| a & 0x80 == 0 && a & 0x0f != 0)
        .collect();
    match (inputs.as_slice(), outputs.as_slice()) {
        ([input], [output]) => Ok((*input, *output)),
        _ => Err(Error::AmbiguousEndpoints),
    }
}

// Private transfer boundary shared by local nusb, remote USB and test peers.
pub(crate) trait Io {
    /// Release host resources after a successful camera protocol close.
    async fn release(&mut self) -> Result<()> {
        Ok(())
    }
    fn trace(&self) -> Option<crate::trace::Trace> {
        None
    }
    fn endpoint(&self, _stage: TransferStage) -> Option<u8> {
        None
    }
    async fn control(&mut self, kind: RequestKind, envelope: [u8; 12]) -> Result<()>;
    async fn write(&mut self, bytes: &[u8]) -> Result<()>;
    async fn read(&mut self, maximum: usize) -> Result<Vec<u8>>;
}

pub(crate) struct UsbIo {
    interface: Interface,
    input_address: u8,
    output_address: u8,
    input: Endpoint<Bulk, In>,
    output: Endpoint<Bulk, Out>,
    timeout: Duration,
    // Drop endpoints before releasing the logical owner claim.
    lease: Option<crate::owner::LocalLease>,
}

async fn deadline<T>(
    timeout: Duration,
    stage: TransferStage,
    task: impl std::future::IntoFuture<Output = T>,
) -> Result<T> {
    futures_lite::future::race(async { Ok(task.await) }, async {
        futures_timer::Delay::new(timeout).await;
        Err(Error::Timeout(stage))
    })
    .await
}

impl Io for UsbIo {
    async fn release(&mut self) -> Result<()> {
        if let Some(lease) = &mut self.lease {
            lease.release();
        }
        Ok(())
    }
    fn endpoint(&self, stage: TransferStage) -> Option<u8> {
        Some(match stage {
            TransferStage::Control(_) => 0,
            TransferStage::BulkIn => self.input_address,
            TransferStage::BulkOut => self.output_address,
        })
    }
    async fn control(&mut self, kind: RequestKind, envelope: [u8; 12]) -> Result<()> {
        if let Some(lease) = &self.lease {
            lease.owner().observe_control(&envelope);
        }
        let result = self.control_transfer(kind, envelope).await;
        if let Some(lease) = &self.lease {
            match &result {
                Ok(()) => lease.owner().observe_control_complete(),
                Err(error) => lease.owner().observe_failure(&error.to_string()),
            }
        }
        result
    }
    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let result = self.write_transfer(bytes).await;
        if let Some(lease) = &self.lease {
            let transferred = match &result {
                Ok(()) => bytes.len(),
                Err(Error::Transfer { transferred, .. }) => transferred.unwrap_or(0),
                Err(Error::ShortTransfer { actual, .. }) => *actual,
                _ => 0,
            };
            lease.owner().observe_payload(
                false,
                bytes,
                bytes.len(),
                transferred,
                result.as_ref().err().map(ToString::to_string),
            );
        }
        result
    }
    async fn read(&mut self, maximum: usize) -> Result<Vec<u8>> {
        let result = self.read_transfer(maximum).await;
        if let Some(lease) = &self.lease {
            match &result {
                Ok(data) => lease
                    .owner()
                    .observe_payload(true, data, maximum, data.len(), None),
                Err(Error::Transfer {
                    transferred,
                    partial,
                    ..
                }) => lease.owner().observe_payload(
                    true,
                    partial,
                    maximum,
                    transferred.unwrap_or(0),
                    result.as_ref().err().map(ToString::to_string),
                ),
                Err(error) => lease.owner().observe_failure(&error.to_string()),
            }
        }
        result
    }
}
impl UsbIo {
    async fn control_transfer(&mut self, kind: RequestKind, envelope: [u8; 12]) -> Result<()> {
        // nusb's WebUSB backend has no control-transfer timeout. Apply the
        // same host deadline as bulk transfers; dropping this future cannot
        // establish that the camera stopped processing the envelope.
        deadline(
            self.timeout,
            TransferStage::Control(kind),
            self.interface.control_out(
                ControlOut {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Other,
                    request: 0,
                    value: 0,
                    index: 0,
                    data: &envelope,
                },
                self.timeout,
            ),
        )
        .await?
        .map_err(|source| Error::Transfer {
            stage: TransferStage::Control(kind),
            source,
            transferred: None,
            partial: vec![],
        })
    }
    async fn write_transfer(&mut self, bytes: &[u8]) -> Result<()> {
        self.output.submit(bytes.to_vec().into());
        let done = deadline(
            self.timeout,
            TransferStage::BulkOut,
            self.output.next_complete(),
        )
        .await?;
        done.status.map_err(|source| Error::Transfer {
            stage: TransferStage::BulkOut,
            source,
            transferred: Some(done.actual_len),
            partial: vec![],
        })?;
        if done.actual_len != bytes.len() {
            return Err(Error::ShortTransfer {
                stage: TransferStage::BulkOut,
                expected: bytes.len(),
                actual: done.actual_len,
            });
        }
        Ok(())
    }
    async fn read_transfer(&mut self, maximum: usize) -> Result<Vec<u8>> {
        // nusb requires IN buffer sizes to be multiples of USB wMaxPacketSize.
        // The control envelope still requests the exact ff80 response size.
        let packet = self.input.max_packet_size();
        let rounded = maximum
            .div_ceil(packet)
            .checked_mul(packet)
            .filter(|n| *n <= u32::MAX as usize)
            .ok_or(Error::InvalidLength)?;
        self.input.submit(Buffer::new(rounded));
        let done = deadline(
            self.timeout,
            TransferStage::BulkIn,
            self.input.next_complete(),
        )
        .await?;
        let data = done.buffer.to_vec();
        done.status.map_err(|source| Error::Transfer {
            stage: TransferStage::BulkIn,
            source,
            transferred: Some(done.actual_len),
            partial: data.clone(),
        })?;
        Ok(data)
    }
}

// The replay variant exists only in builds explicitly enabling test support.
pub(crate) enum Backend {
    Usb(UsbIo),
    Proxy(crate::proxy::ProxyIo),
    Replay(crate::testing::Script),
    Model(crate::testing::Model),
}
impl Io for Backend {
    async fn release(&mut self) -> Result<()> {
        match self {
            Self::Proxy(io) => io.release().await,
            Self::Usb(io) => io.release().await,
            _ => Ok(()),
        }
    }
    fn endpoint(&self, stage: TransferStage) -> Option<u8> {
        match self {
            Self::Usb(io) => io.endpoint(stage),
            Self::Proxy(io) => io.endpoint(stage),
            Self::Replay(io) => io.endpoint(stage),
            Self::Model(io) => io.endpoint(stage),
        }
    }
    async fn control(&mut self, kind: RequestKind, envelope: [u8; 12]) -> Result<()> {
        match self {
            Self::Usb(io) => io.control(kind, envelope).await,
            Self::Proxy(io) => io.control(kind, envelope).await,
            Self::Replay(io) => io.control(kind, envelope).await,
            Self::Model(io) => io.control(kind, envelope).await,
        }
    }
    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        match self {
            Self::Usb(io) => io.write(bytes).await,
            Self::Proxy(io) => io.write(bytes).await,
            Self::Replay(io) => io.write(bytes).await,
            Self::Model(io) => io.write(bytes).await,
        }
    }
    async fn read(&mut self, maximum: usize) -> Result<Vec<u8>> {
        match self {
            Self::Usb(io) => io.read(maximum).await,
            Self::Proxy(io) => io.read(maximum).await,
            Self::Replay(io) => io.read(maximum).await,
            Self::Model(io) => io.read(maximum).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoint_selection_never_silently_chooses_first_pair() {
        assert_eq!(bulk_pair(&[0x82, 0x03]).unwrap(), (0x82, 3));
        assert!(bulk_pair(&[0x81, 0x82, 3]).is_err());
        assert!(bulk_pair(&[0x81]).is_err());
        assert!(bulk_pair(&[0x80, 0]).is_err());
    }
    #[test]
    fn unfinished_transfer_has_a_deadline() {
        let result = futures_lite::future::block_on(deadline(
            Duration::from_millis(1),
            TransferStage::BulkIn,
            std::future::pending::<()>(),
        ));
        assert!(matches!(result, Err(Error::Timeout(TransferStage::BulkIn))));
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    async fn host_control_deadline_works_without_a_blocking_thread() {
        let stage = TransferStage::Control(RequestKind::ReadCommand);
        let result = deadline(
            Duration::from_millis(1),
            stage,
            std::future::pending::<()>(),
        )
        .await;
        assert!(matches!(result, Err(Error::Timeout(actual)) if actual == stage));
    }
}
