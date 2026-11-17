//! Single-frame baseline: X-Trans demosaic and upscaling to the merge grid.
//!
//! Green is interpolated by normalized Gaussian convolution of the green
//! sites; red and blue by interpolating the color differences `R - G` and
//! `B - G` the same way with a wider kernel. The result is resampled onto the
//! merge's output grid with Catmull-Rom bicubic interpolation.

use crate::merge::{OutputGrid, Region};
use crate::plane::{Plane, cubic_weights};
use crate::source::FrameSource;
use std::io;

fn normalized_convolution(values: &Plane, known: &[bool], sigma: f32, radius: i64) -> Plane {
    let (w, h) = (values.width, values.height);
    let mut out = Plane::new(w, h);
    let mut kernel = Vec::new();
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let d2 = (dx * dx + dy * dy) as f32;
            kernel.push((dx, dy, (-d2 / (2.0 * sigma * sigma)).exp()));
        }
    }
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let i = (y as usize) * w + x as usize;
            if known[i] {
                out.data[i] = values.data[i];
                continue;
            }
            let (mut num, mut den) = (0f32, 0f32);
            for &(dx, dy, g) in &kernel {
                let (xx, yy) = (x + dx, y + dy);
                if xx < 0 || yy < 0 || xx >= w as i64 || yy >= h as i64 {
                    continue;
                }
                let k = yy as usize * w + xx as usize;
                if known[k] {
                    num += g * values.data[k];
                    den += g;
                }
            }
            out.data[i] = if den > 0.0 { num / den } else { 0.0 };
        }
    }
    out
}

/// Demosaiced linear RGB planes for `region` plus `margin` sites (clamped),
/// with the covered rectangle.
pub fn demosaic(
    frame: &dyn FrameSource,
    region: Region,
    margin: usize,
) -> io::Result<(Region, [Plane; 3])> {
    let (fw, fh) = (frame.width(), frame.height());
    let x0 = region.x.saturating_sub(margin);
    let y0 = region.y.saturating_sub(margin);
    let x1 = (region.x + region.width + margin).min(fw);
    let y1 = (region.y + region.height + margin).min(fh);
    let (w, h) = (x1 - x0, y1 - y0);
    let mut raw = vec![0u16; (y1 - y0) * fw];
    frame.read_rows(y0, &mut raw)?;
    let cfa = frame.cfa();
    let cal = frame.calibration();
    let mut v = Plane::new(w, h);
    let mut color = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let (val, _) = cal.linear(raw[y * fw + x0 + x], x0 + x, y0 + y);
            v.data[y * w + x] = val.min(1.0);
            color[y * w + x] = cfa.color(x0 + x, y0 + y);
        }
    }
    let is = |c: u8| -> Vec<bool> { color.iter().map(|&k| k == c).collect() };
    let green = normalized_convolution(&v, &is(1), 0.7, 2);
    let mut planes = [Plane::new(w, h), green.clone(), Plane::new(w, h)];
    for (slot, c) in [(0usize, 0u8), (2, 2)] {
        let known = is(c);
        let mut diff = Plane::new(w, h);
        for i in 0..w * h {
            if known[i] {
                diff.data[i] = v.data[i] - green.data[i];
            }
        }
        let diff = normalized_convolution(&diff, &known, 1.2, 3);
        for i in 0..w * h {
            planes[slot].data[i] = if known[i] {
                v.data[i]
            } else {
                green.data[i] + diff.data[i]
            };
        }
    }
    Ok((
        Region {
            x: x0,
            y: y0,
            width: w,
            height: h,
        },
        planes,
    ))
}

/// The baseline image on `out`'s grid: demosaic, then bicubic upscaling.
pub fn baseline(frame: &dyn FrameSource, out: &OutputGrid) -> io::Result<Vec<f32>> {
    let (covered, planes) = demosaic(frame, out.region, 4)?;
    let mut rgb = vec![0f32; out.width * out.height * 3];
    for j in 0..out.height {
        for i in 0..out.width {
            let (x, y) = out.to_site(i as f64, j as f64);
            let (lx, ly) = (x - covered.x as f64, y - covered.y as f64);
            for c in 0..3 {
                rgb[(j * out.width + i) * 3 + c] = sample_clamped(&planes[c], lx, ly);
            }
        }
    }
    Ok(rgb)
}

fn sample_clamped(p: &Plane, x: f64, y: f64) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let wx = cubic_weights((x - x0) as f32);
    let wy = cubic_weights((y - y0) as f32);
    let mut acc = 0.0;
    for (j, wyj) in wy.iter().enumerate() {
        let yy = (y0 as i64 + j as i64 - 1).clamp(0, p.height as i64 - 1) as usize;
        for (i, wxi) in wx.iter().enumerate() {
            let xx = (x0 as i64 + i as i64 - 1).clamp(0, p.width as i64 - 1) as usize;
            acc += wyj * wxi * p.data[yy * p.width + xx];
        }
    }
    acc
}
