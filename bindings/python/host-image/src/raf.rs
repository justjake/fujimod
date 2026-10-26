//! Linear scene inspection through the same RAF decoder used by sensor input.
use pyo3::prelude::*;
use pyo3::types::PyBytes;

/// Average each CFA color over 6x6 cells of the visible crop. Float32 LE,
/// row-major RGB, in the decoder's exposure-normalized radiance units.
#[pyfunction]
pub(crate) fn raf_radiance_rgb<'py>(
    py: Python<'py>,
    raf: &[u8],
) -> PyResult<(usize, usize, Bound<'py, PyBytes>)> {
    let scene = fujimod_emulator_image_processing::scene::raf::read_raf(std::io::Cursor::new(raf))
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    let width = usize::from(scene.crop_width) / 6;
    let height = usize::from(scene.crop_height) / 6;
    let bytes = PyBytes::new_with(py, width * height * 12, |output| {
        for y in 0..height {
            for x in 0..width {
                let mut sums = [0f64; 3];
                let mut counts = [0u32; 3];
                for dy in 0..6 {
                    for dx in 0..6 {
                        let sx = usize::from(scene.crop_left) + x * 6 + dx;
                        let sy = usize::from(scene.crop_top) + y * 6 + dy;
                        let c = usize::from(scene.cfa[sy % 6][sx % 6]);
                        sums[c] += f64::from(scene.radiance(sy * scene.width as usize + sx));
                        counts[c] += 1;
                    }
                }
                for c in 0..3 {
                    let at = ((y * width + x) * 3 + c) * 4;
                    output[at..at + 4]
                        .copy_from_slice(&((sums[c] / f64::from(counts[c])) as f32).to_le_bytes());
                }
            }
        }
        Ok(())
    })?;
    Ok((width, height, bytes))
}
