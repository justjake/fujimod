//! Sample-grid scanline fill behind `emulator.graphics._ui_coverage`.
//!
//! The scalar sample computation defines the behavior. This walks sample rows
//! with the same f64 operations in the same order and returns, per pixel, how
//! many of its `scale * scale` samples are inside, so `count / scale**2`
//! equals the scalar raster's mean. It has no machine or memory access.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyByteArray, PyBytes};

/// Inside-sample counts (u8, row-major `height` x `width` pixels).
///
/// `edges` is flat `[ax, ay, bx, by, ...]` of non-horizontal emitted fill
/// edges. Pixel `(0, 0)` is at `(x0, y0)`. Raises ValueError when a sample
/// row's crossings leave an unbalanced winding (nonzero) or parity.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
pub fn ui_fill_counts<'py>(
    py: Python<'py>,
    edges: Vec<f64>,
    nonzero: bool,
    x0: i64,
    y0: i64,
    scale: i64,
    width: usize,
    height: usize,
) -> PyResult<Bound<'py, PyBytes>> {
    if !edges.len().is_multiple_of(4) || !(1..=15).contains(&scale) {
        return Err(PyValueError::new_err(
            "Invalid fill edge list or sample scale",
        ));
    }
    let s = scale as usize;
    let (sample_width, sample_height) = (width * s, height * s);
    let (x0f, y0f, scalef) = (x0 as f64, y0 as f64, scale as f64);
    let mut counts = vec![0u8; width * height];
    let mut crossings: Vec<(f64, i64)> = Vec::with_capacity(edges.len() / 4);
    let inside = |level: i64| if nonzero { level != 0 } else { level & 1 != 0 };
    for row in 0..sample_height {
        let y = y0f + (row as f64 + 0.5) / scalef;
        crossings.clear();
        for edge in edges.as_chunks::<4>().0 {
            let (ax, ay, bx, by) = (edge[0], edge[1], edge[2], edge[3]);
            if ay.min(by) <= y && y < ay.max(by) {
                crossings.push((
                    ax + (y - ay) * (bx - ax) / (by - ay),
                    if by > ay { 1 } else { -1 },
                ));
            }
        }
        crossings.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)));
        let (mut level, mut previous) = (0i64, None::<f64>);
        let pixel_row = (row / s) * width;
        for &(x, direction) in &crossings {
            if let Some(previous) = previous
                && inside(level)
            {
                let left = ((previous - x0f) * scalef - 0.5).ceil().max(0.0);
                let right = ((x - x0f) * scalef - 0.5).ceil().min(sample_width as f64);
                if left < right {
                    let (left, right) = (left as usize, right as usize);
                    let mut column = left;
                    while column < right {
                        let pixel = column / s;
                        let end = right.min((pixel + 1) * s);
                        counts[pixel_row + pixel] += (end - column) as u8;
                        column = end;
                    }
                }
            }
            level = if nonzero {
                level + direction
            } else {
                level + 1
            };
            previous = Some(x);
        }
        if inside(level) {
            return Err(PyValueError::new_err(
                "Graphics UI emitted fill edges have an unbalanced scanline",
            ));
        }
    }
    Ok(PyBytes::new(py, &counts))
}

/// Composite one draw over its target region, the `_ui_render` blend loop.
///
/// `region` is the region's RGBA bytes (row-major, `coverage.len()` pixels),
/// `coverage` its per-pixel f64 coverage, `source` either one RGBA f64 color
/// or one per pixel, all in 0..1. Every value is computed with the f64
/// operations, in the order, of the NumPy expression it replaces; pixels with
/// zero coverage (or zero alpha when blending) keep all their bytes.
#[pyfunction]
pub fn ui_composite<'py>(
    py: Python<'py>,
    region: &[u8],
    coverage: &[u8],
    source: &[u8],
    blend: bool,
) -> PyResult<Bound<'py, PyBytes>> {
    let pixels = coverage.len() / 8;
    let per_pixel = source.len() == pixels * 32;
    if !coverage.len().is_multiple_of(8)
        || region.len() != pixels * 4
        || !(per_pixel || source.len() == 32)
    {
        return Err(PyValueError::new_err(
            "Invalid composite region/coverage/source sizes",
        ));
    }
    let mut out = region.to_vec();
    for i in 0..pixels {
        let pixel: &mut [u8; 4] = (&mut out[i * 4..i * 4 + 4]).try_into().unwrap();
        blend_pixel(
            pixel,
            f64_at(coverage, i),
            source_at(source, per_pixel, i),
            blend,
        );
    }
    Ok(PyBytes::new(py, &out))
}

/// `ui_composite` applied in place to a target kept in its tiled memory
/// layout (8x8 Morton tiles, `_ui_morton`): the region is the `width` x
/// `height` rectangle at (x0, y0) of a `target_width`-wide target, and each
/// pixel's RGBA is bytes (3,2,1,0) of its word for order 0, (2,1,0,3)
/// otherwise, exactly as `_ui_render` detiles and retiles it.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
pub fn ui_composite_tiled(
    target: &Bound<'_, PyByteArray>,
    target_width: usize,
    x0: usize,
    y0: usize,
    width: usize,
    height: usize,
    order: u32,
    coverage: &[u8],
    source: &[u8],
    blend: bool,
) -> PyResult<()> {
    let pixels = width * height;
    let per_pixel = source.len() == pixels * 32;
    let target_height = target.len() / 4 / target_width.max(1);
    if coverage.len() != pixels * 8
        || !(per_pixel || source.len() == 32)
        || !target_width.is_multiple_of(8)
        || target.len() != target_width * target_height * 4
        || x0 + width > target_width
        || y0 + height > target_height
    {
        return Err(PyValueError::new_err(
            "Invalid tiled composite geometry/coverage/source sizes",
        ));
    }
    let channels: [usize; 4] = if order == 0 {
        [3, 2, 1, 0]
    } else {
        [2, 1, 0, 3]
    };
    // SAFETY: no Python code runs while this exclusive view is alive.
    let bytes = unsafe { target.as_bytes_mut() };
    for row in 0..height {
        for column in 0..width {
            let i = row * width + column;
            let (x, y) = (x0 + column, y0 + row);
            let local =
                (x & 1) | (y & 1) << 1 | (x & 2) << 1 | (y & 2) << 2 | (x & 4) << 2 | (y & 4) << 3;
            let word = (((y >> 3) * (target_width >> 3) + (x >> 3)) * 64 + local) * 4;
            let mut pixel = [0u8; 4];
            for (channel, &byte) in channels.iter().enumerate() {
                pixel[channel] = bytes[word + byte];
            }
            blend_pixel(
                &mut pixel,
                f64_at(coverage, i),
                source_at(source, per_pixel, i),
                blend,
            );
            for (channel, &byte) in channels.iter().enumerate() {
                bytes[word + byte] = pixel[channel];
            }
        }
    }
    Ok(())
}

fn f64_at(bytes: &[u8], index: usize) -> f64 {
    f64::from_ne_bytes(bytes[index * 8..index * 8 + 8].try_into().unwrap())
}

fn source_at(source: &[u8], per_pixel: bool, pixel: usize) -> [f64; 4] {
    let s = if per_pixel { pixel * 4 } else { 0 };
    [
        f64_at(source, s),
        f64_at(source, s + 1),
        f64_at(source, s + 2),
        f64_at(source, s + 3),
    ]
}

/// One pixel of the reference blend (see `ui_composite`), in place.
fn blend_pixel(pixel: &mut [u8; 4], c: f64, [sr, sg, sb, sa]: [f64; 4], blend: bool) {
    let alpha = sa * c;
    if c == 0.0 || (blend && alpha == 0.0) {
        return;
    }
    let (dr, dg, db, da) = (
        pixel[0] as f64 / 255.0,
        pixel[1] as f64 / 255.0,
        pixel[2] as f64 / 255.0,
        pixel[3] as f64 / 255.0,
    );
    let retained = if blend { 1.0 - alpha } else { 1.0 - c };
    let out_alpha = alpha + da * retained;
    let channel = |src: f64, dst: f64| {
        let numerator = src * alpha + dst * da * retained;
        if out_alpha != 0.0 {
            numerator / out_alpha
        } else {
            0.0
        }
    };
    // Keep max/min's NaN-to-zero behavior from the reference conversion.
    #[allow(clippy::manual_clamp)]
    let byte = |value: f64| (value.max(0.0).min(1.0) * 255.0 + 0.5).floor() as u8;
    *pixel = [
        byte(channel(sr, dr)),
        byte(channel(sg, dg)),
        byte(channel(sb, db)),
        byte(out_alpha),
    ];
}
