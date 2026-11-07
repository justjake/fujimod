//! Python bindings for emulator boards, run configurations, input assets and settings headers.
#[cfg(not(target_os = "emscripten"))]
pub mod board;
#[cfg(not(target_os = "emscripten"))]
mod perf;
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

#[pymodule(
    name = "emulator",
    module = "fujimod._native",
    submodule,
    gil_used = true
)]
mod binding {
    #[pymodule_export]
    use super::{load_input_assets, prepare_configuration};
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
    #[cfg(not(target_os = "emscripten"))]
    perf::register(py, parent)?;
    let modules = py
        .import("sys")?
        .getattr("modules")?
        .cast_into::<PyDict>()?;
    modules.set_item("fujimod._native.emulator", &module)?;
    Ok(())
}
