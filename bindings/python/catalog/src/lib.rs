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

/// ROM start facts for a firmware and the processor window of their boot strap.
#[pyfunction]
fn firmware_rom_start(firmware_id: &str) -> PyResult<Option<String>> {
    let catalog = fujimod_catalog::ReferenceCatalog::bundled().map_err(PyValueError::new_err)?;
    let Some(facts) = catalog.rom_start(firmware_id) else {
        return Ok(None);
    };
    let firmware = catalog
        .firmware(firmware_id)
        .ok_or_else(|| PyValueError::new_err(format!("unknown firmware identity {firmware_id}")))?;
    let body = catalog.body(&firmware.body_id).ok_or_else(|| {
        PyValueError::new_err(format!(
            "ROM start firmware has no body {}",
            firmware.body_id
        ))
    })?;
    let soc = catalog.soc(&body.soc_id).ok_or_else(|| {
        PyValueError::new_err(format!("ROM start body has no processor {}", body.soc_id))
    })?;
    let block = soc
        .blocks
        .iter()
        .find(|block| block.id == facts.boot_strap.block_id)
        .ok_or_else(|| {
            PyValueError::new_err(format!(
                "ROM start facts name missing block {}",
                facts.boot_strap.block_id
            ))
        })?;
    let payload = serde_json::json!({
        "document": facts,
        "boot_strap_block": {"base": block.base, "size": block.size},
    });
    serde_json::to_string(&payload)
        .map(Some)
        .map_err(|error| PyValueError::new_err(error.to_string()))
}

/// NAND boot facts of one boot ROM image, found by its SHA-256.
#[pyfunction]
fn nand_boot_rom(sha256: &str) -> PyResult<Option<String>> {
    let catalog = fujimod_catalog::ReferenceCatalog::bundled().map_err(PyValueError::new_err)?;
    catalog
        .nand_boot_rom(sha256)
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| PyValueError::new_err(error.to_string()))
}

/// Every bundled `restart.generated.json` profile, as a JSON list.
#[pyfunction]
fn restart_profiles() -> PyResult<String> {
    let catalog = fujimod_catalog::ReferenceCatalog::bundled().map_err(PyValueError::new_err)?;
    let profiles: Vec<_> = catalog.restart_profiles().collect();
    serde_json::to_string(&profiles).map_err(|error| PyValueError::new_err(error.to_string()))
}

/// Every optional card-file capability, as a JSON list.
#[pyfunction]
fn card_profiles() -> PyResult<String> {
    let catalog = fujimod_catalog::ReferenceCatalog::bundled().map_err(PyValueError::new_err)?;
    serde_json::to_string(&catalog.card_profiles().collect::<Vec<_>>())
        .map_err(|e| PyValueError::new_err(e.to_string()))
}

/// Resolve and validate a named run configuration and all reference identities.
#[pyfunction]
#[pyo3(signature = (configuration_id, overlays=Vec::new()))]
fn resolve_configuration(configuration_id: &str, overlays: Vec<String>) -> PyResult<String> {
    let resolved = fujimod_catalog_resolution::ResolvedConfiguration::with_overlays(
        configuration_id,
        &overlays,
    )
    .map_err(PyValueError::new_err)?;
    serde_json::to_string(&resolved).map_err(|error| PyValueError::new_err(error.to_string()))
}

/// Add `corpus` and `fujimod.catalog` with its `service` submodule to `parent`.
pub fn register(py: Python<'_>, parent: &Bound<'_, PyModule>) -> PyResult<()> {
    corpus::register(py, parent)?;
    let catalog = PyModule::new(py, "fujimod.catalog")?;
    catalog.add_function(wrap_pyfunction!(resolve_reference, &catalog)?)?;
    catalog.add_function(wrap_pyfunction!(resolve_firmware, &catalog)?)?;
    catalog.add_function(wrap_pyfunction!(firmware_rom_start, &catalog)?)?;
    catalog.add_function(wrap_pyfunction!(nand_boot_rom, &catalog)?)?;
    catalog.add_function(wrap_pyfunction!(resolve_configuration, &catalog)?)?;
    catalog.add_function(wrap_pyfunction!(restart_profiles, &catalog)?)?;
    catalog.add_function(wrap_pyfunction!(card_profiles, &catalog)?)?;
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
