//! Explicit language boundary for unclassified ff80 requests.
use super::*;

#[pyclass(frozen, eq, from_py_object, module = "fujimod.usb.research")]
#[derive(Clone, Copy, Eq, PartialEq)]
struct CommandCode {
    inner: usb::ff80::commands::research::CommandCode,
}
#[pymethods]
impl CommandCode {
    #[new]
    fn new(value: u32) -> Self {
        Self {
            inner: usb::ff80::commands::research::CommandCode(value),
        }
    }
    #[getter]
    fn value(&self) -> u32 {
        self.inner.0
    }
}

#[pyclass(frozen, from_py_object, module = "fujimod.usb.research")]
#[derive(Clone)]
pub(super) struct Request {
    pub(super) inner: usb::ff80::commands::research::Request,
}
#[pymethods]
impl Request {
    #[new]
    #[pyo3(signature=(code,body,*,reply_capacity))]
    fn new(
        py: Python<'_>,
        code: &CommandCode,
        body: Vec<u8>,
        reply_capacity: u32,
    ) -> PyResult<Self> {
        usb::ff80::commands::research::Request::new(code.inner, body, nz(reply_capacity)?)
            .map(|inner| Self { inner })
            .map_err(|e| convert_error(py, e))
    }
    #[getter]
    fn code(&self) -> CommandCode {
        CommandCode {
            inner: self.inner.code(),
        }
    }
    #[getter]
    fn body<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.inner.body())
    }
    #[getter]
    fn reply_capacity(&self) -> u32 {
        self.inner.reply_capacity().get()
    }
}

#[pyclass(frozen, module = "fujimod.usb.research")]
pub(super) struct Reply {
    pub(super) inner: usb::ff80::commands::research::Reply,
}
#[pymethods]
impl Reply {
    #[getter]
    fn code(&self) -> CommandCode {
        CommandCode {
            inner: self.inner.code,
        }
    }
    #[getter]
    fn status(&self) -> CommandStatus {
        match self.inner.status {
            diagnostic::CommandStatus::Success => CommandStatus::Success,
            diagnostic::CommandStatus::DeviceError { .. } => CommandStatus::DeviceError,
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
    fn body<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.inner.body())
    }
    #[getter]
    fn raw<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.inner.raw())
    }
}

pub(super) fn module(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    let module = PyModule::new(py, "fujimod.usb.research")?;
    module.add_class::<CommandCode>()?;
    module.add_class::<Request>()?;
    module.add_class::<Reply>()?;
    Ok(module)
}
