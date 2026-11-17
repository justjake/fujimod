//! Signal-dependent noise of 3x3 block means, measured from two aligned
//! frames of the burst.

use crate::fit::Affine;
use crate::plane::BlockGrid;
use serde::{Deserialize, Serialize};

/// Variance of one frame's block mean per channel: `a * mean + b`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoiseModel {
    pub a: [f32; 3],
    pub b: [f32; 3],
}

const BINS: usize = 20;

impl NoiseModel {
    #[inline(always)]
    pub fn variance(&self, channel: usize, mean: f32) -> f32 {
        (self.a[channel] * mean.max(0.0) + self.b[channel]).max(1e-10)
    }

    /// Variance, in reference units, of a frame exposed `1 / gain` times
    /// the reference and scaled by `gain`, at reference level `mean`.
    #[inline(always)]
    pub fn scaled_variance(&self, channel: usize, mean: f32, gain: f32) -> f32 {
        (gain * self.a[channel] * mean.max(0.0) + gain * gain * self.b[channel]).max(1e-10)
    }

    /// Fits the model to differences between the reference and a frame
    /// sampled through `t` (reference block coordinates to frame block
    /// coordinates) and scaled by `gain` to the reference's exposure. Robust
    /// to edges and moving content (median absolute deviation per brightness
    /// bin). A scaled frame's shot noise is `gain` times the reference's at
    /// the same level, which the normalization assumes.
    pub fn estimate(
        reference: &[[f32; 3]],
        frame: &[[f32; 3]],
        grid: &BlockGrid,
        t: &Affine,
        gain: f32,
    ) -> Option<Self> {
        if reference.len() != grid.width * grid.height || frame.len() != reference.len() {
            return None;
        }
        // Per channel and brightness bin: (local texture, normalized difference).
        let mut bins: Vec<Vec<Vec<(f32, f32)>>> = vec![vec![Vec::new(); BINS]; 3];
        let w = grid.width;
        for j in (2..grid.height - 2).step_by(2) {
            for i in (2..w - 2).step_by(2) {
                let r = reference[j * w + i];
                let neighbors = [
                    reference[j * w + i - 1],
                    reference[j * w + i + 1],
                    reference[(j - 1) * w + i],
                    reference[(j + 1) * w + i],
                ];
                if r[0] < 0.0 || neighbors.iter().any(|p| p[0] < 0.0) {
                    continue;
                }
                let (u, v) = t.apply(i as f64, j as f64);
                if !(u >= 0.0 && v >= 0.0) {
                    continue;
                }
                let (x0, y0) = (u.floor() as usize, v.floor() as usize);
                if x0 + 1 >= w || y0 + 1 >= grid.height {
                    continue;
                }
                let (fx, fy) = ((u - x0 as f64) as f32, (v - y0 as f64) as f32);
                let q = [
                    frame[y0 * w + x0],
                    frame[y0 * w + x0 + 1],
                    frame[(y0 + 1) * w + x0],
                    frame[(y0 + 1) * w + x0 + 1],
                ];
                if q.iter().any(|p| p[0] < 0.0) {
                    continue;
                }
                // Interpolation averages the frame's noise; normalize so the
                // difference has the variance of one frame's block mean.
                let wts = [
                    (1.0 - fx) * (1.0 - fy),
                    fx * (1.0 - fy),
                    (1.0 - fx) * fy,
                    fx * fy,
                ];
                let norm = (1.0 + gain * wts.iter().map(|w| w * w).sum::<f32>()).sqrt();
                for c in 0..3 {
                    let s: f32 = gain * wts.iter().zip(&q).map(|(w, p)| w * p[c]).sum::<f32>();
                    let texture = neighbors
                        .iter()
                        .map(|p| (p[c] - r[c]).abs())
                        .fold(0f32, f32::max);
                    let level = r[c].clamp(0.0, 1.0);
                    let bin = ((level.sqrt() * BINS as f32) as usize).min(BINS - 1);
                    bins[c][bin].push((texture, (s - r[c]) / norm));
                }
            }
        }
        let mut a = [0f32; 3];
        let mut b = [0f32; 3];
        for c in 0..3 {
            // (intensity, variance of one frame, sample count)
            let mut points = Vec::new();
            for (k, entries) in bins[c].iter_mut().enumerate() {
                if entries.len() < 400 {
                    continue;
                }
                // Keep the flattest quarter of the bin: edges and residual
                // misalignment are not noise.
                let quarter = entries.len() / 4;
                entries.select_nth_unstable_by(quarter, |x, y| x.0.total_cmp(&y.0));
                let mut flat: Vec<f32> = entries[..quarter].iter().map(|e| e.1).collect();
                let diffs = &mut flat;
                let n = diffs.len();
                let mid = n / 2;
                diffs.select_nth_unstable_by(mid, f32::total_cmp);
                let median = diffs[mid];
                let mut dev: Vec<f32> = diffs.iter().map(|d| (d - median).abs()).collect();
                dev.select_nth_unstable_by(mid, f32::total_cmp);
                let sigma_diff = 1.4826 * dev[mid] as f64;
                let lo = (k as f64 / BINS as f64).powi(2);
                let hi = ((k + 1) as f64 / BINS as f64).powi(2);
                points.push((0.5 * (lo + hi), sigma_diff * sigma_diff, n as f64));
            }
            if points.is_empty() {
                return None;
            }
            // Bins without flat content report texture as noise. Fit the
            // lower envelope: drop bins far above the model and refit.
            let line = |pts: &[(f64, f64, f64)]| {
                let sw: f64 = pts.iter().map(|p| p.2).sum();
                let mx = pts.iter().map(|p| p.0 * p.2).sum::<f64>() / sw;
                let my = pts.iter().map(|p| p.1 * p.2).sum::<f64>() / sw;
                let sxx: f64 = pts.iter().map(|p| p.2 * (p.0 - mx).powi(2)).sum();
                let sxy: f64 = pts.iter().map(|p| p.2 * (p.0 - mx) * (p.1 - my)).sum();
                let slope = if sxx > 0.0 { (sxy / sxx).max(0.0) } else { 0.0 };
                (slope, (my - slope * mx).max(my * 0.05).max(1e-12))
            };
            let (mut slope, mut intercept) = line(&points);
            for _ in 0..8 {
                let kept: Vec<_> = points
                    .iter()
                    .copied()
                    .filter(|p| p.1 <= 1.5 * (slope * p.0 + intercept))
                    .collect();
                if kept.len() < 2 || kept.len() == points.len() {
                    break;
                }
                points = kept;
                (slope, intercept) = line(&points);
            }
            a[c] = slope as f32;
            b[c] = intercept as f32;
        }
        Some(Self { a, b })
    }
}
