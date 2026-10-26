//! Enum conversion and Python adapters for Rust service codec values.
use super::*;
use fujimod_python_service::{Codec, Definition, ServiceModes};
use fujimod_usb_app80_types::catalog as core;
mod values {
    include!(concat!(env!("OUT_DIR"), "/service_values_python.rs"));
}
use values::*;
pyo3::create_exception!(fujimod.usb.app80, CommandError, PyValueError);

/// A malformed reply is a USB protocol error. This crate builds without the
/// USB bindings, so it raises the class they registered as `fujimod.usb.ProtocolError`.
fn protocol_error(py: Python<'_>, message: String) -> PyErr {
    match py
        .import("fujimod.usb")
        .and_then(|usb| usb.getattr("ProtocolError"))
        .and_then(|class| Ok(class.cast_into::<pyo3::types::PyType>()?))
    {
        Ok(class) => PyErr::from_type(class, message),
        Err(error) => error,
    }
}

#[pyclass(frozen, from_py_object, module = "fujimod.usb.app80")]
#[derive(Clone)]
enum CommandFailure {
    Unsupported { codec: Codec },
    Ambiguous { codec: Codec },
    Unavailable { code: u8 },
    Argument { kind: ArgumentKind },
    OversizeArguments {},
}
fn command_error(py: Python<'_>, error: core::CommandError) -> PyErr {
    let exception = CommandError::new_err(error.to_string());
    let reason = match error {
        core::CommandError::Unsupported(codec) => CommandFailure::Unsupported {
            codec: codec.into(),
        },
        core::CommandError::Ambiguous(codec) => CommandFailure::Ambiguous {
            codec: codec.into(),
        },
        core::CommandError::Unavailable(code) => CommandFailure::Unavailable { code: code.0 },
        core::CommandError::Argument(kind) => CommandFailure::Argument { kind: kind.into() },
        core::CommandError::OversizeArguments => CommandFailure::OversizeArguments {},
    };
    match exception.value(py).setattr("reason", reason) {
        Ok(()) => exception,
        Err(error) => error,
    }
}

#[pyclass(frozen, module = "fujimod.usb.app80")]
pub struct CommandReply {
    pub inner: core::CommandReply,
}
#[pymethods]
impl CommandReply {
    #[getter]
    fn value(&self) -> ReplyValue {
        self.inner.value().clone().into()
    }
    #[getter]
    fn raw<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.inner.raw().raw())
    }
}
#[pyclass(frozen, from_py_object, module = "fujimod.usb.app80")]
#[derive(Clone)]
pub struct Invocation {
    pub prepared: core::PreparedCommand,
    pub page: u32,
    pub offset: u32,
    pub configuration_limit: u32,
}
#[pymethods]
impl Invocation {
    #[getter]
    fn raw<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.prepared.request().encode())
    }
    #[getter]
    fn configuration_limit(&self) -> u32 {
        self.configuration_limit
    }
    fn decode(&self, py: Python<'_>, raw: Vec<u8>) -> PyResult<CommandReply> {
        self.prepared
            .decode_bytes(&raw)
            .map(|inner| CommandReply { inner })
            .map_err(|e| protocol_error(py, e.to_string()))
    }
}
#[pyclass(frozen, module = "fujimod.usb.app80")]
struct Service {
    catalog: core::Catalog,
    page: u32,
    offset: u32,
    configuration_limit: u32,
}
#[pymethods]
impl Service {
    #[new]
    fn new(definition: &Definition) -> PyResult<Self> {
        let mailbox = definition.inner.mailbox();
        Ok(Self {
            catalog: core::Catalog::new(definition.inner.clone()),
            page: mailbox.page(),
            offset: mailbox.offset(),
            configuration_limit: mailbox.configuration_limit().get(),
        })
    }
    fn prepare(
        &self,
        py: Python<'_>,
        modes: &ServiceModes,
        command: Command,
    ) -> PyResult<Invocation> {
        self.catalog
            .prepare(modes.inner, command.into())
            .map(|prepared| Invocation {
                prepared,
                page: self.page,
                offset: self.offset,
                configuration_limit: self.configuration_limit,
            })
            .map_err(|e| command_error(py, e))
    }
    fn prepare_code(
        &self,
        py: Python<'_>,
        code: &CommandCode,
        modes: &ServiceModes,
        command: Command,
    ) -> PyResult<Invocation> {
        self.catalog
            .prepare_code(code.inner, modes.inner, command.into())
            .map(|prepared| Invocation {
                prepared,
                page: self.page,
                offset: self.offset,
                configuration_limit: self.configuration_limit,
            })
            .map_err(|e| command_error(py, e))
    }
    fn prepare_research(
        &self,
        py: Python<'_>,
        code: &CommandCode,
        modes: &ServiceModes,
        arguments: Vec<u8>,
    ) -> PyResult<Invocation> {
        self.catalog
            .prepare_research(code.inner, modes.inner, &arguments)
            .map(|prepared| Invocation {
                prepared,
                page: self.page,
                offset: self.offset,
                configuration_limit: self.configuration_limit,
            })
            .map_err(|e| command_error(py, e))
    }
}
pub(super) fn register(py: Python<'_>, module: &Bound<'_, PyModule>) -> PyResult<()> {
    values::register(module)?;
    module.add("CommandError", py.get_type::<CommandError>())?;

    module.add_class::<CommandFailure>()?;

    module.add_class::<CommandReply>()?;
    module.add_class::<Invocation>()?;
    module.add_class::<Service>()?;
    Ok(())
}
