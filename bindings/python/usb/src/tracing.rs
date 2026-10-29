//! Pull-only transfer tracing; USB work never calls Python callbacks.
use super::*;
use usb::{
    error::TransferStage,
    trace::{Action, Failure},
};

#[pyclass(frozen, eq, hash, from_py_object, module = "fujimod.usb.tracing")]
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
enum Stage {
    Control,
    BulkIn,
    BulkOut,
}
#[pyclass(frozen, eq, hash, from_py_object, module = "fujimod.usb.tracing")]
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
enum Phase {
    Submitted,
    Completed,
    Failed,
    Canceled,
}
#[pyclass(frozen, eq, hash, from_py_object, module = "fujimod.usb.tracing")]
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
enum FailureKind {
    Transfer,
    Timeout,
    ShortTransfer,
    Other,
}
#[pyclass(frozen, eq, hash, from_py_object, module = "fujimod.usb.tracing")]
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
enum RequestKind {
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
impl From<ff80::framing::RequestKind> for RequestKind {
    fn from(value: ff80::framing::RequestKind) -> Self {
        use ff80::framing::RequestKind as K;
        match value {
            K::Reclaim => Self::Reclaim,
            K::Open => Self::Open,
            K::Ping => Self::Ping,
            K::Close => Self::Close,
            K::WriteCommand => Self::WriteCommand,
            K::ReadCommand => Self::ReadCommand,
            K::WriteData => Self::WriteData,
            K::ReadData => Self::ReadData,
            K::EmptyResponse => Self::EmptyResponse,
        }
    }
}

#[pyclass(frozen, module = "fujimod.usb.tracing")]
struct Event {
    inner: usb::trace::Event,
}
#[pymethods]
impl Event {
    #[getter]
    fn sequence(&self) -> u64 {
        self.inner.sequence
    }
    #[getter]
    fn timestamp(&self) -> f64 {
        match self.inner.timestamp.duration_since(std::time::UNIX_EPOCH) {
            Ok(duration) => duration.as_secs_f64(),
            Err(error) => -error.duration().as_secs_f64(),
        }
    }
    #[getter]
    fn transfer(&self) -> u64 {
        self.inner.transfer
    }
    #[getter]
    fn stage(&self) -> Stage {
        match self.inner.stage {
            TransferStage::Control(_) => Stage::Control,
            TransferStage::BulkIn => Stage::BulkIn,
            TransferStage::BulkOut => Stage::BulkOut,
        }
    }
    #[getter]
    fn endpoint(&self) -> Option<u8> {
        self.inner.endpoint
    }
    #[getter]
    fn request_kind(&self) -> Option<RequestKind> {
        match self.inner.stage {
            TransferStage::Control(k) => Some(k.into()),
            _ => None,
        }
    }
    #[getter]
    fn phase(&self) -> Phase {
        match self.inner.action {
            Action::Submitted { .. } => Phase::Submitted,
            Action::Completed { .. } => Phase::Completed,
            Action::Failed { .. } => Phase::Failed,
            Action::Canceled => Phase::Canceled,
        }
    }
    #[getter]
    fn requested(&self) -> Option<usize> {
        match self.inner.action {
            Action::Submitted { requested, .. } => Some(requested),
            _ => None,
        }
    }
    #[getter]
    fn transferred(&self) -> Option<usize> {
        match self.inner.action {
            Action::Completed { transferred, .. } => Some(transferred),
            Action::Failed { transferred, .. } => transferred,
            _ => None,
        }
    }
    #[getter]
    fn data<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.inner.action.data())
    }
    #[getter]
    fn failure_kind(&self) -> Option<FailureKind> {
        match &self.inner.action {
            Action::Failed { failure, .. } => Some(match failure {
                Failure::Transfer(_) => FailureKind::Transfer,
                Failure::Timeout => FailureKind::Timeout,
                Failure::ShortTransfer { .. } => FailureKind::ShortTransfer,
                Failure::Other(_) => FailureKind::Other,
            }),
            _ => None,
        }
    }
    #[getter]
    fn error(&self) -> Option<String> {
        match &self.inner.action {
            Action::Failed { failure, .. } => Some(format!("{failure:?}")),
            _ => None,
        }
    }
}
#[pyclass(frozen, eq, hash, from_py_object, module = "fujimod.usb.tracing")]
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
enum Layer {
    Ff80,
    App80,
}
#[pyclass(frozen, module = "fujimod.usb.tracing")]
struct ProtocolEvent {
    inner: usb::trace::ProtocolEvent,
}
#[pymethods]
impl ProtocolEvent {
    #[getter]
    fn sequence(&self) -> u64 {
        self.inner.sequence
    }
    #[getter]
    fn timestamp(&self) -> f64 {
        self.inner
            .timestamp
            .duration_since(std::time::UNIX_EPOCH)
            .expect("trace epoch")
            .as_secs_f64()
    }
    #[getter]
    fn exchange(&self) -> u64 {
        self.inner.exchange
    }
    #[getter]
    fn layer(&self) -> Layer {
        match self.inner.layer {
            usb::trace::Layer::Ff80 => Layer::Ff80,
            usb::trace::Layer::App80 => Layer::App80,
        }
    }
    #[getter]
    fn command(&self) -> u32 {
        self.inner.command
    }
    #[getter]
    fn phase(&self) -> Phase {
        match self.inner.action {
            Action::Submitted { .. } => Phase::Submitted,
            Action::Completed { .. } => Phase::Completed,
            Action::Failed { .. } => Phase::Failed,
            Action::Canceled => Phase::Canceled,
        }
    }
    #[getter]
    fn data<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.inner.action.data())
    }
    #[getter]
    fn error(&self) -> Option<String> {
        match &self.inner.action {
            Action::Failed { failure, .. } => Some(format!("{failure:?}")),
            _ => None,
        }
    }
}
#[pyclass(frozen, module = "fujimod.usb.tracing")]
struct Batch {
    events: Vec<Py<Event>>,
    protocol_events: Vec<Py<ProtocolEvent>>,
    #[pyo3(get)]
    dropped_events: u64,
    #[pyo3(get)]
    dropped_bytes: u64,
}
#[pymethods]
impl Batch {
    #[getter]
    fn protocol_events(&self, py: Python<'_>) -> Vec<Py<ProtocolEvent>> {
        self.protocol_events
            .iter()
            .map(|e| e.clone_ref(py))
            .collect()
    }
    #[getter]
    fn events(&self, py: Python<'_>) -> Vec<Py<Event>> {
        self.events.iter().map(|e| e.clone_ref(py)).collect()
    }
}
#[pyclass(frozen, from_py_object, module = "fujimod.usb.tracing")]
#[derive(Clone)]
pub(super) struct Trace {
    pub(super) inner: usb::trace::Trace,
}
#[pymethods]
impl Trace {
    #[new]
    fn new(max_events: u32, max_bytes: u32) -> PyResult<Self> {
        Ok(Self {
            inner: usb::trace::Trace::new(nz(max_events)?, nz(max_bytes)?),
        })
    }
    fn drain(&self, py: Python<'_>) -> PyResult<Batch> {
        let batch = self.inner.drain();
        let events = batch
            .events
            .into_iter()
            .map(|inner| Py::new(py, Event { inner }))
            .collect::<PyResult<Vec<_>>>()?;
        let protocol_events = batch
            .protocol_events
            .into_iter()
            .map(|inner| Py::new(py, ProtocolEvent { inner }))
            .collect::<PyResult<Vec<_>>>()?;
        Ok(Batch {
            events,
            protocol_events,
            dropped_events: batch.dropped_events,
            dropped_bytes: batch.dropped_bytes,
        })
    }
}
pub(super) fn module(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    let module = PyModule::new(py, "fujimod.usb.tracing")?;
    module.add_class::<Trace>()?;
    module.add_class::<Layer>()?;
    module.add_class::<ProtocolEvent>()?;
    module.add_class::<Event>()?;
    module.add_class::<Batch>()?;
    module.add_class::<Stage>()?;
    module.add_class::<Phase>()?;
    module.add_class::<FailureKind>()?;
    module.add_class::<RequestKind>()?;
    Ok(module)
}
