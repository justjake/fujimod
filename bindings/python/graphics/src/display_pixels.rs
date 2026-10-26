//! Python byte transport for the shared display color conversion.
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
#[pyfunction]
pub fn decode_display_rgb<'py>(
    py: Python<'py>,
    data: &[u8],
    width: u64,
    height: u64,
    stride: u64,
    packing: &str,
    matrix: &str,
) -> PyResult<Bound<'py, PyBytes>> {
    let bytes = py
        .detach(|| {
            fujimod_emulator_imaging::packed_color::decode(
                data, width, height, stride, packing, matrix,
            )
        })
        .map_err(PyValueError::new_err)?;
    Ok(PyBytes::new(py, &bytes))
}
