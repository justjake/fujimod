//! Bounded transfer logs, drained separately from command ownership.
use super::*;
use error::{bytes, nz};
use usb::{error::TransferStage, ff80::framing::RequestKind, trace};
#[wasm_bindgen]
#[derive(Clone)]
pub struct Trace {
    pub(crate) inner: trace::Trace,
}
#[wasm_bindgen]
impl Trace {
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(unchecked_param_type = "number")] max_events: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] max_bytes: JsValue,
    ) -> Result<Self, JsValue> {
        let max_events = error::integer(max_events, u32::MAX)?;
        let max_bytes = error::integer(max_bytes, u32::MAX)?;

        Ok(Self {
            inner: trace::Trace::new(nz(max_events)?, nz(max_bytes)?),
        })
    }
    pub fn drain(&self) -> TraceBatch {
        TraceBatch {
            inner: self.inner.drain(),
        }
    }
}
#[wasm_bindgen]
#[derive(Clone, Copy)]
pub enum TraceStage {
    Control,
    BulkIn,
    BulkOut,
}
#[wasm_bindgen]
#[derive(Clone, Copy)]
pub enum TracePhase {
    Submitted,
    Completed,
    Failed,
    Canceled,
}
#[wasm_bindgen]
#[derive(Clone, Copy)]
pub enum TraceRequestKind {
    Open,
    Ping,
    Close,
    WriteCommand,
    ReadCommand,
    WriteData,
    ReadData,
    EmptyResponse,
    Reclaim,
}
impl From<RequestKind> for TraceRequestKind {
    fn from(kind: RequestKind) -> Self {
        match kind {
            RequestKind::Reclaim => Self::Reclaim,
            RequestKind::Open => Self::Open,
            RequestKind::Ping => Self::Ping,
            RequestKind::Close => Self::Close,
            RequestKind::WriteCommand => Self::WriteCommand,
            RequestKind::ReadCommand => Self::ReadCommand,
            RequestKind::WriteData => Self::WriteData,
            RequestKind::ReadData => Self::ReadData,
            RequestKind::EmptyResponse => Self::EmptyResponse,
        }
    }
}

#[wasm_bindgen]
#[derive(Clone, Copy)]
pub enum TraceFailureKind {
    Transfer,
    Timeout,
    ShortTransfer,
    Other,
}
#[wasm_bindgen]
#[derive(Clone, Copy)]
pub enum TraceLayer {
    Ff80,
    App80,
}
#[wasm_bindgen]
pub struct ProtocolTraceEvent {
    inner: trace::ProtocolEvent,
}
#[wasm_bindgen]
impl ProtocolTraceEvent {
    #[wasm_bindgen(getter)]
    pub fn sequence(&self) -> u64 {
        self.inner.sequence
    }
    #[wasm_bindgen(getter)]
    pub fn exchange(&self) -> u64 {
        self.inner.exchange
    }
    #[wasm_bindgen(getter, js_name=timestampMs)]
    pub fn timestamp_ms(&self) -> f64 {
        self.inner
            .timestamp
            .duration_since(std::time::UNIX_EPOCH)
            .expect("JS wall clock epoch")
            .as_secs_f64()
            * 1000.0
    }
    #[wasm_bindgen(getter)]
    pub fn layer(&self) -> TraceLayer {
        match self.inner.layer {
            trace::Layer::Ff80 => TraceLayer::Ff80,
            trace::Layer::App80 => TraceLayer::App80,
        }
    }
    #[wasm_bindgen(getter)]
    pub fn command(&self) -> u32 {
        self.inner.command
    }
    #[wasm_bindgen(getter)]
    pub fn phase(&self) -> TracePhase {
        match self.inner.action {
            trace::Action::Submitted { .. } => TracePhase::Submitted,
            trace::Action::Completed { .. } => TracePhase::Completed,
            trace::Action::Failed { .. } => TracePhase::Failed,
            trace::Action::Canceled => TracePhase::Canceled,
        }
    }
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn data(&self) -> JsValue {
        bytes(self.inner.action.data())
    }
    #[wasm_bindgen(getter,js_name=failureMessage)]
    pub fn failure_message(&self) -> Option<String> {
        match &self.inner.action {
            trace::Action::Failed { failure, .. } => Some(format!("{failure:?}")),
            _ => None,
        }
    }
}

#[wasm_bindgen]
pub struct TraceBatch {
    inner: trace::Batch,
}
#[wasm_bindgen]
impl TraceBatch {
    #[wasm_bindgen(getter, js_name=protocolEvents, unchecked_return_type="ProtocolTraceEvent[]")]
    pub fn protocol_events(&self) -> js_sys::Array {
        self.inner
            .protocol_events
            .iter()
            .cloned()
            .map(|inner| JsValue::from(ProtocolTraceEvent { inner }))
            .collect()
    }
    #[wasm_bindgen(getter,js_name=droppedEvents)]
    pub fn dropped_events(&self) -> u64 {
        self.inner.dropped_events
    }
    #[wasm_bindgen(getter,js_name=droppedBytes)]
    pub fn dropped_bytes(&self) -> u64 {
        self.inner.dropped_bytes
    }
    #[wasm_bindgen(getter, unchecked_return_type = "TraceEvent[]")]
    pub fn events(&self) -> js_sys::Array {
        self.inner
            .events
            .iter()
            .cloned()
            .map(|inner| JsValue::from(TraceEvent { inner }))
            .collect()
    }
}
#[wasm_bindgen]
pub struct TraceEvent {
    inner: trace::Event,
}
#[wasm_bindgen]
impl TraceEvent {
    #[wasm_bindgen(getter)]
    pub fn sequence(&self) -> u64 {
        self.inner.sequence
    }
    #[wasm_bindgen(getter)]
    pub fn transfer(&self) -> u64 {
        self.inner.transfer
    }
    #[wasm_bindgen(getter,js_name=timestampMs)]
    pub fn timestamp_ms(&self) -> f64 {
        self.inner
            .timestamp
            .duration_since(std::time::UNIX_EPOCH)
            .expect("JS wall clock epoch")
            .as_secs_f64()
            * 1000.0
    }
    #[wasm_bindgen(getter)]
    pub fn endpoint(&self) -> Option<u8> {
        self.inner.endpoint
    }
    #[wasm_bindgen(getter)]
    pub fn stage(&self) -> TraceStage {
        match self.inner.stage {
            TransferStage::Control(_) => TraceStage::Control,
            TransferStage::BulkIn => TraceStage::BulkIn,
            TransferStage::BulkOut => TraceStage::BulkOut,
        }
    }
    #[wasm_bindgen(getter)]
    pub fn phase(&self) -> TracePhase {
        match self.inner.action {
            trace::Action::Submitted { .. } => TracePhase::Submitted,
            trace::Action::Completed { .. } => TracePhase::Completed,
            trace::Action::Failed { .. } => TracePhase::Failed,
            trace::Action::Canceled => TracePhase::Canceled,
        }
    }
    #[wasm_bindgen(getter,js_name=requestKind)]
    pub fn request_kind(&self) -> Option<TraceRequestKind> {
        match self.inner.stage {
            TransferStage::Control(kind) => Some(kind.into()),
            _ => None,
        }
    }
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn data(&self) -> JsValue {
        bytes(self.inner.action.data())
    }
    #[wasm_bindgen(getter)]
    pub fn requested(&self) -> Option<u32> {
        match self.inner.action {
            trace::Action::Submitted { requested, .. } => Some(requested as u32),
            _ => None,
        }
    }
    #[wasm_bindgen(getter)]
    pub fn transferred(&self) -> Option<u32> {
        match self.inner.action {
            trace::Action::Completed { transferred, .. } => Some(transferred as u32),
            trace::Action::Failed { transferred, .. } => transferred.map(|n| n as u32),
            _ => None,
        }
    }
    #[wasm_bindgen(getter,js_name=failureKind)]
    pub fn failure_kind(&self) -> Option<TraceFailureKind> {
        match &self.inner.action {
            trace::Action::Failed { failure, .. } => Some(match failure {
                trace::Failure::Transfer(_) => TraceFailureKind::Transfer,
                trace::Failure::Timeout => TraceFailureKind::Timeout,
                trace::Failure::ShortTransfer { .. } => TraceFailureKind::ShortTransfer,
                trace::Failure::Other(_) => TraceFailureKind::Other,
            }),
            _ => None,
        }
    }
    #[wasm_bindgen(getter,js_name=failureMessage)]
    pub fn failure_message(&self) -> Option<String> {
        match &self.inner.action {
            trace::Action::Failed { failure, .. } => Some(format!("{failure:?}")),
            _ => None,
        }
    }
}
