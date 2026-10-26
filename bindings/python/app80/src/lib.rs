//! Python USB operations for App80 in `fujimod.usb.app80`.
use fujimod_python_app80_values::CommandCode;
use fujimod_python_app80_values::catalog::{CommandReply, Invocation};
use fujimod_python_usb_base::{self as usb_base, nz};
use pyo3::{
    exceptions::PyValueError,
    prelude::*,
    types::{PyBytes, PyDict, PyModule},
};
use std::num::NonZeroU32;

#[pyclass(frozen, from_py_object, module = "fujimod.usb.app80")]
#[derive(Clone)]
struct Request {
    inner: fujimod_usb_app80_types::Request,
}
#[pymethods]
impl Request {
    #[new]
    fn new(code: &CommandCode, arguments: Vec<u8>) -> PyResult<Self> {
        fujimod_usb_app80_types::Request::new(code.inner, &arguments)
            .map(|inner| Self { inner })
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }
    #[getter]
    fn raw<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.inner.encode())
    }
}

#[pyclass(frozen, from_py_object, module = "fujimod.usb.app80")]
#[derive(Clone)]
struct Mailbox {
    page: u32,
    offset: u32,
    configuration_limit: NonZeroU32,
}
#[pymethods]
impl Mailbox {
    #[new]
    #[pyo3(signature=(*,page,offset,configuration_limit))]
    fn new(page: u32, offset: u32, configuration_limit: u32) -> PyResult<Self> {
        let configuration_limit = nz(configuration_limit)?;
        fujimod_usb_app80::operations::Mailbox::new(page, offset, configuration_limit)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Self {
            page,
            offset,
            configuration_limit,
        })
    }
}

fn configuration_limit(session: &Py<PyAny>) -> PyResult<u32> {
    Python::attach(|py| usb_base::configuration_limit(session.bind(py)))
}

#[pyfunction]
async fn exchange(session: Py<PyAny>, mailbox: Mailbox, request: Request) -> PyResult<Py<PyBytes>> {
    if configuration_limit(&session)? < 33 {
        return Err(PyValueError::new_err(
            "App80 mailbox requires a 33-byte configuration command",
        ));
    }
    let operation = fujimod_usb_app80::operations::Mailbox::new(
        mailbox.page,
        mailbox.offset,
        mailbox.configuration_limit,
    )
    .map_err(|error| PyValueError::new_err(error.to_string()))?;
    let reply = usb_base::with(&session, move |s| {
        Box::pin(async move { operation.exchange(s, &request.inner).await })
    })
    .await?;
    Ok(Python::attach(|py| PyBytes::new(py, reply.raw()).unbind()))
}

#[pyfunction]
async fn execute(session: Py<PyAny>, invocation: Invocation) -> PyResult<Py<PyAny>> {
    let limit = configuration_limit(&session)?;
    check_limit(limit, invocation.prepared.request().encoded_len())?;
    let mailbox = fujimod_usb_app80::operations::Mailbox::new(
        invocation.page,
        invocation.offset,
        nz(invocation.configuration_limit)?,
    )
    .map_err(|error| PyValueError::new_err(error.to_string()))?;
    let operation = fujimod_usb_app80::operations::Invocation::new(invocation.prepared, mailbox);
    let inner = usb_base::with(&session, move |s| {
        Box::pin(async move { operation.execute(s).await })
    })
    .await?;
    Python::attach(|py| Py::new(py, CommandReply { inner }).map(|value| value.into_any()))
}

fn check_limit(configuration_limit: u32, length: usize) -> PyResult<()> {
    if (configuration_limit as usize) < length {
        return Err(PyValueError::new_err(
            "session configuration limit cannot carry this service mailbox",
        ));
    }
    Ok(())
}

/// Add `fujimod.usb.app80` to the `fujimod.usb` module the USB bindings registered on `parent`.
pub fn register(py: Python<'_>, parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let usb = parent.getattr("usb")?.cast_into::<PyModule>()?;
    let module = PyModule::new(py, "fujimod.usb.app80")?;
    module.add_class::<Request>()?;
    module.add_class::<Mailbox>()?;
    fujimod_python_app80_values::register(py, &module)?;
    module.add_function(wrap_pyfunction!(exchange, &module)?)?;
    module.add_function(wrap_pyfunction!(execute, &module)?)?;
    usb.add_submodule(&module)?;
    py.import("sys")?
        .getattr("modules")?
        .cast_into::<PyDict>()?
        .set_item("fujimod.usb.app80", &module)?;
    Ok(())
}
