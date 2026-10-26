//! Python bindings for the bundled reference catalog and the corpus index.
use pyo3::{
    exceptions::PyValueError,
    prelude::*,
    types::{PyDict, PyList, PyModule},
};

mod corpus;

/// Resolve a checked body and firmware selection from the bundled catalog.
#[pyfunction]
fn resolve_reference(body_id: &str, firmware_id: &str) -> PyResult<String> {
    let selection = fujimod_catalog::ReferenceCatalog::bundled()
        .and_then(|catalog| catalog.resolve(body_id, firmware_id))
        .map_err(PyValueError::new_err)?;
    serde_json::to_string(&selection).map_err(|error| PyValueError::new_err(error.to_string()))
}

/// Resolve a checked firmware selection and its body from the bundled catalog.
#[pyfunction]
fn resolve_firmware(firmware_id: &str) -> PyResult<String> {
    let selection = fujimod_catalog::ReferenceCatalog::bundled()
        .and_then(|catalog| catalog.resolve_firmware(firmware_id))
        .map_err(PyValueError::new_err)?;
    serde_json::to_string(&selection).map_err(|error| PyValueError::new_err(error.to_string()))
}

/// Resolve and validate a named run configuration and all reference identities.
#[pyfunction]
#[pyo3(signature = (configuration_id, overlays=Vec::new()))]
fn resolve_configuration(configuration_id: &str, overlays: Vec<String>) -> PyResult<String> {
    let resolved =
        fujimod_catalog_resolution::ResolvedConfiguration::with_overlays(configuration_id, &overlays)
            .map_err(PyValueError::new_err)?;
    serde_json::to_string(&resolved).map_err(|error| PyValueError::new_err(error.to_string()))
}

/// Add `corpus` and `fujimod.catalog` with its `service` submodule to `parent`.
pub fn register(py: Python<'_>, parent: &Bound<'_, PyModule>) -> PyResult<()> {
    corpus::register(py, parent)?;
    let catalog = PyModule::new(py, "fujimod.catalog")?;
    catalog.add_function(wrap_pyfunction!(resolve_reference, &catalog)?)?;
    catalog.add_function(wrap_pyfunction!(resolve_firmware, &catalog)?)?;
    catalog.add_function(wrap_pyfunction!(resolve_configuration, &catalog)?)?;
    catalog.add("__path__", PyList::empty(py))?;
    let service = fujimod_python_service::register(py, &catalog)?;
    parent.add_submodule(&catalog)?;
    let modules = py
        .import("sys")?
        .getattr("modules")?
        .cast_into::<PyDict>()?;
    modules.set_item("fujimod.catalog", &catalog)?;
    modules.set_item("fujimod.catalog.service", &service)?;
    Ok(())
}
