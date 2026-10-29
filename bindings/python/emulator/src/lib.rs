//! Python bindings for emulator boards, run configurations, input assets and settings headers.
#[cfg(not(target_os = "emscripten"))]
pub mod board;
mod settings;

use fujimod_python_base::error;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyModule};

/// Compile checked reference identities and starting inputs without constructing a CPU.
#[pyfunction]
#[pyo3(signature = (configuration_id, overlays=Vec::new()))]
fn prepare_configuration(configuration_id: &str, overlays: Vec<String>) -> PyResult<String> {
    fujimod_python_base::boot::boot()?
        .prepare(configuration_id, &overlays)
        .map_err(error)
}

/// Load explicitly located, identity-checked inputs before constructing a board.
#[pyfunction]
fn load_input_assets<'py>(
    py: Python<'py>,
    root: std::path::PathBuf,
    assets: &str,
    locations: &str,
    max_asset_bytes: u64,
) -> PyResult<Bound<'py, PyDict>> {
    let assets: Vec<fujimod_catalog_assets::Asset> = serde_json::from_str(assets)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    let locations = serde_json::from_str(locations)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    let provider = fujimod_catalog_assets::files::Files::new(
        &root,
        &root,
        Default::default(),
        locations,
        max_asset_bytes,
    )
    .map_err(error)?;
    let output = PyDict::new(py);
    for asset in assets {
        if output.contains(&asset.id)? {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "duplicate input asset identity",
            ));
        }
        let bytes = asset.load(&provider).map_err(error)?;
        output.set_item(asset.id, PyBytes::new(py, &bytes))?;
    }
    Ok(output)
}

/// Verify each declared asset identity before constructing an execution graph.
#[pyfunction]
fn verify_input_assets(manifest: &str, assets: &Bound<'_, PyDict>) -> PyResult<()> {
    let declared: Vec<fujimod_catalog_assets::Asset> = serde_json::from_str(manifest)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    let mut seen = std::collections::BTreeSet::new();
    for asset in declared {
        if !seen.insert(asset.id.clone()) {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "duplicate input asset identity",
            ));
        }
        let bytes = assets.get_item(&asset.id)?.ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err(format!("missing input asset {}", asset.id))
        })?;
        asset
            .check(bytes.cast::<PyBytes>()?.as_bytes())
            .map_err(pyo3::exceptions::PyValueError::new_err)?;
    }
    if seen.len() != assets.len() {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "undeclared input assets",
        ));
    }
    Ok(())
}

/// Incremental SHA-256 for bounded host file verification.
#[pyclass(module = "fujimod._native.emulator")]
struct Sha256 {
    digest: sha2::Sha256,
}

#[pymethods]
impl Sha256 {
    #[new]
    #[pyo3(signature=(data=None))]
    fn new(data: Option<&[u8]>) -> Self {
        use sha2::Digest;
        let mut digest = sha2::Sha256::new();
        if let Some(data) = data {
            digest.update(data);
        }
        Self { digest }
    }

    fn update(&mut self, data: &[u8]) {
        use sha2::Digest;
        self.digest.update(data);
    }

    fn hexdigest(&self) -> String {
        use sha2::Digest;
        format!("{:x}", self.digest.clone().finalize())
    }
}

/// Pure host checksums; batching avoids a call per immutable sector.
#[pyfunction]
fn sha256_many(values: Vec<Bound<'_, PyBytes>>) -> Vec<String> {
    values
        .iter()
        .map(|value| fujimod_emulator_devices::exr2::scanout_frame::sha256(value.as_bytes()))
        .collect()
}

#[pymodule(
    name = "emulator",
    module = "fujimod._native",
    submodule,
    gil_used = true
)]
mod binding {
    #[pymodule_export]
    use super::{
        Sha256, load_input_assets, prepare_configuration, sha256_many, verify_input_assets,
    };
    #[pymodule_export]
    use fujimod_python_base::NativeError;

    #[cfg(not(target_os = "emscripten"))]
    #[pymodule_export]
    use super::board::{
        AnalogInputs, Board, CardMedium, Checkpoint, DeviceContext, NandMedium, Xp4,
        normalize_card_files, persistent_packet,
    };

    #[pymodule_export]
    use fujimod_python_base::discovery::{FrontierEntry, Intervention};
    #[pymodule_export]
    use super::settings::{settings_header, settings_profile};
    #[pymodule_export]
    use fujimod_python_base::values::DeviceRefusal;

    #[pymodule_export]
    const UNICORN_ABI: &str = "2.1.4";
    #[pymodule_export]
    const VERSION: &str = env!("CARGO_PKG_VERSION");
    #[pymodule_export]
    const RUSTC_VERSION: &str = fujimod_emulator::RUSTC_VERSION;
    #[pymodule_export]
    const TARGET: &str = fujimod_emulator::TARGET;
    #[pymodule_export]
    const PYO3_VERSION: &str = "0.30.1";
}

/// Add `fujimod._native.emulator` to `parent`; the host image, graphics and XP4 binding crates
/// add their functions to it.
pub fn register(py: Python<'_>, parent: &Bound<'_, PyModule>) -> PyResult<()> {
    fujimod_python_base::board_session::install(board::borrow_session);
    let module = binding::_PYO3_DEF.make_module(py)?;
    parent.add_submodule(module.bind(py))?;
    let modules = py
        .import("sys")?
        .getattr("modules")?
        .cast_into::<PyDict>()?;
    modules.set_item("fujimod._native.emulator", &module)?;
    Ok(())
}
