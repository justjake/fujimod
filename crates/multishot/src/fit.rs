//! Global transform fitting from distributed tile correspondences.
//!
//! Bounded deterministic consensus over two-point (similarity) or three-point
//! (affine) hypotheses, then three least-squares refinement and
//! reclassification passes on centered, normalized coordinates. Gates on
//! inlier count and fraction, residuals, 4x4 grid coverage, determinant and
//! corner motion. A rejected fit publishes no transform.

use serde::{Deserialize, Serialize};

/// Maps reference coordinates to frame coordinates:
/// `u = xx*x + xy*y + tx`, `v = yx*x + yy*y + ty`, stored
/// `[xx, xy, yx, yy, tx, ty]`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Affine(pub [f64; 6]);

impl Affine {
    pub const IDENTITY: Affine = Affine([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub fn translation(dx: f64, dy: f64) -> Self {
        Affine([1.0, 0.0, 0.0, 1.0, dx, dy])
    }

    /// Rotation by `radians` about `(cx, cy)`, then translation.
    pub fn rotation_about(radians: f64, cx: f64, cy: f64, dx: f64, dy: f64) -> Self {
        let (s, c) = radians.sin_cos();
        Affine([
            c,
            -s,
            s,
            c,
            cx - c * cx + s * cy + dx,
            cy - s * cx - c * cy + dy,
        ])
    }

    #[inline(always)]
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.0;
        (m[0] * x + m[1] * y + m[4], m[2] * x + m[3] * y + m[5])
    }

    pub fn determinant(&self) -> f64 {
        self.0[0] * self.0[3] - self.0[1] * self.0[2]
    }

    pub fn inverse(&self) -> Option<Affine> {
        let d = self.determinant();
        if d.abs() < 1e-12 {
            return None;
        }
        let [a, b, c, e, tx, ty] = self.0;
        let (ia, ib, ic, ie) = (e / d, -b / d, -c / d, a / d);
        Some(Affine([
            ia,
            ib,
            ic,
            ie,
            -(ia * tx + ib * ty),
            -(ic * tx + ie * ty),
        ]))
    }

    /// `self` after `first`: `x -> self(first(x))`.
    pub fn compose(&self, first: &Affine) -> Affine {
        let [a, b, c, d, e, f] = self.0;
        let [p, q, r, s, t, u] = first.0;
        Affine([
            a * p + b * r,
            a * q + b * s,
            c * p + d * r,
            c * q + d * s,
            a * t + b * u + e,
            c * t + d * u + f,
        ])
    }

    /// The same mapping expressed in coordinates `x' = k*x + o`.
    pub fn rescaled(&self, k: f64, ox: f64, oy: f64) -> Affine {
        let to = Affine([k, 0.0, 0.0, k, ox, oy]);
        let from = Affine([1.0 / k, 0.0, 0.0, 1.0 / k, -ox / k, -oy / k]);
        to.compose(&self.compose(&from))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Model {
    Similarity,
    Affine,
}

/// One correspondence: reference point and its position in the frame.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Correspondence {
    pub x: f64,
    pub y: f64,
    pub u: f64,
    pub v: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FitParams {
    pub model: Model,
    pub inlier_threshold: f64,
    pub min_inliers: usize,
    pub min_inlier_fraction: f64,
    pub max_rms: f64,
    pub min_grid_cells: usize,
    pub determinant_range: (f64, f64),
    pub max_corner_motion: f64,
}

impl Default for FitParams {
    fn default() -> Self {
        Self {
            model: Model::Similarity,
            inlier_threshold: 0.5,
            min_inliers: 12,
            min_inlier_fraction: 0.3,
            max_rms: 0.35,
            min_grid_cells: 8,
            determinant_range: (0.95, 1.05),
            max_corner_motion: 200.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FitResult {
    pub transform: Affine,
    pub inliers: Vec<bool>,
    pub inlier_count: usize,
    pub rms: f64,
    pub grid_cells: usize,
}

fn residual2(t: &Affine, c: &Correspondence) -> f64 {
    let (u, v) = t.apply(c.x, c.y);
    (u - c.u).powi(2) + (v - c.v).powi(2)
}

/// Solves the symmetric positive system `a x = b` in place (Gaussian
/// elimination with partial pivoting). Returns `None` when singular.
pub fn solve<const N: usize>(mut a: [[f64; N]; N], mut b: [f64; N]) -> Option<[f64; N]> {
    for col in 0..N {
        let pivot = (col..N).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in col + 1..N {
            let f = a[row][col] / a[col][col];
            for k in col..N {
                a[row][k] -= f * a[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0; N];
    for row in (0..N).rev() {
        let mut s = b[row];
        for k in row + 1..N {
            s -= a[row][k] * x[k];
        }
        x[row] = s / a[row][row];
    }
    Some(x)
}

/// Least-squares transform over the selected points, in normalized
/// coordinates `(p - center) / scale`.
fn least_squares(
    points: &[Correspondence],
    select: impl Fn(usize) -> bool,
    model: Model,
    center: (f64, f64),
    scale: f64,
) -> Option<Affine> {
    let n = |c: &Correspondence| {
        (
            (c.x - center.0) / scale,
            (c.y - center.1) / scale,
            (c.u - center.0) / scale,
            (c.v - center.1) / scale,
        )
    };
    let normalized = match model {
        Model::Similarity => {
            // u = a x - b y + tx ; v = b x + a y + ty
            let mut ata = [[0.0; 4]; 4];
            let mut atb = [0.0; 4];
            let mut count = 0;
            for (i, c) in points.iter().enumerate() {
                if !select(i) {
                    continue;
                }
                count += 1;
                let (x, y, u, v) = n(c);
                for (row, rhs) in [([x, -y, 1.0, 0.0], u), ([y, x, 0.0, 1.0], v)] {
                    for r in 0..4 {
                        atb[r] += row[r] * rhs;
                        for k in 0..4 {
                            ata[r][k] += row[r] * row[k];
                        }
                    }
                }
            }
            if count < 2 {
                return None;
            }
            let [a, b, tx, ty] = solve(ata, atb)?;
            Affine([a, -b, b, a, tx, ty])
        }
        Model::Affine => {
            let mut ata = [[0.0; 3]; 3];
            let mut atu = [0.0; 3];
            let mut atv = [0.0; 3];
            let mut count = 0;
            for (i, c) in points.iter().enumerate() {
                if !select(i) {
                    continue;
                }
                count += 1;
                let (x, y, u, v) = n(c);
                let row = [x, y, 1.0];
                for r in 0..3 {
                    atu[r] += row[r] * u;
                    atv[r] += row[r] * v;
                    for k in 0..3 {
                        ata[r][k] += row[r] * row[k];
                    }
                }
            }
            if count < 3 {
                return None;
            }
            let [a, b, tx] = solve(ata, atu)?;
            let [c, d, ty] = solve(ata, atv)?;
            Affine([a, b, c, d, tx, ty])
        }
    };
    // Back to plane coordinates: p' = scale * p + center.
    let to = Affine([scale, 0.0, 0.0, scale, center.0, center.1]);
    let from = Affine([
        1.0 / scale,
        0.0,
        0.0,
        1.0 / scale,
        -center.0 / scale,
        -center.1 / scale,
    ]);
    Some(to.compose(&normalized.compose(&from)))
}

/// Deterministic index sequence for hypothesis sampling.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) % n as u64) as usize
    }
}

pub fn fit(
    points: &[Correspondence],
    width: f64,
    height: f64,
    params: &FitParams,
) -> Result<FitResult, String> {
    let n = points.len();
    if n < params.min_inliers.max(3) {
        return Err(format!(
            "{n} correspondences; at least {} needed",
            params.min_inliers.max(3)
        ));
    }
    let center = (width / 2.0, height / 2.0);
    let scale = width.max(height) / 2.0;
    let thr2 = params.inlier_threshold * params.inlier_threshold;
    let needed = match params.model {
        Model::Similarity => 2,
        Model::Affine => 3,
    };

    let mut best: Option<(usize, Affine)> = None;
    let mut rng = Lcg(0x9e3779b97f4a7c15);
    for _ in 0..1024 {
        let mut idx = [0usize; 3];
        for k in 0..needed {
            idx[k] = rng.next(n);
        }
        if idx[..needed]
            .iter()
            .enumerate()
            .any(|(i, a)| idx[..i].contains(a))
        {
            continue;
        }
        let Some(t) = least_squares(
            points,
            |i| idx[..needed].contains(&i),
            params.model,
            center,
            scale,
        ) else {
            continue;
        };
        let count = points.iter().filter(|c| residual2(&t, c) < thr2).count();
        if best.as_ref().is_none_or(|(b, _)| count > *b) {
            best = Some((count, t));
        }
    }
    let (_, mut transform) = best.ok_or("no consensus hypothesis could be formed")?;
    let mut inliers = vec![false; n];
    for _ in 0..3 {
        for (flag, c) in inliers.iter_mut().zip(points) {
            *flag = residual2(&transform, c) < thr2;
        }
        transform = least_squares(points, |i| inliers[i], params.model, center, scale)
            .ok_or("inlier set is degenerate")?;
    }
    for (flag, c) in inliers.iter_mut().zip(points) {
        *flag = residual2(&transform, c) < thr2;
    }
    let inlier_count = inliers.iter().filter(|&&f| f).count();
    let rms = (points
        .iter()
        .zip(&inliers)
        .filter(|(_, f)| **f)
        .map(|(c, _)| residual2(&transform, c))
        .sum::<f64>()
        / inlier_count.max(1) as f64)
        .sqrt();
    let mut cells = [false; 16];
    for (c, &f) in points.iter().zip(&inliers) {
        if f {
            let gx = ((c.x / width * 4.0) as usize).min(3);
            let gy = ((c.y / height * 4.0) as usize).min(3);
            cells[gy * 4 + gx] = true;
        }
    }
    let grid_cells = cells.iter().filter(|&&c| c).count();
    let result = FitResult {
        transform,
        inliers,
        inlier_count,
        rms,
        grid_cells,
    };

    if inlier_count < params.min_inliers {
        return Err(format!(
            "{inlier_count} inliers; at least {} needed",
            params.min_inliers
        ));
    }
    if (inlier_count as f64) < params.min_inlier_fraction * n as f64 {
        return Err(format!("{inlier_count} of {n} correspondences are inliers"));
    }
    if rms > params.max_rms {
        return Err(format!(
            "inlier rms residual {rms:.3} exceeds {}",
            params.max_rms
        ));
    }
    if grid_cells < params.min_grid_cells {
        return Err(format!("inliers cover {grid_cells} of 16 grid cells"));
    }
    let det = transform.determinant();
    if det < params.determinant_range.0 || det > params.determinant_range.1 {
        return Err(format!("transform determinant {det:.4} is out of range"));
    }
    for (x, y) in [(0.0, 0.0), (width, 0.0), (0.0, height), (width, height)] {
        let (u, v) = transform.apply(x, y);
        if ((u - x).powi(2) + (v - y).powi(2)).sqrt() > params.max_corner_motion {
            return Err("corner motion exceeds the limit".into());
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_affine_with_outliers() {
        let truth = Affine::rotation_about(0.003, 200.0, 150.0, 3.25, -1.75);
        let mut pts = Vec::new();
        for j in 0..12 {
            for i in 0..16 {
                let (x, y) = (12.0 + 25.0 * i as f64, 10.0 + 25.0 * j as f64);
                let (mut u, mut v) = truth.apply(x, y);
                if (i * 7 + j * 3) % 5 == 0 {
                    u += 4.0;
                    v -= 3.0;
                }
                pts.push(Correspondence { x, y, u, v });
            }
        }
        let r = fit(&pts, 400.0, 300.0, &FitParams::default()).unwrap();
        for (x, y) in [(0.0, 0.0), (400.0, 300.0)] {
            let (a, b) = r.transform.apply(x, y);
            let (c, d) = truth.apply(x, y);
            assert!((a - c).abs() < 1e-6 && (b - d).abs() < 1e-6);
        }
        let inv = r.transform.inverse().unwrap().compose(&r.transform);
        assert!((inv.0[0] - 1.0).abs() < 1e-9 && inv.0[4].abs() < 1e-9);
    }

    #[test]
    fn rescaled_matches_coordinate_change() {
        let t = Affine::rotation_about(0.01, 50.0, 40.0, 1.5, 2.5);
        let r = t.rescaled(3.0, 1.0, 2.0);
        let (u, v) = t.apply(10.0, 20.0);
        let (a, b) = r.apply(31.0, 62.0);
        assert!((a - (3.0 * u + 1.0)).abs() < 1e-9 && (b - (3.0 * v + 2.0)).abs() < 1e-9);
    }
}
