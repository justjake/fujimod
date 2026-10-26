//! Complete method-4 token decoding for isolated firmware-format harnesses.
//! Peripheral streaming uses the same codec through the shared Device model.
use fujimod_emulator_devices::compression::method4::Stream;
use pyo3::{exceptions::PyValueError, prelude::*, types::PyBytes};

#[pyfunction]
pub fn method4_decode<'py>(
    py: Python<'py>,
    source: &[u8],
    unpacked: usize,
) -> PyResult<(Bound<'py, PyBytes>, usize, usize)> {
    if unpacked > 512 * 1024 * 1024 {
        return Err(PyValueError::new_err(
            "Declared method-4 output exceeds 512 MiB",
        ));
    }
    let mut output = vec![0; unpacked];
    let mut stream = Stream::default();
    let count = stream
        .feed(source, &mut output)
        .map_err(PyValueError::new_err)?;
    if count != unpacked || stream.pending_bytes() != 0 {
        return Err(PyValueError::new_err(
            "Truncated method-4 stream or declared size differs",
        ));
    }
    Ok((PyBytes::new(py, &output), stream.literals, stream.matches))
}
