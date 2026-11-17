//! Burst alignment on block-luma planes.
//!
//! Per-tile translation matching on box pyramids (zero-mean normalized
//! correlation squared, bounded symmetric search, texture, correlation and
//! competing-peak tests, quadratic sub-pixel peaks and a reverse closure
//! check) feeds a global consensus fit (see [`crate::fit`]). The fit is then
//! refined by Gauss-Newton image registration over the inlier tiles, which
//! removes the sub-pixel bias of correlation peak interpolation, and finally
//! on full-resolution green, which avoids the aliasing of 3x3 block means on
//! fine detail. Each frame is
//! matched against the retained reference, using the previous frame's
//! transform as the search prior.

use crate::fit::{Affine, Correspondence, FitParams, FitResult, fit};
use crate::noise::NoiseModel;
use crate::plane::{BlockGrid, Plane, green_plane, read_block_planes};
use crate::source::FrameSource;
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchParams {
    pub levels: usize,
    pub patch_radius: i64,
    pub coarse_search: i64,
    pub refine_search: i64,
    /// Grid spacing on the finest (block) level; a multiple of `2^(levels-1)`.
    pub grid_step: usize,
    /// Minimum standard deviation of a reference patch (linear units).
    pub min_std: f32,
    pub min_coarse_score: f32,
    pub min_score: f32,
    /// A second peak above this fraction of the best rejects the tile.
    pub peak_ratio: f32,
    pub reverse_tolerance: f64,
}

impl Default for MatchParams {
    fn default() -> Self {
        Self {
            levels: 4,
            patch_radius: 8,
            coarse_search: 4,
            refine_search: 2,
            grid_step: 24,
            min_std: 0.002,
            min_coarse_score: 0.3,
            min_score: 0.5,
            peak_ratio: 0.9,
            reverse_tolerance: 0.5,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MatchStats {
    pub grid_points: usize,
    pub accepted: usize,
    pub low_texture: usize,
    pub low_score: usize,
    pub ambiguous: usize,
    pub boundary: usize,
    pub reverse_failed: usize,
}

pub struct Pyramid {
    pub levels: Vec<Plane>,
}

impl Pyramid {
    pub fn new(base: Plane, levels: usize) -> Self {
        let mut out = vec![base];
        for _ in 1..levels {
            let next = out.last().unwrap().downsample2();
            out.push(next);
        }
        Self { levels: out }
    }
}

/// Zero-mean normalized correlation, squared when positive, and the
/// reference patch standard deviation.
fn zncc2(a: &Plane, ax: i64, ay: i64, b: &Plane, bx: i64, by: i64, r: i64) -> Option<(f32, f32)> {
    for (p, x, y) in [(a, ax, ay), (b, bx, by)] {
        if x - r < 0 || y - r < 0 || x + r >= p.width as i64 || y + r >= p.height as i64 {
            return None;
        }
    }
    let n = ((2 * r + 1) * (2 * r + 1)) as f64;
    let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0f64, 0f64, 0f64, 0f64, 0f64);
    for dy in -r..=r {
        let ra =
            &a.data[((ay + dy) as usize) * a.width + (ax - r) as usize..][..(2 * r + 1) as usize];
        let rb =
            &b.data[((by + dy) as usize) * b.width + (bx - r) as usize..][..(2 * r + 1) as usize];
        for (&va, &vb) in ra.iter().zip(rb) {
            let (va, vb) = (va as f64, vb as f64);
            sa += va;
            sb += vb;
            saa += va * va;
            sbb += vb * vb;
            sab += va * vb;
        }
    }
    let va = saa - sa * sa / n;
    let vb = sbb - sb * sb / n;
    let cov = sab - sa * sb / n;
    let std_a = (va.max(0.0) / n).sqrt() as f32;
    if va <= 0.0 || vb <= 0.0 || cov <= 0.0 {
        return Some((0.0, std_a));
    }
    Some(((cov * cov / (va * vb)) as f32, std_a))
}

enum Outcome {
    Accepted(f64, f64),
    LowTexture,
    LowScore,
    Ambiguous,
    Boundary,
    ReverseFailed,
}

/// Integer search around `(cx + dx0, cy + dy0)` in `b` for the patch at
/// `(cx, cy)` in `a`. Returns the score grid (row-major, side `2s+1`).
fn search(a: &Plane, b: &Plane, cx: i64, cy: i64, dx0: i64, dy0: i64, s: i64, r: i64) -> Vec<f32> {
    let side = 2 * s + 1;
    let mut scores = vec![-1f32; (side * side) as usize];
    for j in -s..=s {
        for i in -s..=s {
            if let Some((score, _)) = zncc2(a, cx, cy, b, cx + dx0 + i, cy + dy0 + j, r) {
                scores[((j + s) * side + i + s) as usize] = score;
            }
        }
    }
    scores
}

fn argmax(scores: &[f32]) -> (usize, f32) {
    scores.iter().enumerate().fold(
        (0, f32::MIN),
        |acc, (i, &s)| if s > acc.1 { (i, s) } else { acc },
    )
}

fn parabola(m: f32, c: f32, p: f32) -> f64 {
    let denom = m - 2.0 * c + p;
    if denom >= -1e-9 {
        return 0.0;
    }
    ((m - p) / (2.0 * denom)).clamp(-0.5, 0.5) as f64
}

/// Finest-level search with sub-pixel peak; `None` if the peak is on the
/// window boundary.
fn refine_finest(
    a: &Plane,
    b: &Plane,
    cx: i64,
    cy: i64,
    dx0: i64,
    dy0: i64,
    s: i64,
    r: i64,
) -> Option<(f64, f64, f32)> {
    let side = 2 * s + 1;
    let scores = search(a, b, cx, cy, dx0, dy0, s, r);
    let (k, best) = argmax(&scores);
    let (i, j) = ((k as i64 % side), (k as i64 / side));
    if i == 0 || j == 0 || i == side - 1 || j == side - 1 || best < 0.0 {
        return None;
    }
    let at = |ii: i64, jj: i64| scores[(jj * side + ii) as usize];
    let fx = parabola(at(i - 1, j), best, at(i + 1, j));
    let fy = parabola(at(i, j - 1), best, at(i, j + 1));
    Some(((dx0 + i - s) as f64 + fx, (dy0 + j - s) as f64 + fy, best))
}

fn match_point(
    reference: &Pyramid,
    frame: &Pyramid,
    px: i64,
    py: i64,
    prior: &Affine,
    p: &MatchParams,
) -> Outcome {
    let top = p.levels - 1;
    let f = 1i64 << top;
    let (ux, uy) = prior.apply(px as f64, py as f64);
    let (mut dx, mut dy) = (
        ((ux - px as f64) / f as f64).round() as i64,
        ((uy - py as f64) / f as f64).round() as i64,
    );
    // Coarsest level: full search with competing-peak test.
    let (a, b) = (&reference.levels[top], &frame.levels[top]);
    let (cx, cy) = (px >> top, py >> top);
    match zncc2(a, cx, cy, a, cx, cy, p.patch_radius) {
        Some((_, std)) if std >= p.min_std => {}
        Some(_) => return Outcome::LowTexture,
        None => return Outcome::Boundary,
    }
    let s = p.coarse_search;
    let side = 2 * s + 1;
    let scores = search(a, b, cx, cy, dx, dy, s, p.patch_radius);
    let (k, best) = argmax(&scores);
    let (bi, bj) = (k as i64 % side, k as i64 / side);
    if best < p.min_coarse_score {
        return Outcome::LowScore;
    }
    if bi == 0 || bj == 0 || bi == side - 1 || bj == side - 1 {
        return Outcome::Boundary;
    }
    let second = scores
        .iter()
        .enumerate()
        .filter(|(i, _)| ((*i as i64 % side) - bi).abs() > 1 || ((*i as i64 / side) - bj).abs() > 1)
        .map(|(_, &v)| v)
        .fold(f32::MIN, f32::max);
    if second > p.peak_ratio * best {
        return Outcome::Ambiguous;
    }
    dx += bi - s;
    dy += bj - s;
    // Intermediate levels.
    for level in (1..top).rev() {
        let (a, b) = (&reference.levels[level], &frame.levels[level]);
        let (cx, cy) = (px >> level, py >> level);
        let s = p.refine_search;
        let side = 2 * s + 1;
        let scores = search(a, b, cx, cy, 2 * dx, 2 * dy, s, p.patch_radius);
        let (k, best) = argmax(&scores);
        let (bi, bj) = (k as i64 % side, k as i64 / side);
        if best < p.min_coarse_score {
            return Outcome::LowScore;
        }
        if bi == 0 || bj == 0 || bi == side - 1 || bj == side - 1 {
            return Outcome::Boundary;
        }
        dx = 2 * dx + bi - s;
        dy = 2 * dy + bj - s;
    }
    let (a, b) = (&reference.levels[0], &frame.levels[0]);
    let (dx0, dy0) = if top > 0 { (2 * dx, 2 * dy) } else { (dx, dy) };
    let Some((fx, fy, score)) =
        refine_finest(a, b, px, py, dx0, dy0, p.refine_search, p.patch_radius)
    else {
        return Outcome::Boundary;
    };
    if score < p.min_score {
        return Outcome::LowScore;
    }
    // Reverse closure from the rounded endpoint back to the reference.
    let (qx, qy) = (px + fx.round() as i64, py + fy.round() as i64);
    let Some((rx, ry, _)) = refine_finest(
        b,
        a,
        qx,
        qy,
        -fx.round() as i64,
        -fy.round() as i64,
        p.refine_search,
        p.patch_radius,
    ) else {
        return Outcome::ReverseFailed;
    };
    // The reverse displacement from the rounded endpoint should undo the
    // forward one.
    if ((fx + rx).powi(2) + (fy + ry).powi(2)).sqrt() > p.reverse_tolerance {
        return Outcome::ReverseFailed;
    }
    Outcome::Accepted(fx, fy)
}

fn threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(16)
}

/// Matches the reference against a frame on a distributed grid.
pub fn match_planes(
    reference: &Pyramid,
    frame: &Pyramid,
    prior: &Affine,
    p: &MatchParams,
) -> (Vec<Correspondence>, MatchStats) {
    let base = &reference.levels[0];
    let align = 1usize << (p.levels - 1);
    let step = p.grid_step.next_multiple_of(align);
    let margin = ((p.patch_radius + p.coarse_search + 1) as usize * align).next_multiple_of(align);
    let mut grid = Vec::new();
    let mut y = margin;
    while y + margin < base.height {
        let mut x = margin;
        while x + margin < base.width {
            grid.push((x as i64, y as i64));
            x += step;
        }
        y += step;
    }
    let n = threads();
    let chunk = grid.len().div_ceil(n).max(1);
    let outcomes: Vec<Vec<Outcome>> = std::thread::scope(|s| {
        let handles: Vec<_> = grid
            .chunks(chunk)
            .map(|pts| {
                s.spawn(move || {
                    pts.iter()
                        .map(|&(x, y)| match_point(reference, frame, x, y, prior, p))
                        .collect()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut stats = MatchStats {
        grid_points: grid.len(),
        ..Default::default()
    };
    let mut out = Vec::new();
    for (&(x, y), o) in grid.iter().zip(outcomes.into_iter().flatten()) {
        match o {
            Outcome::Accepted(dx, dy) => {
                stats.accepted += 1;
                out.push(Correspondence {
                    x: x as f64,
                    y: y as f64,
                    u: x as f64 + dx,
                    v: y as f64 + dy,
                });
            }
            Outcome::LowTexture => stats.low_texture += 1,
            Outcome::LowScore => stats.low_score += 1,
            Outcome::Ambiguous => stats.ambiguous += 1,
            Outcome::Boundary => stats.boundary += 1,
            Outcome::ReverseFailed => stats.reverse_failed += 1,
        }
    }
    (out, stats)
}

fn gradients(p: &Plane) -> (Plane, Plane) {
    let mut gx = Plane::new(p.width, p.height);
    let mut gy = Plane::new(p.width, p.height);
    for y in 1..p.height - 1 {
        for x in 1..p.width - 1 {
            let i = y * p.width + x;
            gx.data[i] = 0.5 * (p.data[i + 1] - p.data[i - 1]);
            gy.data[i] = 0.5 * (p.data[i + p.width] - p.data[i - p.width]);
        }
    }
    (gx, gy)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RefineReport {
    pub iterations: usize,
    pub pixels: usize,
    pub residual_rms: f64,
    pub last_step: f64,
}

type Normal = ([[f64; 8]; 8], [f64; 8], f64, f64, usize);

/// Normal equations, robust cost and plain squared residual sum at one
/// parameter set.
#[allow(clippy::too_many_arguments)]
fn evaluate(
    reference: &Plane,
    frame: &Plane,
    gx: &Plane,
    gy: &Plane,
    mask: &[bool],
    stride: usize,
    t: Affine,
    gain: f64,
    offset: f64,
    huber: f64,
) -> Normal {
    let (cx, cy) = (reference.width as f64 / 2.0, reference.height as f64 / 2.0);
    let sc = reference.width.max(reference.height) as f64 / 2.0;
    let rows: Vec<usize> = (2..reference.height - 2).step_by(stride).collect();
    let chunk = rows.len().div_ceil(threads()).max(1);
    let partials: Vec<Normal> = std::thread::scope(|s| {
        let handles: Vec<_> = rows
            .chunks(chunk)
            .map(|rows| {
                s.spawn(move || {
                    let mut ata = [[0f64; 8]; 8];
                    let mut atb = [0f64; 8];
                    let (mut cost, mut ss, mut count) = (0f64, 0f64, 0usize);
                    for &y in rows {
                        for x in (2..reference.width - 2).step_by(stride) {
                            if !mask[y * reference.width + x] {
                                continue;
                            }
                            let (u, v) = t.apply(x as f64, y as f64);
                            let (Some(f), Some(fx), Some(fy)) =
                                (frame.bicubic(u, v), gx.bilinear(u, v), gy.bilinear(u, v))
                            else {
                                continue;
                            };
                            let (f, fx, fy) = (f as f64, fx as f64 * gain, fy as f64 * gain);
                            let r = reference.at(x, y) as f64 - (gain * f + offset);
                            let (xn, yn) = ((x as f64 - cx) / sc, (y as f64 - cy) / sc);
                            let j = [fx * xn, fx * yn, fy * xn, fy * yn, fx, fy, f, 1.0];
                            let a = r.abs();
                            let (w, rho) = if a <= huber {
                                (1.0, 0.5 * r * r)
                            } else {
                                (huber / a, huber * (a - 0.5 * huber))
                            };
                            for p in 0..8 {
                                atb[p] += w * j[p] * r;
                                for q in p..8 {
                                    ata[p][q] += w * j[p] * j[q];
                                }
                            }
                            cost += rho;
                            ss += r * r;
                            count += 1;
                        }
                    }
                    (ata, atb, cost, ss, count)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut total: Normal = ([[0f64; 8]; 8], [0f64; 8], 0.0, 0.0, 0);
    for (a, b, c, s, n) in partials {
        for i in 0..8 {
            total.1[i] += b[i];
            for k in i..8 {
                total.0[i][k] += a[i][k];
            }
        }
        total.2 += c;
        total.3 += s;
        total.4 += n;
    }
    for i in 0..8 {
        for k in 0..i {
            total.0[i][k] = total.0[k][i];
        }
    }
    total
}

/// Gauss-Newton refinement of `init` (reference -> frame), with gain and
/// offset, over reference pixels where `mask` is set. Huber-weighted, with
/// step halving whenever the mean robust cost does not decrease.
pub fn refine_affine(
    reference: &Plane,
    frame: &Plane,
    init: Affine,
    mask: &[bool],
    iterations: usize,
    stride: usize,
) -> (Affine, RefineReport) {
    let (gx, gy) = gradients(frame);
    let (cx, cy) = (reference.width as f64 / 2.0, reference.height as f64 / 2.0);
    let sc = reference.width.max(reference.height) as f64 / 2.0;
    let mut report = RefineReport {
        iterations: 0,
        pixels: 0,
        residual_rms: 0.0,
        last_step: 0.0,
    };
    let eval = |t: Affine, g: f64, o: f64, huber: f64| {
        evaluate(reference, frame, &gx, &gy, mask, stride, t, g, o, huber)
    };
    let first = eval(init, 1.0, 0.0, f64::INFINITY);
    if first.4 < 64 {
        return (init, report);
    }
    let huber = 2.0 * (first.3 / first.4 as f64).sqrt().max(1e-6);
    let apply = |t: Affine, g: f64, o: f64, d: [f64; 8], k: f64| {
        let [dxx, dxy, dyx, dyy, dtx, dty, dg, doff] = d.map(|v| v * k);
        let (dxx, dxy, dyx, dyy) = (dxx / sc, dxy / sc, dyx / sc, dyy / sc);
        let mut m = t.0;
        m[0] += dxx;
        m[1] += dxy;
        m[2] += dyx;
        m[3] += dyy;
        m[4] += dtx - dxx * cx - dxy * cy;
        m[5] += dty - dyx * cx - dyy * cy;
        let size =
            (dtx * dtx + dty * dty).sqrt() + (dxx.abs() + dxy.abs() + dyx.abs() + dyy.abs()) * sc;
        (Affine(m), g + dg, o + doff, size)
    };
    let (mut t, mut gain, mut offset) = (init, 1.0f64, 0.0f64);
    let mut current = eval(t, gain, offset, huber);
    for it in 0..iterations {
        report.iterations = it + 1;
        report.pixels = current.4;
        report.residual_rms = (current.3 / current.4.max(1) as f64).sqrt();
        let mut a = current.0;
        for i in 0..8 {
            a[i][i] *= 1.0 + 1e-6;
        }
        let Some(d) = crate::fit::solve(a, current.1) else {
            break;
        };
        let mean_cost = current.2 / current.4.max(1) as f64;
        let mut scale = 1.0;
        let mut accepted = false;
        for _ in 0..6 {
            let (nt, ng, no, size) = apply(t, gain, offset, d, scale);
            let next = eval(nt, ng, no, huber);
            if next.4 >= 64 && next.2 / next.4 as f64 <= mean_cost {
                t = nt;
                gain = ng;
                offset = no;
                current = next;
                report.last_step = size;
                accepted = true;
                break;
            }
            scale *= 0.5;
        }
        if !accepted || report.last_step < 1e-4 {
            break;
        }
    }
    report.pixels = current.4;
    report.residual_rms = (current.3 / current.4.max(1) as f64).sqrt();
    (t, report)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlignParams {
    pub matching: MatchParams,
    pub fit: FitParams,
    pub refine_iterations: usize,
    pub refine_stride: usize,
    /// Minimum luma gradient (per block) of a refinement pixel.
    pub refine_min_gradient: f32,
    /// Final refinement on full-resolution green, sampling the reference's
    /// green sites every `fine_stride` rows and columns. Zero iterations
    /// skips it.
    pub fine_iterations: usize,
    pub fine_stride: usize,
    /// Subject rectangle `[x, y, width, height]` in registration-plane
    /// coordinates (block plane: sites / 3). When set, only tiles and
    /// refinement pixels inside it determine the transform, so a subject at
    /// a different depth from the background is aligned instead of the
    /// background.
    pub subject: Option<[f64; 4]>,
    /// Global translation search radius, in coarsest-level pixels, before
    /// tile matching. Zero disables it.
    pub global_search: i64,
}

impl Default for AlignParams {
    fn default() -> Self {
        Self {
            matching: MatchParams::default(),
            fit: FitParams::default(),
            refine_iterations: 20,
            refine_stride: 1,
            refine_min_gradient: 0.004,
            fine_iterations: 10,
            fine_stride: 1,
            global_search: 16,
            subject: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameAlignment {
    pub label: String,
    /// Reference active-area coordinates to this frame's active-area
    /// coordinates. Present only for an accepted frame.
    pub to_frame: Option<Affine>,
    /// The same in block-plane coordinates, before and after refinement.
    pub block_fit: Option<Affine>,
    pub block_refined: Option<Affine>,
    pub matches: MatchStats,
    pub inliers: usize,
    pub fit_rms: f64,
    pub refine: Option<RefineReport>,
    pub fine: Option<RefineReport>,
    pub rejected: Option<String>,
    /// Factor that brings this frame's linear values to the reference's
    /// exposure: the median ratio of reference to frame block luma over
    /// mid-tones both expose without clipping.
    #[serde(default = "unit_gain")]
    pub gain: f64,
}

fn unit_gain() -> f64 {
    1.0
}

/// Median ratio of reference to frame block luma at the frame position
/// `t` maps each reference block to, over blocks where both lie in
/// `[0.01, 0.5]` (well above black, well below clipping).
pub fn exposure_gain(reference: &Plane, frame: &Plane, t: &Affine) -> Option<f64> {
    let mut ratios = Vec::new();
    for y in (1..reference.height - 1).step_by(2) {
        for x in (1..reference.width - 1).step_by(2) {
            let r = reference.at(x, y);
            let (u, v) = t.apply(x as f64, y as f64);
            let Some(f) = frame.bilinear(u, v) else {
                continue;
            };
            if (0.01..=0.5).contains(&r) && (0.01..=0.5).contains(&f) {
                ratios.push(r / f);
            }
        }
    }
    if ratios.len() < 1000 {
        return None;
    }
    let mid = ratios.len() / 2;
    ratios.select_nth_unstable_by(mid, f32::total_cmp);
    Some(ratios[mid] as f64)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Alignment {
    pub reference: usize,
    pub width: usize,
    pub height: usize,
    pub frames: Vec<FrameAlignment>,
    pub noise: Option<NoiseModel>,
    pub peak_bytes: usize,
}

impl Alignment {
    pub fn accepted(&self) -> usize {
        self.frames.iter().filter(|f| f.to_frame.is_some()).count()
    }
}

/// For an exposure bracket (recorded exposures spanning more than 1/6 EV),
/// the frame with the shortest exposure: it has the least motion blur and
/// the fewest clipped highlights, while gradient energy would favor the
/// brightest frame. `None` for a burst or when any frame lacks a shutter
/// time.
pub fn shortest_exposure(frames: &[&dyn FrameSource]) -> Option<usize> {
    let values: Vec<f64> = frames
        .iter()
        .map(|f| {
            let e = f.exposure();
            let n = e.f_number.unwrap_or(1.0);
            Some(e.seconds? * e.iso.unwrap_or(100) as f64 / (n * n))
        })
        .collect::<Option<_>>()?;
    let (lo, hi) = values
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    if !(lo > 0.0) || (hi / lo).log2() <= 1.0 / 6.0 {
        return None;
    }
    values.iter().position(|&v| v == lo)
}

/// Picks the sharpest frame by block-luma gradient energy over the middle
/// band of rows.
pub fn sharpest(frames: &[&dyn FrameSource]) -> io::Result<usize> {
    let mut best = (0, f64::MIN);
    for (k, f) in frames.iter().enumerate() {
        let planes = read_block_planes(*f, false)?;
        let p = &planes.luma;
        let mut e = 0f64;
        for y in p.height * 3 / 8..p.height * 5 / 8 {
            for x in 1..p.width - 1 {
                let i = y * p.width + x;
                let l = 4.0 * p.data[i]
                    - p.data[i - 1]
                    - p.data[i + 1]
                    - p.data[i - p.width]
                    - p.data[i + p.width];
                e += (l * l) as f64;
            }
        }
        if e > best.1 {
            best = (k, e);
        }
    }
    Ok(best.0)
}

/// Pixels used for refinement: textured pixels (gradient above `min_gradient`)
/// that are not near a tile whose motion disagreed with the fit.
fn refine_mask(
    luma: &Plane,
    step: usize,
    matches: &[Correspondence],
    fit: &FitResult,
    min_gradient: f32,
) -> Vec<bool> {
    let (width, height) = (luma.width, luma.height);
    let mut mask = vec![false; width * height];
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let i = y * width + x;
            let gx = luma.data[i + 1] - luma.data[i - 1];
            let gy = luma.data[i + width] - luma.data[i - width];
            mask[i] = 0.5 * (gx * gx + gy * gy).sqrt() > min_gradient;
        }
    }
    let half = (step / 2) as i64;
    for (c, &ok) in matches.iter().zip(&fit.inliers) {
        if ok {
            continue;
        }
        let (x0, y0) = (c.x as i64, c.y as i64);
        for y in (y0 - half).max(0)..(y0 + half).min(height as i64) {
            for x in (x0 - half).max(0)..(x0 + half).min(width as i64) {
                mask[y as usize * width + x as usize] = false;
            }
        }
    }
    mask
}

/// Result of registering one luma plane against a reference pyramid.
#[derive(Clone, Debug)]
pub struct Registration {
    pub matches: MatchStats,
    pub fit: Option<FitResult>,
    /// Refined reference-to-frame transform in plane coordinates.
    pub refined: Option<Affine>,
    pub refine: Option<RefineReport>,
    /// Reference pixels used for refinement.
    pub mask: Option<Vec<bool>>,
    pub rejected: Option<String>,
}

/// Zero-mean normalized correlation (squared when positive) of a centered
/// rectangle of `a` against `b` displaced by `(dx, dy)`.
fn zncc2_rect(a: &Plane, b: &Plane, rx: i64, ry: i64, dx: i64, dy: i64) -> f32 {
    let (cx, cy) = (a.width as i64 / 2, a.height as i64 / 2);
    let (mut sa, mut sb, mut saa, mut sbb, mut sab, mut n) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
    for y in cy - ry..=cy + ry {
        let by = y + dy;
        if by < 0 || by >= b.height as i64 {
            continue;
        }
        for x in cx - rx..=cx + rx {
            let bx = x + dx;
            if bx < 0 || bx >= b.width as i64 {
                continue;
            }
            let va = a.data[y as usize * a.width + x as usize] as f64;
            let vb = b.data[by as usize * b.width + bx as usize] as f64;
            sa += va;
            sb += vb;
            saa += va * va;
            sbb += vb * vb;
            sab += va * vb;
            n += 1.0;
        }
    }
    if n < 16.0 {
        return 0.0;
    }
    let va = saa - sa * sa / n;
    let vb = sbb - sb * sb / n;
    let cov = sab - sa * sb / n;
    if va <= 0.0 || vb <= 0.0 || cov <= 0.0 {
        return 0.0;
    }
    (cov * cov / (va * vb)) as f32
}

/// Corrects the prior's translation by matching the central 60% of the
/// coarsest level as one patch, searching `global_search` coarse pixels
/// around the prior. Large hand motion between frames then stays within the
/// tile search.
fn global_prior(
    reference: &Pyramid,
    frame: &Pyramid,
    prior: &Affine,
    search: i64,
) -> (Affine, f32) {
    let top = reference.levels.len() - 1;
    let (a, b) = (&reference.levels[top], &frame.levels[top]);
    let f = (1i64 << top) as f64;
    let (cx, cy) = (
        reference.levels[0].width as f64 / 2.0,
        reference.levels[0].height as f64 / 2.0,
    );
    let (u, v) = prior.apply(cx, cy);
    let (pdx, pdy) = (((u - cx) / f).round() as i64, ((v - cy) / f).round() as i64);
    let (rx, ry) = (
        (a.width as f64 * 0.3) as i64,
        (a.height as f64 * 0.3) as i64,
    );
    let candidates: Vec<(i64, i64)> = (-search..=search)
        .flat_map(|j| (-search..=search).map(move |i| (i, j)))
        .collect();
    let chunk = candidates.len().div_ceil(threads()).max(1);
    let best = std::thread::scope(|s| {
        let handles: Vec<_> = candidates
            .chunks(chunk)
            .map(|c| {
                s.spawn(move || {
                    c.iter()
                        .map(|&(i, j)| (zncc2_rect(a, b, rx, ry, pdx + i, pdy + j), i, j))
                        .fold((f32::MIN, 0, 0), |acc, v| if v.0 > acc.0 { v } else { acc })
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .fold((f32::MIN, 0, 0), |acc, v| if v.0 > acc.0 { v } else { acc })
    });
    let (score, i, j) = best;
    (
        Affine::translation(i as f64 * f, j as f64 * f).compose(prior),
        score,
    )
}

/// Tile matching, consensus fit and Gauss-Newton refinement of one plane.
pub fn register(
    reference: &Pyramid,
    frame: &Plane,
    prior: &Affine,
    params: &AlignParams,
) -> Registration {
    let base = &reference.levels[0];
    let pyr = Pyramid::new(frame.clone(), params.matching.levels);
    let prior = if params.global_search > 0 {
        global_prior(reference, &pyr, prior, params.global_search).0
    } else {
        *prior
    };
    let (mut matches, stats) = match_planes(reference, &pyr, &prior, &params.matching);
    let inside = |x: f64, y: f64| {
        params
            .subject
            .is_none_or(|[sx, sy, sw, sh]| x >= sx && y >= sy && x < sx + sw && y < sy + sh)
    };
    matches.retain(|c| inside(c.x, c.y));
    match fit(&matches, base.width as f64, base.height as f64, &params.fit) {
        Err(reason) => Registration {
            matches: stats,
            fit: None,
            refined: None,
            refine: None,
            mask: None,
            rejected: Some(reason),
        },
        Ok(result) => {
            let mut mask = refine_mask(
                base,
                params.matching.grid_step,
                &matches,
                &result,
                params.refine_min_gradient,
            );
            if params.subject.is_some() {
                for (i, m) in mask.iter_mut().enumerate() {
                    *m &= inside((i % base.width) as f64, (i / base.width) as f64);
                }
            }
            let (refined, report) = refine_affine(
                base,
                frame,
                result.transform,
                &mask,
                params.refine_iterations,
                params.refine_stride,
            );
            Registration {
                matches: stats,
                fit: Some(result),
                refined: Some(refined),
                refine: Some(report),
                mask: Some(mask),
                rejected: None,
            }
        }
    }
}

/// Aligns every frame to `reference`.
pub fn align(
    frames: &[&dyn FrameSource],
    reference: usize,
    params: &AlignParams,
) -> io::Result<Alignment> {
    let rf = frames[reference];
    let ref_planes = read_block_planes(rf, true)?;
    let grid = ref_planes.grid;
    let ref_pyr = Pyramid::new(ref_planes.luma.clone(), params.matching.levels);
    let plane_bytes = grid.width * grid.height * 4;
    let mut peak = plane_bytes * 3 * 2 + plane_bytes * 4;
    let mut out = Vec::new();
    let mut prior = Affine::IDENTITY;
    let mut noise = None;
    let mut ref_green: Option<Plane> = None;
    for (k, f) in frames.iter().enumerate() {
        if k == reference {
            out.push(FrameAlignment {
                label: f.label(),
                to_frame: Some(Affine::IDENTITY),
                block_fit: Some(Affine::IDENTITY),
                block_refined: Some(Affine::IDENTITY),
                matches: MatchStats::default(),
                inliers: 0,
                fit_rms: 0.0,
                refine: None,
                fine: None,
                rejected: None,
                gain: 1.0,
            });
            continue;
        }
        if (f.width(), f.height(), f.cfa()) != (rf.width(), rf.height(), rf.cfa()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} has a different geometry or CFA phase than the reference",
                    f.label()
                ),
            ));
        }
        let planes = read_block_planes(*f, noise.is_none())?;
        let reg = register(&ref_pyr, &planes.luma, &prior, params);
        let mut entry = FrameAlignment {
            label: f.label(),
            to_frame: None,
            block_fit: reg.fit.as_ref().map(|r| r.transform),
            block_refined: reg.refined,
            matches: reg.matches.clone(),
            inliers: reg.fit.as_ref().map_or(0, |r| r.inlier_count),
            fit_rms: reg.fit.as_ref().map_or(0.0, |r| r.rms),
            refine: reg.refine.clone(),
            fine: None,
            rejected: reg.rejected.clone(),
            gain: 1.0,
        };
        match (reg.refined, reg.mask) {
            (None, _) | (_, None) => {}
            (Some(refined), Some(mask)) => {
                let mut site = block_to_site(&grid, &refined);
                if params.fine_iterations > 0 {
                    if ref_green.is_none() {
                        ref_green = Some(green_plane(rf)?);
                    }
                    let g = ref_green.as_ref().unwrap();
                    let cfa = rf.cfa();
                    // Reference green sites inside refinement blocks of this
                    // frame's block mask.
                    let mut fine_mask = vec![false; g.width * g.height];
                    for y in 0..g.height {
                        for x in 0..g.width {
                            if cfa.color(x, y) != crate::xtrans::GREEN {
                                continue;
                            }
                            let (u, v) = grid.to_block(x as f64, y as f64);
                            let (u, v) = (u.round(), v.round());
                            if u >= 0.0
                                && v >= 0.0
                                && (u as usize) < grid.width
                                && (v as usize) < grid.height
                            {
                                fine_mask[y * g.width + x] =
                                    mask[v as usize * grid.width + u as usize];
                            }
                        }
                    }
                    let fg = green_plane(*f)?;
                    let (fine, fine_report) = refine_affine(
                        g,
                        &fg,
                        site,
                        &fine_mask,
                        params.fine_iterations,
                        params.fine_stride,
                    );
                    site = fine;
                    entry.fine = Some(fine_report);
                    peak = peak.max(g.data.len() * 4 * 4 + fine_mask.len() + g.data.len() * 2);
                }
                entry.gain = exposure_gain(&ref_planes.luma, &planes.luma, &refined).unwrap_or(1.0);
                entry.to_frame = Some(site);
                prior = refined;
                if noise.is_none() {
                    noise = NoiseModel::estimate(
                        &ref_planes.rgb,
                        &planes.rgb,
                        &grid,
                        &refined,
                        entry.gain as f32,
                    );
                }
            }
        }
        peak = peak.max(plane_bytes * 3 * 4 + plane_bytes * 4 * 2 + plane_bytes * 3);
        out.push(entry);
    }
    Ok(Alignment {
        reference,
        width: rf.width(),
        height: rf.height(),
        frames: out,
        noise,
        peak_bytes: peak,
    })
}

/// Converts a block-plane transform to active-area site coordinates.
pub fn block_to_site(grid: &BlockGrid, t: &Affine) -> Affine {
    t.rescaled(3.0, grid.ox as f64 + 1.0, grid.oy as f64 + 1.0)
}
