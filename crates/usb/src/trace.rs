//! Bounded, pull-based capture of ff80 USB transfers. Recording never calls
//! user code or changes an exchange's result; discarded records are counted.
use crate::{Error, Result, connection::Io, error::TransferStage, ff80::framing::RequestKind};
use std::{
    collections::VecDeque,
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

// std's SystemTime::now is unavailable on wasm32-unknown-unknown. Keep the
// public timestamp type and epoch while using the browser/JS wall clock there.
#[cfg(target_arch = "wasm32")]
fn timestamp() -> std::time::SystemTime {
    std::time::UNIX_EPOCH + std::time::Duration::from_millis(js_sys::Date::now() as u64)
}
#[cfg(not(target_arch = "wasm32"))]
fn timestamp() -> std::time::SystemTime {
    std::time::SystemTime::now()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Failure {
    Transfer(nusb::transfer::TransferError),
    Timeout,
    ShortTransfer { expected: usize, actual: usize },
    Other(String),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    Submitted {
        requested: usize,
        data: Vec<u8>,
    },
    Completed {
        transferred: usize,
        data: Vec<u8>,
    },
    Failed {
        transferred: Option<usize>,
        data: Vec<u8>,
        failure: Failure,
    },
    /// The host dropped the transfer future. Camera execution may continue.
    Canceled,
}
impl Action {
    pub fn data(&self) -> &[u8] {
        match self {
            Self::Submitted { data, .. }
            | Self::Completed { data, .. }
            | Self::Failed { data, .. } => data,
            Self::Canceled => &[],
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Event {
    pub sequence: u64,
    pub timestamp: std::time::SystemTime,
    pub transfer: u64,
    pub stage: TransferStage,
    /// Actual USB endpoint address when available. Control uses endpoint zero.
    pub endpoint: Option<u8>,
    pub action: Action,
}
#[derive(Debug)]
pub struct Batch {
    pub events: Vec<Event>,
    pub protocol_events: Vec<ProtocolEvent>,
    pub dropped_events: u64,
    pub dropped_bytes: u64,
}

/// Protocol observations are captured where a command is executed, before
/// and after its nested transfers. They share ordering and budgets with USB.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Layer {
    Ff80,
    App80,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolEvent {
    pub sequence: u64,
    pub timestamp: std::time::SystemTime,
    pub exchange: u64,
    pub layer: Layer,
    pub command: u32,
    pub action: Action,
}
enum Record {
    Transfer(Event),
    Protocol(ProtocolEvent),
}
impl Record {
    fn bytes(&self) -> usize {
        match self {
            Self::Transfer(e) => e.action.data().len(),
            Self::Protocol(e) => e.action.data().len(),
        }
    }
}

/// Shared recorder. Clone before opening a session to retain handshake failures
/// and drain independently of session ownership or an in-flight operation.
///
/// Oldest records are discarded when either bound is reached. An individual
/// record larger than the byte budget is discarded whole, never truncated.
#[derive(Clone)]
pub struct Trace {
    inner: Arc<Mutex<State>>,
}
struct State {
    records: VecDeque<Record>,
    max_events: usize,
    max_bytes: usize,
    bytes: usize,
    next_sequence: u64,
    next_transfer: u64,
    dropped_events: u64,
    dropped_bytes: u64,
}
impl Trace {
    pub fn new(max_events: NonZeroU32, max_bytes: NonZeroU32) -> Self {
        Self {
            inner: Arc::new(Mutex::new(State {
                records: VecDeque::new(),
                max_events: max_events.get() as usize,
                max_bytes: max_bytes.get() as usize,
                bytes: 0,
                next_sequence: 0,
                next_transfer: 0,
                dropped_events: 0,
                dropped_bytes: 0,
            })),
        }
    }
    pub fn drain(&self) -> Batch {
        let mut state = self.inner.lock().expect("trace mutex");
        let mut events = Vec::new();
        let mut protocol_events = Vec::new();
        for record in state.records.drain(..) {
            match record {
                Record::Transfer(e) => events.push(e),
                Record::Protocol(e) => protocol_events.push(e),
            }
        }
        let batch = Batch {
            events,
            protocol_events,
            dropped_events: state.dropped_events,
            dropped_bytes: state.dropped_bytes,
        };
        state.bytes = 0;
        state.dropped_events = 0;
        state.dropped_bytes = 0;
        batch
    }
    fn record(
        &self,
        transfer: u64,
        stage: TransferStage,
        endpoint: Option<u8>,
        bytes: usize,
        action: impl FnOnce() -> Action,
    ) {
        self.record_capture(bytes, |sequence| {
            Record::Transfer(Event {
                sequence,
                timestamp: timestamp(),
                transfer,
                stage,
                endpoint,
                action: action(),
            })
        });
    }
    fn record_capture(&self, bytes: usize, make: impl FnOnce(u64) -> Record) {
        let mut state = self.inner.lock().expect("trace mutex");
        let sequence = state.next_sequence;
        state.next_sequence = state.next_sequence.wrapping_add(1);
        if bytes > state.max_bytes {
            state.dropped_events += 1;
            state.dropped_bytes += bytes as u64;
            return;
        }
        while state.records.len() >= state.max_events || bytes > state.max_bytes - state.bytes {
            let old = state.records.pop_front().expect("nonempty bounded trace");
            let removed = old.bytes();
            state.bytes -= removed;
            state.dropped_events += 1;
            state.dropped_bytes += removed as u64;
        }
        state.records.push_back(make(sequence));
        state.bytes += bytes;
    }
    pub fn protocol(&self, layer: Layer, command: u32, data: &[u8]) -> ProtocolSpan {
        let exchange = {
            let mut s = self.inner.lock().expect("trace mutex");
            let id = s.next_transfer;
            s.next_transfer = s.next_transfer.wrapping_add(1);
            id
        };
        let span = ProtocolSpan {
            trace: self.clone(),
            exchange,
            layer,
            command,
            finished: false,
        };
        span.record(data.len(), || Action::Submitted {
            requested: data.len(),
            data: data.to_vec(),
        });
        span
    }
    fn start(
        &self,
        stage: TransferStage,
        endpoint: Option<u8>,
        requested: usize,
        data: &[u8],
    ) -> Span {
        let transfer = {
            let mut s = self.inner.lock().expect("trace mutex");
            let id = s.next_transfer;
            s.next_transfer = s.next_transfer.wrapping_add(1);
            id
        };
        self.record(transfer, stage, endpoint, data.len(), || {
            Action::Submitted {
                requested,
                data: data.to_vec(),
            }
        });
        Span {
            trace: self.clone(),
            transfer,
            stage,
            endpoint,
            finished: false,
        }
    }
}
/// A protocol exchange being traced; completing it records its result.
pub struct ProtocolSpan {
    trace: Trace,
    exchange: u64,
    layer: Layer,
    command: u32,
    finished: bool,
}
impl ProtocolSpan {
    fn record(&self, bytes: usize, action: impl FnOnce() -> Action) {
        self.trace.record_capture(bytes, |sequence| {
            Record::Protocol(ProtocolEvent {
                sequence,
                timestamp: timestamp(),
                exchange: self.exchange,
                layer: self.layer,
                command: self.command,
                action: action(),
            })
        });
    }
    pub fn complete<T>(mut self, result: &Result<T>, data: &[u8]) {
        self.finished = true;
        self.record(data.len(), || match result {
            Ok(_) => Action::Completed {
                transferred: data.len(),
                data: data.to_vec(),
            },
            Err(e) => Action::Failed {
                transferred: (!data.is_empty()).then_some(data.len()),
                data: data.to_vec(),
                failure: Failure::Other(e.to_string()),
            },
        });
    }
}
impl Drop for ProtocolSpan {
    fn drop(&mut self) {
        if !self.finished {
            self.record(0, || Action::Canceled);
        }
    }
}

struct Span {
    trace: Trace,
    transfer: u64,
    stage: TransferStage,
    endpoint: Option<u8>,
    finished: bool,
}
impl Span {
    fn complete<T>(mut self, result: &Result<T>, transferred: usize, data: &[u8]) {
        self.finished = true;
        match result {
            Ok(_) => {
                self.trace
                    .record(self.transfer, self.stage, self.endpoint, data.len(), || {
                        Action::Completed {
                            transferred,
                            data: data.to_vec(),
                        }
                    })
            }
            Err(error) => {
                let (count, partial, failure) = match error {
                    Error::Transfer {
                        source,
                        transferred,
                        partial,
                        ..
                    } => (*transferred, partial.as_slice(), Failure::Transfer(*source)),
                    Error::Timeout(_) => (None, &[][..], Failure::Timeout),
                    Error::ShortTransfer {
                        expected, actual, ..
                    } => (
                        Some(*actual),
                        &[][..],
                        Failure::ShortTransfer {
                            expected: *expected,
                            actual: *actual,
                        },
                    ),
                    error => (None, &[][..], Failure::Other(error.to_string())),
                };
                self.trace.record(
                    self.transfer,
                    self.stage,
                    self.endpoint,
                    partial.len(),
                    || Action::Failed {
                        transferred: count,
                        data: partial.to_vec(),
                        failure,
                    },
                );
            }
        }
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        if !self.finished {
            self.trace
                .record(self.transfer, self.stage, self.endpoint, 0, || {
                    Action::Canceled
                });
        }
    }
}

// Observation wraps the existing private USB/test seam; it does not select an
// operating-system transport or expose a second transport API.
pub(crate) struct Observed<I> {
    io: I,
    trace: Option<Trace>,
}
impl<I> Observed<I> {
    pub(crate) fn new(io: I, trace: Option<Trace>) -> Self {
        Self { io, trace }
    }
    pub(crate) fn inner_mut(&mut self) -> &mut I {
        &mut self.io
    }
}
impl<I: Io> Io for Observed<I> {
    async fn release(&mut self) -> Result<()> {
        self.io.release().await
    }
    fn trace(&self) -> Option<Trace> {
        self.trace.clone()
    }
    fn endpoint(&self, stage: TransferStage) -> Option<u8> {
        self.io.endpoint(stage)
    }
    async fn control(&mut self, kind: RequestKind, bytes: [u8; 12]) -> Result<()> {
        let span = self
            .trace
            .as_ref()
            .map(|t| t.start(TransferStage::Control(kind), Some(0), 12, &bytes));
        let result = self.io.control(kind, bytes).await;
        if let Some(span) = span {
            span.complete(&result, 12, &[]);
        }
        result
    }
    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let span = self.trace.as_ref().map(|t| {
            t.start(
                TransferStage::BulkOut,
                self.io.endpoint(TransferStage::BulkOut),
                bytes.len(),
                bytes,
            )
        });
        let result = self.io.write(bytes).await;
        if let Some(span) = span {
            span.complete(&result, bytes.len(), &[]);
        }
        result
    }
    async fn read(&mut self, maximum: usize) -> Result<Vec<u8>> {
        let span = self.trace.as_ref().map(|t| {
            t.start(
                TransferStage::BulkIn,
                self.io.endpoint(TransferStage::BulkIn),
                maximum,
                &[],
            )
        });
        let result = self.io.read(maximum).await;
        if let Some(span) = span {
            let data = result.as_deref().unwrap_or(&[]);
            span.complete(&result, data.len(), data);
        }
        result
    }
}

#[cfg(test)]
mod tests;
