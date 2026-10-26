//! Bounded immutable monochrome pixel kernels; no board or hardware state.
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;

#[pyfunction]
pub fn zero_row_edges<'py>(
    py: Python<'py>,
    source: Bound<'py, PyBytes>,
    stride: usize,
    active: usize,
    rows: usize,
    edge: usize,
) -> PyResult<Bound<'py, PyBytes>> {
    let input = source.as_bytes();
    if input.len() > 64 * 1024 * 1024
        || stride == 0
        || active > stride
        || edge == 0
        || edge > active / 2
        || input.len() % stride != 0
        || rows > input.len() / stride
    {
        return Err(PyValueError::new_err(
            "Row edge geometry exceeds bounded source",
        ));
    }
    if (0..rows).all(|y| {
        input[y * stride..y * stride + edge]
            .iter()
            .chain(input[y * stride + active - edge..y * stride + active].iter())
            .all(|&v| v == 0)
    }) {
        return Ok(source);
    }
    PyBytes::new_with(py, input.len(), |output| {
        output.copy_from_slice(input);
        for y in 0..rows {
            output[y * stride..y * stride + edge].fill(0);
            output[y * stride + active - edge..y * stride + active].fill(0);
        }
        Ok(())
    })
}

#[pyfunction]
pub fn mono12_phase_gain<'py>(
    py: Python<'py>,
    source: Bound<'py, PyBytes>,
    stride: usize,
    roi: (usize, usize, usize, usize),
    gains: Vec<u16>,
    blacks: Vec<u16>,
) -> PyResult<(Bound<'py, PyBytes>, usize)> {
    let input = source.as_bytes();
    let (x0, y0, x1, y1) = roi;
    if stride == 0
        || stride > 8192
        || input.len() > 64 * 1024 * 1024
        || input.len() % (stride * 2) != 0
        || x0 > x1
        || x1 >= stride
        || y0 > y1
        || y1 >= input.len() / (stride * 2)
        || gains.len() != 256
        || gains.iter().any(|&v| v > 2047)
        || blacks.len() != 4
        || blacks.iter().any(|&v| v > 4095)
        || input
            .chunks_exact(2)
            .any(|v| u16::from_le_bytes([v[0], v[1]]) > 4095)
    {
        return Err(PyValueError::new_err(
            "Phase gain geometry or sample domain differs",
        ));
    }
    if gains.iter().all(|&v| v == 256) {
        return Ok((source, 0));
    }
    let mut changed = 0;
    let output = PyBytes::new_with(py, input.len(), |output| {
        output.copy_from_slice(input);
        for y in y0..=y1 {
            let gy = (y - y0) * 8 / (y1 - y0 + 1);
            for x in x0..=x1 {
                let phase = ((y - y0) & 1) * 2 + ((x - x0) & 1);
                let at = 2 * (y * stride + x);
                let value = u16::from_le_bytes([input[at], input[at + 1]]) as u32;
                let black = blacks[phase] as u32;
                let gain = gains[phase * 64 + gy * 8 + (x - x0) * 8 / (x1 - x0 + 1)] as u32;
                let transformed = if value > black {
                    (black + ((value - black) * gain >> 8)).min(4095)
                } else {
                    value
                };
                changed += usize::from(transformed != value);
                output[at..at + 2].copy_from_slice(&(transformed as u16).to_le_bytes());
            }
        }
        Ok(())
    })?;
    Ok((if changed == 0 { source } else { output }, changed))
}
