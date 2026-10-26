//! Language conversion for App80 service definitions; no USB operations.
use fujimod_catalog_firmware_identity::IdentityError;
use fujimod_catalog_service as core;
use pyo3::{
    exceptions::PyValueError,
    prelude::*,
    types::{PyModule, PyTuple},
};

include!(concat!(env!("OUT_DIR"), "/definition_codec_python.rs"));
pyo3::create_exception!(fujimod.catalog, DefinitionError, PyValueError);

#[pyclass(frozen, from_py_object, module = "fujimod.catalog")]
#[derive(Clone)]
enum DefinitionFailure {
    Json { line: usize, column: usize },
    Schema { schema: u32 },
    EmptyIdentity {},
    InvalidDigest {},
    FirmwareMismatch {},
    EmptyModes {},
    InvalidMailbox {},
    InvalidLayout { code: u8, codec: Codec },
    SourceCount {},
    EmptyLabel {},
}
fn invalid(error: core::DefinitionError) -> PyErr {
    let exception = DefinitionError::new_err(error.to_string());
    let reason = match error {
        core::DefinitionError::Json(error) => DefinitionFailure::Json {
            line: error.line(),
            column: error.column(),
        },
        core::DefinitionError::Schema(schema) => DefinitionFailure::Schema { schema },
        core::DefinitionError::Identity(IdentityError::EmptyIdentity) => {
            DefinitionFailure::EmptyIdentity {}
        }
        core::DefinitionError::Identity(IdentityError::InvalidDigest) => {
            DefinitionFailure::InvalidDigest {}
        }
        core::DefinitionError::Identity(IdentityError::FirmwareMismatch) => {
            DefinitionFailure::FirmwareMismatch {}
        }
        core::DefinitionError::EmptyModes => DefinitionFailure::EmptyModes {},
        core::DefinitionError::InvalidMailbox => DefinitionFailure::InvalidMailbox {},
        core::DefinitionError::InvalidLayout { code, codec } => DefinitionFailure::InvalidLayout {
            code,
            codec: codec.into(),
        },
        core::DefinitionError::SourceCount => DefinitionFailure::SourceCount {},
        core::DefinitionError::EmptyLabel => DefinitionFailure::EmptyLabel {},
    };
    Python::attach(|py| match exception.value(py).setattr("reason", reason) {
        Ok(()) => exception,
        Err(error) => error,
    })
}

#[pyclass(frozen, from_py_object, module = "fujimod.catalog")]
#[derive(Clone)]
pub struct FirmwareIdentity {
    pub inner: fujimod_catalog_firmware_identity::FirmwareIdentity,
}
#[pymethods]
impl FirmwareIdentity {
    #[new]
    fn new(id: String, sha256: &str) -> PyResult<Self> {
        fujimod_catalog_firmware_identity::FirmwareIdentity::new(id, sha256)
            .map(|inner| Self { inner })
            .map_err(|error| invalid(error.into()))
    }
    #[getter]
    fn id(&self) -> &str {
        self.inner.id()
    }
    #[getter]
    fn sha256(&self) -> String {
        self.inner.digest().hex()
    }
}

#[pyclass(frozen, eq, from_py_object, module = "fujimod.catalog.service")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceModes {
    pub inner: core::ServiceModes,
}
#[pymethods]
impl ServiceModes {
    #[new]
    fn new(mask: u32) -> PyResult<Self> {
        core::ServiceModes::new(mask)
            .map(|inner| Self { inner })
            .map_err(invalid)
    }
    #[getter]
    fn mask(&self) -> u32 {
        self.inner.mask()
    }
}

#[pyclass(frozen, from_py_object, module = "fujimod.catalog.service")]
#[derive(Clone)]
enum ModeAvailability {
    Modes { modes: ServiceModes },
    Disabled {},
    Unreviewed {},
}
impl From<core::ModeAvailability> for ModeAvailability {
    fn from(value: core::ModeAvailability) -> Self {
        match value {
            core::ModeAvailability::Modes(inner) => Self::Modes {
                modes: ServiceModes { inner },
            },
            core::ModeAvailability::Disabled => Self::Disabled {},
            core::ModeAvailability::Unreviewed => Self::Unreviewed {},
        }
    }
}
#[pyclass(frozen, from_py_object, module = "fujimod.catalog.service")]
#[derive(Clone)]
enum ModeTransition {
    Keep {},
    Set { modes: ServiceModes },
}
impl From<core::ModeTransition> for ModeTransition {
    fn from(value: core::ModeTransition) -> Self {
        match value {
            core::ModeTransition::Keep => Self::Keep {},
            core::ModeTransition::Set(inner) => Self::Set {
                modes: ServiceModes { inner },
            },
        }
    }
}
#[pyclass(frozen, module = "fujimod.catalog.service")]
struct Entry {
    inner: core::Entry,
}
#[pymethods]
impl Entry {
    #[getter]
    fn code(&self) -> u8 {
        self.inner.code()
    }
    #[getter]
    fn label(&self) -> &str {
        self.inner.label()
    }
    #[getter]
    fn codec(&self) -> Codec {
        self.inner.codec().into()
    }
    #[getter]
    fn slots<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, self.inner.slots().iter().copied())
    }
    #[getter]
    fn enabled_modes(&self) -> ModeAvailability {
        self.inner.enabled_modes().into()
    }
    #[getter]
    fn transition(&self) -> ModeTransition {
        self.inner.transition().into()
    }
    #[getter]
    fn firmware_kind(&self) -> u32 {
        self.inner.firmware_kind()
    }
    #[getter]
    fn handler(&self) -> u64 {
        self.inner.handler()
    }
    #[getter]
    fn parameter_table(&self) -> u64 {
        self.inner.parameter_table()
    }
}
#[pyclass(frozen, module = "fujimod.catalog.service")]
struct Mailbox {
    inner: core::Mailbox,
}
#[pymethods]
impl Mailbox {
    #[getter]
    fn page(&self) -> u32 {
        self.inner.page()
    }
    #[getter]
    fn offset(&self) -> u32 {
        self.inner.offset()
    }
    #[getter]
    fn configuration_limit(&self) -> u32 {
        self.inner.configuration_limit().get()
    }
}
#[pyclass(frozen, from_py_object, module = "fujimod.catalog.service")]
#[derive(Clone)]
pub struct Definition {
    pub inner: core::Definition,
}
#[pymethods]
impl Definition {
    #[staticmethod]
    fn from_json(json: &str, expected: &FirmwareIdentity) -> PyResult<Self> {
        core::Definition::from_json(json, &expected.inner)
            .map(|inner| Self { inner })
            .map_err(invalid)
    }
    #[getter]
    fn identity(&self) -> FirmwareIdentity {
        FirmwareIdentity {
            inner: self.inner.identity().clone(),
        }
    }
    #[getter]
    fn model(&self) -> &str {
        self.inner.model()
    }
    #[getter]
    fn version(&self) -> &str {
        self.inner.version()
    }
    #[getter]
    fn mailbox(&self) -> Mailbox {
        Mailbox {
            inner: self.inner.mailbox(),
        }
    }
    #[getter]
    fn entries(&self) -> Vec<Entry> {
        self.inner
            .entries()
            .iter()
            .cloned()
            .map(|inner| Entry { inner })
            .collect()
    }
    fn select(&self, code: u8, modes: &ServiceModes) -> Option<Entry> {
        self.inner
            .select(code, modes.inner)
            .cloned()
            .map(|inner| Entry { inner })
    }
}

/// Add the definition identity and errors to `catalog`, with its `service` submodule.
pub fn register<'py>(
    py: Python<'py>,
    catalog: &Bound<'py, PyModule>,
) -> PyResult<Bound<'py, PyModule>> {
    catalog.add_class::<FirmwareIdentity>()?;
    catalog.add("DefinitionError", py.get_type::<DefinitionError>())?;
    catalog.add_class::<DefinitionFailure>()?;
    let service = PyModule::new(py, "fujimod.catalog.service")?;
    service.add_class::<ServiceModes>()?;
    service.add_class::<Codec>()?;
    service.add_class::<ModeAvailability>()?;
    service.add_class::<ModeTransition>()?;
    service.add_class::<Entry>()?;
    service.add_class::<Mailbox>()?;
    service.add_class::<Definition>()?;
    catalog.add_submodule(&service)?;
    Ok(service)
}
