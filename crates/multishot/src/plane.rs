//! Single-channel float planes and 3x3 CFA block planes.
//!
//! Every 3x3 block on the X-Trans block grid holds two red, five green and two
//! blue sites, so block means are free of CFA phase. Block `(i, j)` has its
//! center at active-area site `(ox + 3i + 1, oy + 3j + 1)`.

use crate::source::{Calibration, FrameSource};
use crate::xtrans::XTrans;
use std::io;

#[derive(Clone, Debug)]
pub struct Plane {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Plane {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0.0; width * height],
        }
    }

    #[inline(always)]
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }

    /// 2x2 box average, dropping an odd last row or column.
    pub fn downsample2(&self) -> Plane {
        let (w, h) = (self.width / 2, self.height / 2);
        let mut out = Plane::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let s = self.at(2 * x, 2 * y)
                    + self.at(2 * x + 1, 2 * y)
                    + self.at(2 * x, 2 * y + 1)
                    + self.at(2 * x + 1, 2 * y + 1);
                out.data[y * w + x] = 0.25 * s;
            }
        }
        out
    }

    /// Bilinear sample; `None` outside the plane.
    #[inline]
    pub fn bilinear(&self, x: f64, y: f64) -> Option<f32> {
        if !(x >= 0.0 && y >= 0.0) {
            return None;
        }
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        if x0 + 1 >= self.width || y0 + 1 >= self.height {
            return None;
        }
        let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
        let i = y0 * self.width + x0;
        let d = &self.data;
        let top = d[i] + (d[i + 1] - d[i]) * fx;
        let bottom = d[i + self.width] + (d[i + self.width + 1] - d[i + self.width]) * fx;
        Some(top + (bottom - top) * fy)
    }

    /// Catmull-Rom bicubic sample; `None` within two samples of the edge.
    #[inline]
    pub fn bicubic(&self, x: f64, y: f64) -> Option<f32> {
        if !(x >= 1.0 && y >= 1.0) {
            return None;
        }
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        if x0 + 2 >= self.width || y0 + 2 >= self.height {
            return None;
        }
        let wx = cubic_weights((x - x0 as f64) as f32);
        let wy = cubic_weights((y - y0 as f64) as f32);
        let mut acc = 0.0;
        for (j, wyj) in wy.iter().enumerate() {
            let row = (y0 + j - 1) * self.width + x0 - 1;
            let r = &self.data[row..row + 4];
            acc += wyj * (wx[0] * r[0] + wx[1] * r[1] + wx[2] * r[2] + wx[3] * r[3]);
        }
        Some(acc)
    }
}

#[inline(always)]
pub fn cubic_weights(t: f32) -> [f32; 4] {
    let t2 = t * t;
    let t3 = t2 * t;
    [
        0.5 * (-t3 + 2.0 * t2 - t),
        0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
        0.5 * (-3.0 * t3 + 4.0 * t2 + t),
        0.5 * (t3 - t2),
    ]
}

/// Geometry of the 3x3 block grid of one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockGrid {
    pub ox: usize,
    pub oy: usize,
    pub width: usize,
    pub height: usize,
}

impl BlockGrid {
    pub fn new(cfa: XTrans, width: usize, height: usize) -> Result<Self, String> {
        let (ox, oy) = cfa.block_origin()?;
        Ok(Self {
            ox,
            oy,
            width: (width - ox) / 3,
            height: (height - oy) / 3,
        })
    }

    /// Active-area coordinate of a block-plane coordinate.
    #[inline(always)]
    pub fn to_site(&self, u: f64, v: f64) -> (f64, f64) {
        (
            3.0 * u + self.ox as f64 + 1.0,
            3.0 * v + self.oy as f64 + 1.0,
        )
    }

    #[inline(always)]
    pub fn to_block(&self, x: f64, y: f64) -> (f64, f64) {
        (
            (x - self.ox as f64 - 1.0) / 3.0,
            (y - self.oy as f64 - 1.0) / 3.0,
        )
    }
}

/// Per-channel block means `[R, G, B]` for block rows `j0..j0 + rows`.
/// `raw` holds active rows starting at `raw_y0` and must cover those blocks.
/// Clipped sites still count; a block with any clipped site is marked by
/// storing `-1 - red` (red is clamped at zero, so the mark is always <= -1)
/// so callers can skip it.
pub fn block_rgb(
    raw: &[u16],
    raw_y0: usize,
    width: usize,
    grid: &BlockGrid,
    cfa: XTrans,
    calibration: &Calibration,
    j0: usize,
    rows: usize,
    out: &mut Vec<[f32; 3]>,
) {
    out.clear();
    out.reserve(rows * grid.width);
    for j in j0..j0 + rows {
        let y_top = grid.oy + 3 * j;
        for i in 0..grid.width {
            let x_left = grid.ox + 3 * i;
            let mut sum = [0f32; 3];
            let mut n = [0f32; 3];
            let mut clipped = false;
            for dy in 0..3 {
                let y = y_top + dy;
                let row = &raw[(y - raw_y0) * width..];
                for dx in 0..3 {
                    let x = x_left + dx;
                    let c = cfa.color(x, y) as usize;
                    let (v, clip) = calibration.linear(row[x], x, y);
                    clipped |= clip;
                    sum[c] += v;
                    n[c] += 1.0;
                }
            }
            // Red is clamped at zero so that a dark block whose noisy red mean
            // falls below black cannot be mistaken for the clip mark.
            let mut m = [(sum[0] / n[0]).max(0.0), sum[1] / n[1], sum[2] / n[2]];
            if clipped {
                m[0] = -1.0 - m[0];
            }
            out.push(m);
        }
    }
}

/// White-balanced luma of a block mean, ignoring the clip mark.
#[inline(always)]
pub fn block_luma(rgb: [f32; 3], wb: [f32; 3]) -> f32 {
    let r = if rgb[0] < 0.0 { -rgb[0] - 1.0 } else { rgb[0] };
    (2.0 * r * wb[0] + 5.0 * rgb[1] * wb[1] + 2.0 * rgb[2] * wb[2]) / 9.0
}

/// Whole-frame block planes: luma for alignment and RGB for noise estimation,
/// read in bands of rows to bound memory.
pub struct BlockPlanes {
    pub grid: BlockGrid,
    pub luma: Plane,
    pub rgb: Vec<[f32; 3]>,
}

pub fn read_block_planes(frame: &dyn FrameSource, keep_rgb: bool) -> io::Result<BlockPlanes> {
    let (w, h) = (frame.width(), frame.height());
    let cfa = frame.cfa();
    let grid =
        BlockGrid::new(cfa, w, h).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let cal = frame.calibration();
    let mut luma = Plane::new(grid.width, grid.height);
    let mut rgb_all = Vec::new();
    let band = 64usize;
    let mut raw = Vec::new();
    let mut rgb = Vec::new();
    let mut j = 0;
    while j < grid.height {
        let rows = band.min(grid.height - j);
        let y0 = grid.oy + 3 * j;
        raw.resize(3 * rows * w, 0);
        frame.read_rows(y0, &mut raw)?;
        block_rgb(&raw, y0, w, &grid, cfa, cal, j, rows, &mut rgb);
        for (k, px) in rgb.iter().enumerate() {
            luma.data[j * grid.width + k] = block_luma(*px, cal.wb);
        }
        if keep_rgb {
            rgb_all.extend_from_slice(&rgb);
        }
        j += rows;
    }
    Ok(BlockPlanes {
        grid,
        luma,
        rgb: rgb_all,
    })
}

/// Full-resolution green: green sites as recorded (linear, white-balanced
/// like luma), other sites by normalized Gaussian interpolation of their
/// green neighbors.
pub fn green_plane(frame: &dyn FrameSource) -> io::Result<Plane> {
    let (w, h) = (frame.width(), frame.height());
    let cfa = frame.cfa();
    let cal = frame.calibration();
    let mut raw = vec![0u16; w * h];
    frame.read_rows(0, &mut raw)?;
    let mut out = Plane::new(w, h);
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(16);
    let rows_per = h.div_ceil(threads);
    let taps: Vec<(i64, i64, f32)> = (-2i64..=2)
        .flat_map(|dy| {
            (-2i64..=2).map(move |dx| {
                (
                    dx,
                    dy,
                    (-((dx * dx + dy * dy) as f32) / (2.0 * 0.7 * 0.7)).exp(),
                )
            })
        })
        .collect();
    std::thread::scope(|s| {
        for (t, chunk) in out.data.chunks_mut(rows_per * w).enumerate() {
            let (raw, taps) = (&raw, &taps);
            s.spawn(move || {
                let g = |x: usize, y: usize| cal.linear(raw[y * w + x], x, y).0 * cal.wb[1];
                for (r, row) in chunk.chunks_mut(w).enumerate() {
                    let y = t * rows_per + r;
                    for (x, o) in row.iter_mut().enumerate() {
                        if cfa.color(x, y) == crate::xtrans::GREEN {
                            *o = g(x, y);
                            continue;
                        }
                        let (mut num, mut den) = (0f32, 0f32);
                        for &(dx, dy, k) in taps {
                            let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                            if xx < 0 || yy < 0 || xx >= w as i64 || yy >= h as i64 {
                                continue;
                            }
                            let (xx, yy) = (xx as usize, yy as usize);
                            if cfa.color(xx, yy) == crate::xtrans::GREEN {
                                num += k * g(xx, yy);
                                den += k;
                            }
                        }
                        *o = if den > 0.0 { num / den } else { 0.0 };
                    }
                }
            });
        }
    });
    Ok(out)
}
