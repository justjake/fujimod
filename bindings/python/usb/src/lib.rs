//! Python packaging and language conversion only; all USB framing lives in Rust.
use fujimod_python_usb_base::{
    ClosedSessionError, CompletionUnknownError, DeviceCommandError, ProtocolError,
    SessionBusyError, TransferError, UsbError, convert_error, nz,
};
use fujimod_usb::{
    self as usb,
    connection::{Connection, TransferOptions},
    ff80::{
        self,
        commands::{
            capabilities,
            configuration::{self, ConfigurationLocation, ReadConfiguration, WriteConfiguration},
            debug, diagnostic, info,
            memory::{Address, DebugMemoryLimits, MemoryRange, ReadMemory, WriteMemory},
        },
    },
};
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
    types::{PyBytes, PyDict, PyList, PyModule},
};
use std::{
    num::NonZeroU32,
    sync::{Arc, Mutex},
    time::Duration,
};

macro_rules! enum_binding {
    ($name:ident,$python:literal,$target:ty,{$($variant:ident),+ $(,)?}) => {
        #[pyclass(name=$python, module="fujimod.usb", eq, from_py_object)]
        #[derive(Clone,Copy,Debug,PartialEq)]
        enum $name { $($variant),+ }
        impl From<$name> for $target { fn from(value:$name)->Self { match value { $($name::$variant => <$target>::$variant),+ } } }
    };
}
enum_binding!(InfoSelector,"InfoSelector",info::InfoSelector,{Vendor,Manufacturer,Product,InternalVersion,DeviceType,Serial,Framework});
enum_binding!(ConfigurationView,"ConfigurationView",configuration::ConfigurationView,{Storage,Ram});
enum_binding!(KeyState,"KeyState",debug::KeyState,{On,Off});
enum_binding!(CameraMode,"CameraMode",debug::CameraMode,{Record,Playback});
enum_binding!(DataReadMode,"DataReadMode",diagnostic::DataReadMode,{Pattern,Echo});
enum_binding!(DataWriteMode,"DataWriteMode",diagnostic::DataWriteMode,{Preserve,FillOnes,Echo});
enum_binding!(OffsetMode,"OffsetMode",usb::operations::data::OffsetMode,{Checked,Wrapping});
enum_binding!(DeviceFilter,"DeviceFilter",usb::connection::DeviceFilter,{Fujifilm,Diagnostic});

#[pyclass(module = "fujimod.usb", eq, from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq)]
enum SessionState {
    Ready,
    InFlight,
    CompletionUnknown,
    Closed,
    Abandoned,
    ReadingRange,
}
#[pyclass(module = "fujimod.usb", eq, from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq)]
enum CommandStatus {
    Success,
    DeviceError,
}
#[pyclass(module = "fujimod.usb", eq, from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq)]
enum ConsoleProgress {
    Complete,
    Pending,
    Unknown,
}

#[pyclass(frozen, module = "fujimod.usb", from_py_object)]
#[derive(Clone)]
struct FirmwareLimits {
    memory: DebugMemoryLimits,
    configuration: NonZeroU32,
    console: NonZeroU32,
}
#[pymethods]
impl FirmwareLimits {
    #[new]
    #[pyo3(signature=(*,memory_read,memory_write,configuration,console))]
    fn new(
        memory_read: u32,
        memory_write: u32,
        configuration: u32,
        console: u32,
    ) -> PyResult<Self> {
        Ok(Self {
            memory: DebugMemoryLimits {
                read: nz(memory_read)?,
                write: nz(memory_write)?,
            },
            configuration: nz(configuration)?,
            console: nz(console)?,
        })
    }
    #[getter]
    fn memory_read(&self) -> u32 {
        self.memory.read.get()
    }
    #[getter]
    fn memory_write(&self) -> u32 {
        self.memory.write.get()
    }
    #[getter]
    fn configuration(&self) -> u32 {
        self.configuration.get()
    }
    #[getter]
    fn console(&self) -> u32 {
        self.console.get()
    }
}
#[pyclass(frozen, from_py_object, module = "fujimod.usb")]
#[derive(Clone)]
struct InterfaceSelection {
    inner: usb::connection::InterfaceSelection,
}
#[pymethods]
impl InterfaceSelection {
    #[new]
    #[pyo3(signature=(*,configuration,interface,alternate))]
    fn new(configuration: u8, interface: u8, alternate: u8) -> Self {
        Self {
            inner: usb::connection::InterfaceSelection {
                configuration,
                interface,
                alternate,
            },
        }
    }
    #[getter]
    fn configuration(&self) -> u8 {
        self.inner.configuration
    }
    #[getter]
    fn interface(&self) -> u8 {
        self.inner.interface
    }
    #[getter]
    fn alternate(&self) -> u8 {
        self.inner.alternate
    }
}
#[pyclass(frozen, from_py_object, module = "fujimod.usb")]
#[derive(Clone)]
struct Device {
    inner: usb::connection::Device,
}
#[pymethods]
impl Device {
    #[getter]
    fn vendor_id(&self) -> u16 {
        self.inner.vendor_id()
    }
    #[getter]
    fn product_id(&self) -> u16 {
        self.inner.product_id()
    }
    #[getter]
    fn bus_id(&self) -> &str {
        self.inner.bus_id()
    }
    #[getter]
    fn address(&self) -> u8 {
        self.inner.device_address()
    }
    #[getter]
    fn ports(&self) -> Vec<u8> {
        self.inner.port_chain().to_vec()
    }
    #[getter]
    fn manufacturer(&self) -> Option<&str> {
        self.inner.manufacturer_string()
    }
    #[getter]
    fn product(&self) -> Option<&str> {
        self.inner.product_string()
    }
    #[getter]
    fn serial(&self) -> Option<&str> {
        self.inner.serial_number()
    }
}

#[pyclass(frozen, from_py_object, module = "fujimod.usb")]
#[derive(Clone)]
struct DiagnosticTag {
    inner: diagnostic::DiagnosticTag,
}
#[pymethods]
impl DiagnosticTag {
    #[staticmethod]
    fn bulk_source() -> Self {
        Self {
            inner: diagnostic::DiagnosticTag::BulkSource,
        }
    }
    #[staticmethod]
    fn research(value: u32) -> Self {
        Self {
            inner: diagnostic::DiagnosticTag::Research(value),
        }
    }
    #[getter]
    fn value(&self) -> u32 {
        self.inner.value()
    }
}
#[pyclass(frozen, module = "fujimod.usb")]
struct LogReply {
    inner: debug::LogReply,
}
#[pymethods]
impl LogReply {
    #[getter]
    fn status(&self) -> u32 {
        self.inner.status
    }
    #[getter]
    fn data<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.inner.data)
    }
}

#[pyclass(module = "fujimod.usb", eq, from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq)]
enum FirmwareDownloadState {
    Normal,
    Writing,
    Success,
    Failed,
    Unknown,
}

#[pyclass(frozen, module = "fujimod.usb")]
struct FirmwareDownloadReply {
    inner: info::FirmwareDownloadReply,
}
#[pymethods]
impl FirmwareDownloadReply {
    #[getter]
    fn state(&self) -> FirmwareDownloadState {
        match self.inner.state {
            info::FirmwareDownloadState::Normal => FirmwareDownloadState::Normal,
            info::FirmwareDownloadState::Writing => FirmwareDownloadState::Writing,
            info::FirmwareDownloadState::Success => FirmwareDownloadState::Success,
            info::FirmwareDownloadState::Failed { .. } => FirmwareDownloadState::Failed,
            info::FirmwareDownloadState::Unknown(_) => FirmwareDownloadState::Unknown,
        }
    }
    #[getter]
    fn failure_code(&self) -> Option<u8> {
        match self.inner.state {
            info::FirmwareDownloadState::Failed { code } => Some(code),
            _ => None,
        }
    }
    #[getter]
    fn text(&self) -> &str {
        &self.inner.text
    }
    #[getter]
    fn raw<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.inner.raw)
    }
}

#[pyclass(frozen, module = "fujimod.usb")]
struct CapabilitiesReply {
    inner: capabilities::CapabilitiesReply,
}
#[pymethods]
impl CapabilitiesReply {
    #[getter]
    fn byte_length(&self) -> u32 {
        self.inner.byte_length
    }
    #[getter]
    fn raw<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.inner.raw)
    }
}

#[pyclass(frozen, module = "fujimod.usb")]
struct DiagnosticReply {
    inner: diagnostic::DiagnosticReply,
}
#[pymethods]
impl DiagnosticReply {
    #[getter]
    fn status(&self) -> CommandStatus {
        match self.inner.status {
            diagnostic::CommandStatus::Success => CommandStatus::Success,
            _ => CommandStatus::DeviceError,
        }
    }
    #[getter]
    fn primary(&self) -> u32 {
        match self.inner.status {
            diagnostic::CommandStatus::Success => 0,
            diagnostic::CommandStatus::DeviceError { primary, .. } => primary,
        }
    }
    #[getter]
    fn secondary(&self) -> u32 {
        match self.inner.status {
            diagnostic::CommandStatus::Success => 0,
            diagnostic::CommandStatus::DeviceError { secondary, .. } => secondary,
        }
    }
    #[getter]
    fn fields(&self) -> Vec<u32> {
        self.inner.fields.to_vec()
    }
    #[getter]
    fn raw<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.inner.raw)
    }
}
#[pyclass(frozen, module = "fujimod.usb")]
struct ConsoleReply {
    inner: debug::ConsoleReply,
}
#[pymethods]
impl ConsoleReply {
    #[getter]
    fn progress(&self) -> ConsoleProgress {
        match self.inner.progress {
            debug::ConsoleProgress::Complete => ConsoleProgress::Complete,
            debug::ConsoleProgress::Pending => ConsoleProgress::Pending,
            _ => ConsoleProgress::Unknown,
        }
    }
    #[getter]
    fn unknown_progress(&self) -> Option<u32> {
        match self.inner.progress {
            debug::ConsoleProgress::Unknown(n) => Some(n),
            _ => None,
        }
    }
    #[getter]
    fn status(&self) -> u32 {
        self.inner.status
    }
    #[getter]
    fn output<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.inner.output)
    }
}
#[pyclass(frozen, module = "fujimod.usb")]
struct ConfigurationGeometry {
    inner: configuration::ConfigurationGeometry,
}
#[pymethods]
impl ConfigurationGeometry {
    #[getter]
    fn capacity(&self) -> u32 {
        self.inner.capacity
    }
    #[getter]
    fn page_bytes(&self) -> u32 {
        self.inner.page_bytes.get()
    }
    #[getter]
    fn raw<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.inner.raw)
    }
}

enum BindingError {
    Busy,
    Closed,
    Abandoned,
    CompletionUnknown,
    Poisoned,
}
impl BindingError {
    fn into_python(self, _py: Python<'_>) -> PyErr {
        match self {
            BindingError::Busy => {
                SessionBusyError::new_err("session is owned by another operation or range reader")
            }
            BindingError::Closed => ClosedSessionError::new_err("session is closed"),
            BindingError::Abandoned => ClosedSessionError::new_err(
                "session was abandoned without closing the camera session",
            ),
            BindingError::CompletionUnknown => CompletionUnknownError::new_err(
                "session completion is unknown; further I/O is unavailable",
            ),
            BindingError::Poisoned => {
                CompletionUnknownError::new_err("session mutex was poisoned; completion unknown")
            }
        }
    }
}
/// Add `fujimod.firmware` and `fujimod.usb` with its submodules to `m`.
pub fn register(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    asynchronous::install();
    firmware::register(py, m, "fujimod.firmware")?;
    recovery::register(py, m)?;
    let usb = PyModule::new(py, "fujimod.usb")?;
    usb.add("__path__", PyList::empty(py))?;
    usb.add_class::<InfoSelector>()?;
    usb.add_class::<ConfigurationView>()?;
    usb.add_class::<KeyState>()?;
    usb.add_class::<CameraMode>()?;
    usb.add_class::<DataReadMode>()?;
    usb.add_class::<DataWriteMode>()?;
    usb.add_class::<DeviceFilter>()?;
    usb.add_class::<SessionState>()?;
    usb.add_class::<CommandStatus>()?;
    usb.add_class::<ConsoleProgress>()?;
    usb.add_class::<FirmwareLimits>()?;
    usb.add_class::<InterfaceSelection>()?;
    usb.add_class::<Device>()?;
    usb.add_class::<DiagnosticReply>()?;
    usb.add_class::<CapabilitiesReply>()?;
    usb.add_class::<FirmwareDownloadState>()?;
    usb.add_class::<FirmwareDownloadReply>()?;
    usb.add_class::<DiagnosticTag>()?;
    usb.add_class::<LogReply>()?;
    usb.add_class::<ConsoleReply>()?;
    usb.add_class::<ConfigurationGeometry>()?;
    usb.add("UsbError", py.get_type::<UsbError>())?;
    usb.add("TransferError", py.get_type::<TransferError>())?;
    usb.add("ProtocolError", py.get_type::<ProtocolError>())?;
    usb.add(
        "CompletionUnknownError",
        py.get_type::<CompletionUnknownError>(),
    )?;
    usb.add("DeviceCommandError", py.get_type::<DeviceCommandError>())?;
    usb.add("ClosedSessionError", py.get_type::<ClosedSessionError>())?;
    usb.add("SessionBusyError", py.get_type::<SessionBusyError>())?;
    let research = research::module(py)?;
    usb.add_submodule(&research)?;
    let tracing = tracing::module(py)?;
    usb.add_submodule(&tracing)?;
    asynchronous::register(py, &usb)?;
    usb.add_class::<OffsetMode>()?;
    usb.add_class::<data::DataWindow>()?;
    m.add_submodule(&usb)?;
    let modules = py
        .import("sys")?
        .getattr("modules")?
        .cast_into::<PyDict>()?;
    modules.set_item("fujimod.usb", &usb)?;
    modules.set_item("fujimod.usb.research", &research)?;
    modules.set_item("fujimod.usb.tracing", &tracing)?;
    {
        let testing = testing::module(py)?;
        usb.add_submodule(&testing)?;
        modules.set_item("fujimod.usb.testing", &testing)?;
    }
    Ok(())
}

mod asynchronous;
mod data;
mod firmware;
mod recovery;
mod research;
mod tracing;

mod testing;
