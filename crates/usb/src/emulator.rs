//! Caller-owned emulated USB endpoints below the ff80 protocol.
use crate::{
    Error, Result,
    connection::{InterfaceSelection, Io, TransferOptions, bulk_pair, deadline},
    error::TransferStage,
    ff80::framing::RequestKind,
};
use std::{future::Future, num::NonZeroU16, pin::Pin, time::Duration};

pub use nusb::transfer::{ControlType, Recipient, TransferError};

#[cfg(not(target_arch = "wasm32"))]
pub type EndpointFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
#[cfg(target_arch = "wasm32")]
pub type EndpointFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EndpointAddress(u8);
impl EndpointAddress {
    pub const fn new(address: u8) -> Option<Self> {
        if address & 0x70 == 0 && address & 0x0f != 0 {
            Some(Self(address))
        } else {
            None
        }
    }
    pub const fn get(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PacketSize(NonZeroU16);
impl PacketSize {
    pub const fn new(bytes: u16) -> Option<Self> {
        match NonZeroU16::new(bytes) {
            Some(bytes) => Some(Self(bytes)),
            None => None,
        }
    }
    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointKind {
    Bulk,
    Interrupt,
    Isochronous,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EndpointDescriptor {
    pub address: EndpointAddress,
    pub kind: EndpointKind,
    pub packet_size: PacketSize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterfaceDescriptor {
    pub selection: InterfaceSelection,
    pub endpoints: Vec<EndpointDescriptor>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceOperation {
    Claim,
    Release,
}

#[derive(Clone, Copy, Debug)]
pub struct ControlRequest {
    pub control_type: ControlType,
    pub recipient: Recipient,
    pub request: u8,
    pub value: u16,
    pub index: u16,
}

/// Counts bytes even on a failed or cancelled transfer.
#[derive(Clone, Copy, Debug)]
pub struct Completion {
    pub transferred: usize,
    pub status: std::result::Result<(), TransferError>,
}

/// Descriptor inspection must not claim resources. Claim cancellation must release any acquired resources.
pub trait EndpointProvider {
    fn interfaces(&self) -> &[InterfaceDescriptor];
    fn claim(
        &mut self,
        selection: InterfaceSelection,
    ) -> EndpointFuture<'_, std::result::Result<Box<dyn Endpoints>, TransferError>>;
}

/// One exclusive interface claim, owned by the connection and then the session.
/// Dropping a transfer future cancels host waiting; device completion can remain unknown.
/// Drop must release the claim without protocol traffic, including after a failed release.
/// Futures must yield during pending emulation work.
pub trait Endpoints: Send + Sync + 'static {
    fn control_out<'a>(
        &'a mut self,
        request: ControlRequest,
        data: &'a [u8],
    ) -> EndpointFuture<'a, Completion>;
    fn bulk_out<'a>(
        &'a mut self,
        endpoint: EndpointAddress,
        data: &'a [u8],
    ) -> EndpointFuture<'a, Completion>;
    fn bulk_in<'a>(
        &'a mut self,
        endpoint: EndpointAddress,
        data: &'a mut [u8],
    ) -> EndpointFuture<'a, Completion>;
    fn release(&mut self) -> EndpointFuture<'_, std::result::Result<(), TransferError>>;
}

pub(crate) struct EmulatorIo {
    endpoints: Box<dyn Endpoints>,
    input: EndpointAddress,
    output: EndpointAddress,
    timeout: Duration,
}
impl EmulatorIo {
    pub(crate) async fn open(
        provider: &mut impl EndpointProvider,
        selection: InterfaceSelection,
        options: TransferOptions,
    ) -> Result<Self> {
        let mut matching = provider
            .interfaces()
            .iter()
            .filter(|i| i.selection == selection);
        let descriptor = matching.next().ok_or(Error::InvalidInterface)?;
        if matching.next().is_some() {
            return Err(Error::InvalidInterface);
        }
        let (input, output) = bulk_pair(
            descriptor
                .endpoints
                .iter()
                .filter(|e| e.kind == EndpointKind::Bulk)
                .map(|e| e.address.get()),
        )?;
        let endpoints = resource_deadline(
            options.timeout,
            ResourceOperation::Claim,
            provider.claim(selection),
        )
        .await?;
        Ok(Self {
            endpoints,
            input: EndpointAddress(input),
            output: EndpointAddress(output),
            timeout: options.timeout,
        })
    }
}

async fn resource_deadline<T>(
    timeout: Duration,
    operation: ResourceOperation,
    task: EndpointFuture<'_, std::result::Result<T, TransferError>>,
) -> Result<T> {
    futures_lite::future::race(
        async {
            task.await
                .map_err(|source| Error::EmulatorResource { operation, source })
        },
        async {
            futures_timer::Delay::new(timeout).await;
            Err(Error::EmulatorResourceTimeout(operation))
        },
    )
    .await
}

fn check_completion(
    completion: Completion,
    capacity: usize,
    stage: TransferStage,
    partial: &mut Vec<u8>,
) -> Result<usize> {
    if completion.transferred > capacity {
        return Err(Error::EmulatorCompletion {
            stage,
            capacity,
            actual: completion.transferred,
        });
    }
    completion.status.map_err(|source| Error::Transfer {
        stage,
        source,
        transferred: Some(completion.transferred),
        partial: std::mem::take(partial),
    })?;
    Ok(completion.transferred)
}

impl Io for EmulatorIo {
    async fn release(&mut self) -> Result<()> {
        resource_deadline(
            self.timeout,
            ResourceOperation::Release,
            self.endpoints.release(),
        )
        .await
    }
    fn endpoint(&self, stage: TransferStage) -> Option<u8> {
        Some(match stage {
            TransferStage::Control(_) => 0,
            TransferStage::BulkIn => self.input.get(),
            TransferStage::BulkOut => self.output.get(),
        })
    }
    async fn control(&mut self, kind: RequestKind, envelope: [u8; 12]) -> Result<()> {
        let stage = TransferStage::Control(kind);
        let completion = deadline(
            self.timeout,
            stage,
            self.endpoints.control_out(
                ControlRequest {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Other,
                    request: 0,
                    value: 0,
                    index: 0,
                },
                &envelope,
            ),
        )
        .await?;
        let actual = check_completion(completion, envelope.len(), stage, &mut Vec::new())?;
        if actual != envelope.len() {
            return Err(Error::ShortTransfer {
                stage,
                expected: envelope.len(),
                actual,
            });
        }
        Ok(())
    }
    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let stage = TransferStage::BulkOut;
        let completion = deadline(
            self.timeout,
            stage,
            self.endpoints.bulk_out(self.output, bytes),
        )
        .await?;
        let actual = check_completion(completion, bytes.len(), stage, &mut Vec::new())?;
        if actual != bytes.len() {
            return Err(Error::ShortTransfer {
                stage,
                expected: bytes.len(),
                actual,
            });
        }
        Ok(())
    }
    async fn read(&mut self, maximum: usize) -> Result<Vec<u8>> {
        let mut data = vec![0; maximum];
        let completion = deadline(
            self.timeout,
            TransferStage::BulkIn,
            self.endpoints.bulk_in(self.input, &mut data),
        )
        .await?;
        data.truncate(completion.transferred.min(maximum));
        check_completion(completion, maximum, TransferStage::BulkIn, &mut data)?;
        Ok(data)
    }
}
