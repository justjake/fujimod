//! Resolution stage: multi-frame super-resolution of an aligned burst.
//!
//! Every CFA site of every accepted frame is mapped into reference
//! coordinates and splatted, per color, onto an output grid finer than the
//! sensor with a small Gaussian kernel. Hand shake places the samples of
//! different frames at different sub-pixel positions, so each output pixel
//! collects real red, green and blue samples near its center and needs no
//! demosaic. Each frame's contribution is scaled by a robustness weight that
//! compares it with the reference at block resolution and rejects local
//! motion instead of ghosting it. Where samples are sparse, a weak prior from
//! the reference's block means fills in. Kernels follow the reference's local
//! structure (after Wronski et al.): narrow across edges, long along them and
//! wide in flat regions.
//!
//! Red and blue sites are sparse on X-Trans (2 of 9 each), so by default green
//! is merged first and red and blue are merged as white-balanced differences
//! to the merged green at their own sample positions. Color differences vary
//! slowly, so red and blue inherit green's resolution instead of their own.
//!
//! Output rows are produced in strips; each strip reads only the rows of each
//! frame that map into it.

use crate::align::Alignment;
use crate::fit::Affine;
use crate::local::{LocalParams, LocalReport, ShiftField, residual_fields};
use crate::noise::NoiseModel;
use crate::plane::{BlockGrid, block_luma, block_rgb};
use crate::source::FrameSource;
use serde::{Deserialize, Serialize};
use std::io;
use std::time::Instant;

/// A region of the reference in active-area sites.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RobustParams {
    /// Normalized difference at which the weight falls to `exp(-1/2)`.
    pub threshold: f32,
    /// Fraction of the reference's local standard deviation added to the
    /// expected difference, tolerating residual misalignment at edges.
    pub texture_term: f32,
}

impl Default for RobustParams {
    fn default() -> Self {
        Self {
            threshold: 3.0,
            texture_term: 0.8,
        }
    }
}

/// Kernel shape from the reference's local structure tensor: narrower across
/// edges, longer along them, wider where the reference is flat.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KernelShape {
    /// Along-edge widening at full coherence (`sigma * (1 + stretch)`).
    pub stretch: f64,
    /// Across-edge narrowing at full coherence (`sigma * (1 - shrink)`).
    pub shrink: f64,
    /// Width factor in flat regions.
    pub flat_scale: f64,
}

impl Default for KernelShape {
    fn default() -> Self {
        Self {
            stretch: 1.0,
            shrink: 0.5,
            flat_scale: 1.5,
        }
    }
}

/// How red and blue are merged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColorMerge {
    /// Each color is splatted from its own samples only.
    Independent,
    /// Green is merged first; red and blue are merged as white-balanced
    /// differences to the merged green.
    #[default]
    GreenGuided,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolutionParams {
    /// Output pixels per sensor site along each axis.
    pub scale: f64,
    /// Kernel standard deviation in sensor sites.
    pub sigma: f64,
    /// Structure-adaptive kernel; `None` keeps it isotropic.
    pub shape: Option<KernelShape>,
    /// Kernel support radius in sensor sites.
    pub radius: f64,
    /// Weight of the reference block-mean prior relative to one sample.
    pub prior_weight: f32,
    pub robustness: Option<RobustParams>,
    #[serde(default)]
    pub color: ColorMerge,
    /// Bracketed exposures: each frame is scaled by its alignment gain to the
    /// reference's exposure and its samples are weighted by their noise in
    /// reference units, so long exposures carry the shadows and short ones
    /// the highlights. The output is scaled so that 1.0 is the shortest
    /// exposure's white level.
    #[serde(default)]
    pub bracket: bool,
    /// Per-tile residual alignment on top of each frame's global transform;
    /// `None` uses the global transforms only.
    #[serde(default)]
    pub local: Option<LocalParams>,
    pub strip_rows: usize,
    pub region: Option<Region>,
}

impl Default for ResolutionParams {
    fn default() -> Self {
        Self {
            scale: 1.5,
            sigma: 0.38,
            shape: Some(KernelShape::default()),
            radius: 1.5,
            prior_weight: 0.02,
            robustness: Some(RobustParams::default()),
            color: ColorMerge::default(),
            bracket: false,
            local: None,
            strip_rows: 48,
            region: None,
        }
    }
}

/// Receives the merged image in linear camera RGB (white level 1.0), strip by
/// strip, top to bottom. `rgb` holds `rows * width * 3` values.
pub trait StripSink {
    fn write_strip(&mut self, y0: usize, rows: usize, rgb: &[f32]) -> io::Result<()>;
}

/// Keeps the whole merged image in memory.
pub struct ImageSink {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<f32>,
}

impl ImageSink {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            rgb: vec![0.0; width * height * 3],
        }
    }
}

impl StripSink for ImageSink {
    fn write_strip(&mut self, y0: usize, rows: usize, rgb: &[f32]) -> io::Result<()> {
        let start = y0 * self.width * 3;
        self.rgb[start..start + rows * self.width * 3].copy_from_slice(rgb);
        Ok(())
    }
}

/// Fans one strip out to several sinks.
pub struct Tee<'a>(pub Vec<&'a mut dyn StripSink>);

impl StripSink for Tee<'_> {
    fn write_strip(&mut self, y0: usize, rows: usize, rgb: &[f32]) -> io::Result<()> {
        for s in self.0.iter_mut() {
            s.write_strip(y0, rows, rgb)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameMergeStats {
    pub label: String,
    pub used: bool,
    /// Mean robustness weight over the reference blocks the frame covers.
    pub mean_weight: f32,
    /// Fraction of covered blocks with weight below 0.1.
    pub rejected_fraction: f32,
    /// Exposure factor applied to the frame (1 outside bracket merges).
    #[serde(default)]
    pub gain: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MergeReport {
    pub width: usize,
    pub height: usize,
    pub region: Region,
    pub scale: f64,
    pub frames: Vec<FrameMergeStats>,
    pub strips: usize,
    pub strip_rows: usize,
    pub threads: usize,
    /// Largest working memory of one strip: accumulators, frame row bands,
    /// block bands and the output strip.
    pub peak_strip_bytes: usize,
    /// Residual alignment, when enabled.
    pub local: Option<LocalReport>,
    /// Factor applied to the output: 1, or for a bracket merge the inverse
    /// of the largest frame gain.
    #[serde(default)]
    pub output_scale: f32,
    pub seconds: f64,
}

/// Output geometry: output pixel `(i, j)` is centered on reference site
/// `(x + (i + 0.5) / scale - 0.5, y + (j + 0.5) / scale - 0.5)`.
#[derive(Clone, Copy, Debug)]
pub struct OutputGrid {
    pub region: Region,
    pub scale: f64,
    pub width: usize,
    pub height: usize,
}

impl OutputGrid {
    pub fn new(region: Region, scale: f64) -> Self {
        Self {
            region,
            scale,
            width: (region.width as f64 * scale).round() as usize,
            height: (region.height as f64 * scale).round() as usize,
        }
    }

    #[inline(always)]
    pub fn to_site(&self, i: f64, j: f64) -> (f64, f64) {
        (
            self.region.x as f64 + (i + 0.5) / self.scale - 0.5,
            self.region.y as f64 + (j + 0.5) / self.scale - 0.5,
        )
    }

    #[inline(always)]
    pub fn to_output(&self, x: f64, y: f64) -> (f64, f64) {
        (
            (x - self.region.x as f64 + 0.5) * self.scale - 0.5,
            (y - self.region.y as f64 + 0.5) * self.scale - 0.5,
        )
    }
}

/// Block means for block rows `j0..j0 + rows` of a frame (whole width).
fn read_block_band(
    frame: &dyn FrameSource,
    grid: &BlockGrid,
    j0: usize,
    rows: usize,
    raw: &mut Vec<u16>,
    out: &mut Vec<[f32; 3]>,
) -> io::Result<()> {
    let w = frame.width();
    let y0 = grid.oy + 3 * j0;
    raw.resize(3 * rows * w, 0);
    frame.read_rows(y0, raw)?;
    block_rgb(
        raw,
        y0,
        w,
        grid,
        frame.cfa(),
        frame.calibration(),
        j0,
        rows,
        out,
    );
    Ok(())
}

#[inline(always)]
fn unclip(v: f32) -> f32 {
    if v < 0.0 { -v - 1.0 } else { v }
}

/// A band of block-resolution values `[jlo, jlo + rows)`, whole width.
struct BlockBand<T> {
    jlo: usize,
    rows: usize,
    width: usize,
    data: Vec<T>,
}

impl BlockBand<[f32; 3]> {
    fn bilinear(&self, u: f64, v: f64) -> Option<[f32; 3]> {
        let v = v - self.jlo as f64;
        if !(u >= 0.0 && v >= 0.0) {
            return None;
        }
        let (x0, y0) = (u.floor() as usize, v.floor() as usize);
        if x0 + 1 >= self.width || y0 + 1 >= self.rows {
            return None;
        }
        let (fx, fy) = ((u - x0 as f64) as f32, (v - y0 as f64) as f32);
        let q = [
            self.data[y0 * self.width + x0],
            self.data[y0 * self.width + x0 + 1],
            self.data[(y0 + 1) * self.width + x0],
            self.data[(y0 + 1) * self.width + x0 + 1],
        ];
        let mut out = [0f32; 3];
        let clipped = q.iter().any(|p| p[0] < 0.0);
        for c in 0..3 {
            let g = |p: [f32; 3]| if c == 0 { unclip(p[0]) } else { p[c] };
            let top = g(q[0]) + (g(q[1]) - g(q[0])) * fx;
            let bottom = g(q[2]) + (g(q[3]) - g(q[2])) * fx;
            out[c] = top + (bottom - top) * fy;
        }
        if clipped {
            out[0] = -1.0 - out[0];
        }
        Some(out)
    }
}

impl BlockBand<f32> {
    fn bilinear(&self, u: f64, v: f64) -> f32 {
        let v = (v - self.jlo as f64).clamp(0.0, (self.rows - 1) as f64);
        let u = u.clamp(0.0, (self.width - 1) as f64);
        let (x0, y0) = (u.floor() as usize, v.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.rows - 1));
        let (fx, fy) = ((u - x0 as f64) as f32, (v - y0 as f64) as f32);
        let d = &self.data;
        let top =
            d[y0 * self.width + x0] + (d[y0 * self.width + x1] - d[y0 * self.width + x0]) * fx;
        let bottom =
            d[y1 * self.width + x0] + (d[y1 * self.width + x1] - d[y1 * self.width + x0]) * fx;
        top + (bottom - top) * fy
    }
}

/// Robustness weights of one frame for the reference block band.
fn robustness(
    reference: &BlockBand<[f32; 3]>,
    frame: &BlockBand<[f32; 3]>,
    to_frame_blocks: &Affine,
    noise: &NoiseModel,
    wb: [f32; 3],
    params: &RobustParams,
    gain: f32,
) -> (BlockBand<f32>, f32, f32) {
    let (w, rows) = (reference.width, reference.rows);
    let mut raw = vec![0f32; w * rows];
    let t2 = params.threshold * params.threshold;
    for r in 0..rows {
        for i in 0..w {
            let k = r * w + i;
            let refpx = reference.data[k];
            let j = (reference.jlo + r) as f64;
            let (u, v) = to_frame_blocks.apply(i as f64, j);
            let Some(mut fpx) = frame.bilinear(u, v) else {
                continue;
            };
            if fpx[0] >= 0.0 {
                fpx = fpx.map(|p| p * gain);
            }
            if refpx[0] < 0.0 || fpx[0] < 0.0 {
                // Clipped highlights cannot be compared; keep the samples
                // (clipped sites themselves are never merged).
                raw[k] = 1.0;
                continue;
            }
            // Local texture: standard deviation of reference luma over the
            // 3x3 block neighborhood.
            let (mut s, mut ss, mut n) = (0f32, 0f32, 0f32);
            for dr in r.saturating_sub(1)..(r + 2).min(rows) {
                for di in i.saturating_sub(1)..(i + 2).min(w) {
                    let l = block_luma(reference.data[dr * w + di], wb);
                    s += l;
                    ss += l * l;
                    n += 1.0;
                }
            }
            let mean = s / n;
            let local_var = (ss / n - mean * mean).max(0.0);
            let mut d2 = 0f32;
            for c in 0..3 {
                let expected = noise.variance(c, refpx[c])
                    + noise.scaled_variance(c, refpx[c], gain)
                    + params.texture_term * params.texture_term * local_var / (wb[c] * wb[c]);
                d2 += (fpx[c] - refpx[c]).powi(2) / expected;
            }
            d2 /= 3.0;
            raw[k] = (-0.5 * (d2 - 1.0).max(0.0) / t2).exp();
        }
    }
    // Dilate rejections: each block takes the minimum of its neighborhood.
    let mut out = vec![0f32; w * rows];
    let (mut sum, mut low, mut covered) = (0f32, 0usize, 0usize);
    for r in 0..rows {
        for i in 0..w {
            let mut m = f32::MAX;
            for dr in r.saturating_sub(1)..(r + 2).min(rows) {
                for di in i.saturating_sub(1)..(i + 2).min(w) {
                    m = m.min(raw[dr * w + di]);
                }
            }
            out[r * w + i] = m;
            if raw[r * w + i] > 0.0 {
                covered += 1;
                sum += m;
                if m < 0.1 {
                    low += 1;
                }
            }
        }
    }
    let mean = if covered > 0 {
        sum / covered as f32
    } else {
        0.0
    };
    let rejected = if covered > 0 {
        low as f32 / covered as f32
    } else {
        1.0
    };
    (
        BlockBand {
            jlo: reference.jlo,
            rows,
            width: w,
            data: out,
        },
        mean,
        rejected,
    )
}

const LUT: usize = 2048;
const LUT_RANGE: f64 = 8.0;

/// Per output pixel of a strip: half the inverse kernel covariance
/// `[xx, xy, yy]` in site units.
fn strip_kernels(
    params: &ResolutionParams,
    out: &OutputGrid,
    grid: &BlockGrid,
    band: &BlockBand<[f32; 3]>,
    noise: Option<&NoiseModel>,
    wb: [f32; 3],
    j0: usize,
    rows: usize,
) -> Vec<[f32; 3]> {
    let sigma = params.sigma;
    let max_sigma = params.radius / 2.0;
    let iso = (0.5 / (sigma * sigma)) as f32;
    let Some(shape) = &params.shape else {
        return vec![[iso, 0.0, iso]; rows * out.width];
    };
    // Structure tensor per block, from white-balanced luma gradients per site,
    // summed over 3x3 blocks.
    let (w, h) = (band.width, band.rows);
    let l: Vec<f32> = band.data.iter().map(|p| block_luma(*p, wb)).collect();
    let mut g = vec![[0f32; 3]; w * h];
    for r in 1..h.saturating_sub(1) {
        for i in 1..w - 1 {
            let gx = (l[r * w + i + 1] - l[r * w + i - 1]) / 6.0;
            let gy = (l[(r + 1) * w + i] - l[(r - 1) * w + i]) / 6.0;
            g[r * w + i] = [gx * gx, gx * gy, gy * gy];
        }
    }
    let mut tensor = vec![[0f32; 4]; w * h];
    for r in 0..h {
        for i in 0..w {
            let mut t = [0f32; 3];
            for rr in r.saturating_sub(1)..(r + 2).min(h) {
                for ii in i.saturating_sub(1)..(i + 2).min(w) {
                    let v = g[rr * w + ii];
                    t[0] += v[0];
                    t[1] += v[1];
                    t[2] += v[2];
                }
            }
            tensor[r * w + i] = [t[0] / 9.0, t[1] / 9.0, t[2] / 9.0, l[r * w + i]];
        }
    }
    let mut kernels = Vec::with_capacity(rows * out.width);
    for jr in 0..rows {
        for i in 0..out.width {
            let (x, y) = out.to_site(i as f64, (j0 + jr) as f64);
            let (u, v) = grid.to_block(x, y);
            let bi = (u.round().max(0.0) as usize).min(w - 1);
            let br = ((v.round() - band.jlo as f64).max(0.0) as usize).min(h - 1);
            let [txx, txy, tyy, lum] = tensor[br * w + bi];
            let half_tr = 0.5 * (txx + tyy);
            let disc = (0.25 * (txx - tyy).powi(2) + txy * txy).sqrt();
            let (l1, l2) = (half_tr + disc, (half_tr - disc).max(0.0));
            let coherence = if l1 + l2 > 1e-20 {
                ((l1 - l2) / (l1 + l2)) as f64
            } else {
                0.0
            };
            // Detail: gradient magnitude against the gradient noise of block
            // luma (noise of a 3-site difference of block means).
            let detail = match noise {
                Some(n) => {
                    let gn =
                        (2.0 * n.variance(1, lum.max(0.0) / wb[1].max(1e-6))).sqrt() * wb[1] / 6.0;
                    ((l1.sqrt() - 2.0 * gn) / (6.0 * gn).max(1e-9)).clamp(0.0, 1.0) as f64
                }
                None => 1.0,
            };
            let lerp = |a: f64, b: f64| a + (b - a) * detail;
            let across =
                (sigma * lerp(shape.flat_scale, 1.0 - shape.shrink * coherence)).min(max_sigma);
            let along =
                (sigma * lerp(shape.flat_scale, 1.0 + shape.stretch * coherence)).min(max_sigma);
            // Unit gradient direction.
            let (ex, ey) = if disc > 1e-20 {
                let (a, b) = (txy as f64, (l1 - txx) as f64);
                let n = (a * a + b * b).sqrt();
                if n > 1e-20 {
                    (a / n, b / n)
                } else {
                    (1.0, 0.0)
                }
            } else {
                (1.0, 0.0)
            };
            let (ia, il) = (0.5 / (across * across), 0.5 / (along * along));
            kernels.push([
                (ia * ex * ex + il * ey * ey) as f32,
                ((ia - il) * ex * ey) as f32,
                (ia * ey * ey + il * ex * ex) as f32,
            ]);
        }
    }
    kernels
}

fn threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(16)
}

/// One frame's rows for a strip, with its robustness map.
struct PreparedFrame<'a> {
    k: usize,
    to_ref: Affine,
    /// Exposure factor and per-channel noise weights `(a, b)` of the
    /// scaled samples, relative to the reference (bracket merges only).
    gain: f32,
    noise_weight: Option<([f32; 3], [f32; 3])>,
    field: Option<&'a ShiftField>,
    fy0: usize,
    fy1: usize,
    raw: Vec<u16>,
    weights: Option<BlockBand<f32>>,
}

/// Gaussian splatting onto output rows `j0..j0 + rows` of an accumulator
/// holding `stride` values per pixel: weighted sums first, weights `half`
/// values later.
struct Splatter<'a> {
    out: &'a OutputGrid,
    /// Kernels for output rows from `kernel_j0`.
    kernels: &'a [[f32; 3]],
    kernel_j0: usize,
    lut: &'a [f32],
    scale: f64,
    radius: f64,
}

impl Splatter<'_> {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn splat(
        &self,
        acc: &mut [f32],
        stride: usize,
        half: usize,
        slot: usize,
        j0: usize,
        rows: usize,
        (qx, qy): (f64, f64),
        value: f32,
        weight: f32,
    ) {
        let s = self.scale;
        let rs = self.radius * s;
        let r2 = self.radius * self.radius;
        let width = self.out.width;
        let i0 = ((qx - rs).ceil() as i64).max(0);
        let i1 = ((qx + rs).floor() as i64).min(width as i64 - 1);
        let jj0 = ((qy - rs).ceil() as i64).max(j0 as i64);
        let jj1 = ((qy + rs).floor() as i64).min((j0 + rows) as i64 - 1);
        for j in jj0..=jj1 {
            let dy = (j as f64 - qy) / s;
            let row = (j as usize - j0) * width;
            let krow = (j as usize - self.kernel_j0) * width;
            for i in i0..=i1 {
                let dx = (i as f64 - qx) / s;
                if dx * dx + dy * dy >= r2 {
                    continue;
                }
                let k = self.kernels[krow + i as usize];
                let (dx, dy) = (dx as f32, dy as f32);
                let arg = k[0] * dx * dx + 2.0 * k[1] * dx * dy + k[2] * dy * dy;
                if arg >= LUT_RANGE as f32 {
                    continue;
                }
                let g = self.lut[(arg * (LUT as f64 / LUT_RANGE) as f32) as usize] * weight;
                let a = (row + i as usize) * stride;
                acc[a + slot] += g * value;
                acc[a + half + slot] += g;
            }
        }
    }
}

/// Calls `f(color, reference position, linear value, robustness weight)` for
/// every unclipped sample of `frame` that maps inside `[x_lo, x_hi] x [y_lo,
/// y_hi]` of the reference and has a usable weight.
fn for_each_sample(
    frame: &dyn FrameSource,
    pf: &PreparedFrame,
    grid: &BlockGrid,
    (x_lo, x_hi, y_lo, y_hi): (f64, f64, f64, f64),
    mut f: impl FnMut(usize, (f64, f64), f32, f32),
) {
    let w = frame.width();
    let cal = frame.calibration();
    let fcfa = frame.cfa();
    for y in pf.fy0..pf.fy1 {
        let row = &pf.raw[(y - pf.fy0) * w..(y - pf.fy0 + 1) * w];
        let (mut px, mut py) = pf.to_ref.apply(0.0, y as f64);
        let (ddx, ddy) = (pf.to_ref.0[0], pf.to_ref.0[2]);
        for (x, &value) in row.iter().enumerate() {
            if x > 0 {
                px += ddx;
                py += ddy;
            }
            // The residual field shifts the frame position; map it back
            // through the linear part of the global transform.
            let (px, py) = match pf.field {
                Some(field) => {
                    let (dx, dy) = field.at(px, py);
                    let m = &pf.to_ref.0;
                    (px - m[0] * dx - m[1] * dy, py - m[2] * dx - m[3] * dy)
                }
                None => (px, py),
            };
            if py < y_lo || py > y_hi || px < x_lo || px > x_hi {
                continue;
            }
            let (v, clipped) = cal.linear(value, x, y);
            if clipped {
                continue;
            }
            let c = fcfa.color(x, y) as usize;
            let v = v * pf.gain;
            // Inverse variance relative to a reference sample at the same
            // level: (a L + b) / (g a L + g^2 b).
            let wn = match pf.noise_weight {
                Some((a, b)) => {
                    let l = v.max(0.0);
                    (a[c] * l + b[c]) / (pf.gain * a[c] * l + pf.gain * pf.gain * b[c])
                }
                None => 1.0,
            };
            let wr = wn
                * match &pf.weights {
                    Some(map) => {
                        let (bu, bv) = grid.to_block(px, py);
                        map.bilinear(bu, bv)
                    }
                    None => 1.0,
                };
            if wr < 1e-3 {
                continue;
            }
            f(c, (px, py), v, wr);
        }
    }
}

/// Bilinear lookup in a `width`-wide plane of `rows` rows, clamped to its
/// edges.
#[inline(always)]
fn plane_bilinear(plane: &[f32], width: usize, rows: usize, x: f64, y: f64) -> f32 {
    let x = x.clamp(0.0, (width - 1) as f64);
    let y = y.clamp(0.0, (rows - 1) as f64);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(rows - 1));
    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let top = plane[y0 * width + x0] + (plane[y0 * width + x1] - plane[y0 * width + x0]) * fx;
    let bottom = plane[y1 * width + x0] + (plane[y1 * width + x1] - plane[y1 * width + x0]) * fx;
    top + (bottom - top) * fy
}

type PrepareResult<'a> = io::Result<(
    Vec<f32>,
    Vec<(usize, f32, f32)>,
    usize,
    Vec<PreparedFrame<'a>>,
)>;

/// Merges the accepted frames of `alignment` into `sink`.
pub fn merge_resolution(
    frames: &[&dyn FrameSource],
    alignment: &Alignment,
    params: &ResolutionParams,
    sink: &mut dyn StripSink,
) -> io::Result<MergeReport> {
    let started = Instant::now();
    let reference = frames[alignment.reference];
    let (w, h) = (reference.width(), reference.height());
    let cfa = reference.cfa();
    let grid =
        BlockGrid::new(cfa, w, h).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let region = params.region.unwrap_or(Region {
        x: 0,
        y: 0,
        width: w,
        height: h,
    });
    if region.x + region.width > w
        || region.y + region.height > h
        || region.width < 6
        || region.height < 6
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "region lies outside the frame",
        ));
    }
    let out = OutputGrid::new(region, params.scale);
    let s = params.scale;
    let radius = params.radius;
    let guided = params.color == ColorMerge::GreenGuided;
    // exp(-a) for a in [0, LUT_RANGE].
    let lut: Vec<f32> = (0..=LUT)
        .map(|k| (-(k as f64) / LUT as f64 * LUT_RANGE).exp() as f32)
        .collect();
    let used: Vec<(usize, Affine)> = alignment
        .frames
        .iter()
        .enumerate()
        .filter_map(|(k, f)| f.to_frame.map(|t| (k, t)))
        .collect();
    let robust = match (&params.robustness, &alignment.noise) {
        (Some(p), Some(n)) => Some((p, n)),
        (Some(_), None) if used.len() > 1 => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "robustness needs a noise model, but alignment produced none",
            ));
        }
        _ => None,
    };
    let block_scale = |t: &Affine| {
        // Site transform to block-plane transform.
        t.rescaled(
            1.0 / 3.0,
            -(grid.ox as f64 + 1.0) / 3.0,
            -(grid.oy as f64 + 1.0) / 3.0,
        )
    };
    let (fields, local_report) = match &params.local {
        Some(p) if used.len() > 1 => {
            let (f, r) = residual_fields(frames, alignment, region, radius + 2.0, p)?;
            (f, Some(r))
        }
        _ => (vec![None; frames.len()], None),
    };
    // Frame rows a residual shift can move into the strip.
    let shift_rows = params
        .local
        .as_ref()
        .map_or(0, |p| p.max_shift.ceil() as i64);
    let gains: Vec<f32> = alignment
        .frames
        .iter()
        .map(|f| if params.bracket { f.gain as f32 } else { 1.0 })
        .collect();
    // The shortest used exposure has the largest gain; its white is 1.0.
    let output_scale = match used.iter().map(|&(k, _)| gains[k]).reduce(f32::max) {
        Some(g) if params.bracket && g > 0.0 => 1.0 / g,
        _ => 1.0,
    };
    let noise_weight = match (&alignment.noise, params.bracket) {
        (Some(n), true) => Some((n.a, n.b)),
        _ => None,
    };
    let nthreads = threads().min(used.len()).max(1);
    let wb = reference.calibration().wb;
    // The prior scales with the frame count so that a burst of identical
    // frames merges to exactly the single-frame result.
    let eps = params.prior_weight * used.len() as f32;
    // Green-guided red and blue look up the merged green within a kernel
    // radius (plus a bilinear step) of the strip, so green is merged over
    // that many extra output rows on each side.
    let green_margin = if guided {
        (radius * s).ceil() as usize + 2
    } else {
        0
    };
    let mut stats: Vec<(f32, f32, usize)> = vec![(0.0, 0.0, 0); frames.len()];
    let mut peak = 0usize;
    let mut strips = 0usize;
    let mut raw = Vec::new();
    let mut j0 = 0usize;
    let x_lo = region.x as f64 - radius;
    let x_hi = (region.x + region.width - 1) as f64 + radius;
    let site_rows = |ja: usize, jb: usize| {
        let (_, lo) = out.to_site(0.0, ja as f64);
        let (_, hi) = out.to_site(0.0, (jb - 1) as f64);
        (lo - radius, hi + radius)
    };
    while j0 < out.height {
        let rows = params.strip_rows.min(out.height - j0);
        // Output rows of the first pass: the strip, widened for green.
        let ej0 = j0.saturating_sub(green_margin);
        let ej1 = (j0 + rows + green_margin).min(out.height);
        let erows = ej1 - ej0;
        let (y_lo, y_hi) = site_rows(j0, j0 + rows);
        let (ey_lo, ey_hi) = site_rows(ej0, ej1);
        // Reference block band with a margin for neighborhoods and bilinear
        // lookups.
        let jb_lo = ((grid.to_block(0.0, ey_lo).1).floor() as i64 - 2).max(0) as usize;
        let jb_hi = (((grid.to_block(0.0, ey_hi).1).ceil() as i64 + 2).max(0) as usize)
            .min(grid.height - 1);
        let mut ref_blocks = Vec::new();
        read_block_band(
            reference,
            &grid,
            jb_lo,
            jb_hi + 1 - jb_lo,
            &mut raw,
            &mut ref_blocks,
        )?;
        let ref_band = BlockBand {
            jlo: jb_lo,
            rows: jb_hi + 1 - jb_lo,
            width: grid.width,
            data: ref_blocks,
        };
        let kernels = strip_kernels(
            params,
            &out,
            &grid,
            &ref_band,
            alignment.noise.as_ref(),
            wb,
            ej0,
            erows,
        );
        let splatter = Splatter {
            out: &out,
            kernels: &kernels,
            kernel_j0: ej0,
            lut: &lut,
            scale: s,
            radius,
        };
        let prior = |i: usize, j: usize| -> [f32; 3] {
            let (x, y) = out.to_site(i as f64, j as f64);
            let (bu, bv) = grid.to_block(x, y);
            ref_band
                .bilinear(
                    bu.clamp(0.0, grid.width as f64 - 1.001),
                    bv.clamp(
                        ref_band.jlo as f64,
                        (ref_band.jlo + ref_band.rows) as f64 - 1.001,
                    ),
                )
                .map(|p| [unclip(p[0]), p[1], p[2]])
                .unwrap_or([0.0; 3])
        };
        // In a bracket merge a clipped reference block only bounds the
        // radiance from below; its prior then barely counts.
        let prior_weight = |i: usize, j: usize| -> f32 {
            if !params.bracket {
                return eps;
            }
            let (x, y) = out.to_site(i as f64, j as f64);
            let (bu, bv) = grid.to_block(x, y);
            let clipped = ref_band
                .bilinear(
                    bu.clamp(0.0, grid.width as f64 - 1.001),
                    bv.clamp(
                        ref_band.jlo as f64,
                        (ref_band.jlo + ref_band.rows) as f64 - 1.001,
                    ),
                )
                .is_some_and(|p| p[0] < 0.0);
            if clipped { eps * 1e-3 } else { eps }
        };
        // First pass: all colors over the strip, or green over the widened
        // strip.
        let (stride1, len1) = if guided {
            (2, erows * out.width)
        } else {
            (6, rows * out.width)
        };
        let acc1_bytes = len1 * stride1 * 4;

        // Each worker merges a share of the frames into its own accumulator.
        let shares: Vec<Vec<(usize, Affine)>> = (0..nthreads)
            .map(|t| used.iter().copied().skip(t).step_by(nthreads).collect())
            .collect();
        let results: Vec<PrepareResult<'_>> = std::thread::scope(|scope| {
            let handles: Vec<_> = shares
                .iter()
                .map(|share| {
                    let (ref_band, grid, splatter, fields, gains) =
                        (&ref_band, &grid, &splatter, &fields, &gains);
                    scope.spawn(move || -> PrepareResult {
                        let mut acc = vec![0f32; len1 * stride1];
                        let mut frame_stats = Vec::new();
                        let mut worker_peak = 0usize;
                        let mut prepared = Vec::new();
                        let mut fb_raw = Vec::new();
                        let mut fb = Vec::new();
                        for &(k, to_frame) in share {
                            let frame = frames[k];
                            let to_ref = to_frame.inverse().ok_or_else(|| {
                                io::Error::new(io::ErrorKind::InvalidData, "singular transform")
                            })?;
                            // Frame rows that map into the widened strip.
                            let corners =
                                [(x_lo, ey_lo), (x_hi, ey_lo), (x_lo, ey_hi), (x_hi, ey_hi)];
                            let (mut vmin, mut vmax) = (f64::MAX, f64::MIN);
                            for (x, y) in corners {
                                let (_, v) = to_frame.apply(x, y);
                                vmin = vmin.min(v);
                                vmax = vmax.max(v);
                            }
                            let fy0 =
                                (vmin.floor() as i64 - 1 - shift_rows).clamp(0, h as i64) as usize;
                            let fy1 =
                                (vmax.ceil() as i64 + 2 + shift_rows).clamp(0, h as i64) as usize;
                            if fy1 <= fy0 {
                                frame_stats.push((k, 0.0, 1.0));
                                continue;
                            }
                            let mut raw = vec![0u16; (fy1 - fy0) * w];
                            frame.read_rows(fy0, &mut raw)?;
                            // Robustness against the reference.
                            let weights = match robust {
                                Some((p, noise)) if k != alignment.reference => {
                                    let to_blocks = block_scale(&to_frame);
                                    let (_, bv0) = grid.to_block(0.0, vmin);
                                    let (_, bv1) = grid.to_block(0.0, vmax);
                                    let fj0 = (bv0.floor() as i64 - 2).max(0) as usize;
                                    let fj1 = ((bv1.ceil() as i64 + 2).max(0) as usize)
                                        .min(grid.height - 1);
                                    read_block_band(
                                        frame,
                                        grid,
                                        fj0,
                                        fj1 + 1 - fj0,
                                        &mut fb_raw,
                                        &mut fb,
                                    )?;
                                    let fband = BlockBand {
                                        jlo: fj0,
                                        rows: fj1 + 1 - fj0,
                                        width: grid.width,
                                        data: std::mem::take(&mut fb),
                                    };
                                    let (map, mean, rejected) = robustness(
                                        ref_band, &fband, &to_blocks, noise, wb, p, gains[k],
                                    );
                                    worker_peak =
                                        worker_peak.max(fb_raw.len() * 2 + fband.data.len() * 12);
                                    fb = fband.data;
                                    frame_stats.push((k, mean, rejected));
                                    Some(map)
                                }
                                _ => {
                                    frame_stats.push((k, 1.0, 0.0));
                                    None
                                }
                            };
                            let pf = PreparedFrame {
                                k,
                                to_ref,
                                field: fields[k].as_ref(),
                                gain: gains[k],
                                noise_weight: if k == alignment.reference {
                                    None
                                } else {
                                    noise_weight
                                },
                                fy0,
                                fy1,
                                raw,
                                weights,
                            };
                            if guided {
                                for_each_sample(
                                    frame,
                                    &pf,
                                    grid,
                                    (x_lo, x_hi, ey_lo, ey_hi),
                                    |c, (px, py), v, wr| {
                                        if c == 1 {
                                            let q = out.to_output(px, py);
                                            splatter.splat(&mut acc, 2, 1, 0, ej0, erows, q, v, wr);
                                        }
                                    },
                                );
                            } else {
                                for_each_sample(
                                    frame,
                                    &pf,
                                    grid,
                                    (x_lo, x_hi, y_lo, y_hi),
                                    |c, (px, py), v, wr| {
                                        let q = out.to_output(px, py);
                                        splatter.splat(&mut acc, 6, 3, c, j0, rows, q, v, wr);
                                    },
                                );
                            }
                            prepared.push(pf);
                        }
                        let held: usize = prepared
                            .iter()
                            .map(|p| {
                                p.raw.len() * 2 + p.weights.as_ref().map_or(0, |m| m.data.len() * 4)
                            })
                            .sum();
                        worker_peak = worker_peak.max(held + fb_raw.len() * 2);
                        Ok((acc, frame_stats, worker_peak, prepared))
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let mut total = vec![0f32; len1 * stride1];
        let mut worker_bytes = 0usize;
        let mut prepared_shares = Vec::with_capacity(nthreads);
        for r in results {
            let (acc, fs, wp, prepared) = r?;
            for (t, a) in total.iter_mut().zip(&acc) {
                *t += a;
            }
            for (k, mean, rej) in fs {
                stats[k].0 += mean;
                stats[k].1 += rej;
                stats[k].2 += 1;
            }
            worker_bytes += wp + acc1_bytes;
            prepared_shares.push(prepared);
        }
        let mut rgb = vec![0f32; rows * out.width * 3];
        let mut extra_bytes = 0usize;
        if !guided {
            // Normalize with the reference block prior.
            for jr in 0..rows {
                for i in 0..out.width {
                    let p = prior(i, j0 + jr);
                    let e = prior_weight(i, j0 + jr);
                    let a = (jr * out.width + i) * 6;
                    for c in 0..3 {
                        rgb[(jr * out.width + i) * 3 + c] =
                            (total[a + c] + e * p[c]) / (total[a + 3 + c] + e) * output_scale;
                    }
                }
            }
        } else {
            // Merged green over the widened strip.
            let green: Vec<f32> = (0..erows * out.width)
                .map(|n| {
                    let (i, j) = (n % out.width, ej0 + n / out.width);
                    let (p, e) = (prior(i, j), prior_weight(i, j));
                    (total[2 * n] + e * p[1]) / (total[2 * n + 1] + e)
                })
                .collect();
            drop(total);
            // Second pass: white-balanced red and blue differences to the
            // merged green at each red and blue sample.
            let len2 = rows * out.width;
            let results: Vec<io::Result<Vec<f32>>> = std::thread::scope(|scope| {
                let handles: Vec<_> = prepared_shares
                    .iter()
                    .map(|prepared| {
                        let (grid, splatter, green) = (&grid, &splatter, &green);
                        scope.spawn(move || -> io::Result<Vec<f32>> {
                            let mut acc = vec![0f32; len2 * 4];
                            for pf in prepared {
                                for_each_sample(
                                    frames[pf.k],
                                    pf,
                                    grid,
                                    (x_lo, x_hi, y_lo, y_hi),
                                    |c, (px, py), v, wr| {
                                        if c == 1 {
                                            return;
                                        }
                                        let (qx, qy) = out.to_output(px, py);
                                        let g = plane_bilinear(
                                            green,
                                            out.width,
                                            erows,
                                            qx,
                                            qy - ej0 as f64,
                                        );
                                        let d = v * wb[c] - g * wb[1];
                                        splatter.splat(
                                            &mut acc,
                                            4,
                                            2,
                                            c / 2,
                                            j0,
                                            rows,
                                            (qx, qy),
                                            d,
                                            wr,
                                        );
                                    },
                                );
                            }
                            Ok(acc)
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            let mut diff = vec![0f32; len2 * 4];
            for r in results {
                for (t, a) in diff.iter_mut().zip(&r?) {
                    *t += a;
                }
            }
            for jr in 0..rows {
                for i in 0..out.width {
                    let p = prior(i, j0 + jr);
                    let e = prior_weight(i, j0 + jr);
                    let g = green[(j0 - ej0 + jr) * out.width + i];
                    let a = (jr * out.width + i) * 4;
                    let o = (jr * out.width + i) * 3;
                    rgb[o + 1] = g * output_scale;
                    for (slot, c) in [(0, 0), (1, 2)] {
                        let d = (diff[a + slot] + e * (p[c] * wb[c] - p[1] * wb[1]))
                            / (diff[a + 2 + slot] + e);
                        rgb[o + c] = (g * wb[1] + d) / wb[c].max(1e-6) * output_scale;
                    }
                }
            }
            extra_bytes = green.len() * 4 + diff.len() * 4 * (1 + nthreads);
        }
        drop(prepared_shares);
        peak = peak.max(
            worker_bytes
                + len1 * stride1 * 4
                + extra_bytes
                + rgb.len() * 4
                + kernels.len() * 12
                + ref_band.data.len() * 28
                + raw.len() * 2,
        );
        sink.write_strip(j0, rows, &rgb)?;
        strips += 1;
        j0 += rows;
    }
    Ok(MergeReport {
        width: out.width,
        height: out.height,
        region,
        scale: s,
        frames: alignment
            .frames
            .iter()
            .enumerate()
            .map(|(k, f)| FrameMergeStats {
                label: f.label.clone(),
                used: f.to_frame.is_some(),
                mean_weight: if stats[k].2 > 0 {
                    stats[k].0 / stats[k].2 as f32
                } else {
                    0.0
                },
                rejected_fraction: if stats[k].2 > 0 {
                    stats[k].1 / stats[k].2 as f32
                } else {
                    1.0
                },
                gain: gains[k],
            })
            .collect(),
        strips,
        strip_rows: params.strip_rows,
        threads: nthreads,
        peak_strip_bytes: peak,
        local: local_report,
        output_scale,
        seconds: started.elapsed().as_secs_f64(),
    })
}
