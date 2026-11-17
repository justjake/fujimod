//! Pixel shift: frames taken with the sensor moved by commanded stabilizer
//! offsets.
//!
//! The commanded offset of each frame (in stabilizer counts) is converted to
//! an expected image displacement in sites and given to the alignment as that
//! frame's prior. The alignment then measures the actual displacement from the
//! image, because the sensor's position also drifts between frames (on the
//! X-H2S by 0.3 to 0.6 sites between frames commanded to the same position),
//! so commanded offsets alone place samples wrongly. The merge then puts every
//! frame's samples at their measured positions on the output grid.
//!
//! On the X-H2S, +X counts move the image toward -X and +Y counts toward +Y,
//! by about 1/56 site per count (research note on the commanded sensor shift).

use crate::align::Alignment;
use crate::fit::Affine;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShiftPlan {
    /// Commanded stabilizer offset of every frame, in counts.
    pub counts: Vec<[f64; 2]>,
    /// Image displacement per count, in sites, per axis (signed).
    pub sites_per_count: [f64; 2],
}

impl ShiftPlan {
    /// X-H2S scale and polarity.
    pub const XH2S_SITES_PER_COUNT: [f64; 2] = [-1.0 / 56.0, 1.0 / 56.0];

    /// Expected image displacement of frame `k` relative to `reference`.
    pub fn commanded(&self, k: usize, reference: usize) -> [f64; 2] {
        let [cx, cy] = self.counts[k];
        let [rx, ry] = self.counts[reference];
        [
            (cx - rx) * self.sites_per_count[0],
            (cy - ry) * self.sites_per_count[1],
        ]
    }

    /// A 16-position plan: four whole-site offsets (0,0), (1,0), (0,1), (1,1)
    /// times four half-site phases (0,0), (1/2,0), (0,1/2), (1/2,1/2). The
    /// first position is home; every other position is followed by a frame
    /// back at home, 31 frames in all. The home frames sample the sensor's
    /// drift for [`ShiftFrame::predicted`].
    pub fn sixteen_shot(sites_per_count: [f64; 2]) -> Self {
        let mut counts = vec![[0.0, 0.0]];
        for whole in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]] {
            for half in [[0.0, 0.0], [0.5, 0.0], [0.0, 0.5], [0.5, 0.5]] {
                let site = [whole[0] + half[0], whole[1] + half[1]];
                if site == [0.0, 0.0] {
                    continue;
                }
                counts.push([site[0] / sites_per_count[0], site[1] / sites_per_count[1]]);
                counts.push([0.0, 0.0]);
            }
        }
        Self {
            counts,
            sites_per_count,
        }
    }

    /// Alignment priors (reference to frame, site coordinates).
    pub fn priors(&self, reference: usize) -> Vec<Affine> {
        (0..self.counts.len())
            .map(|k| {
                let [dx, dy] = self.commanded(k, reference);
                Affine::translation(dx, dy)
            })
            .collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShiftFrame {
    pub label: String,
    pub counts: [f64; 2],
    pub commanded: [f64; 2],
    /// Displacement of the frame's center measured by the alignment, `None`
    /// when the frame was rejected.
    pub measured: Option<[f64; 2]>,
    /// Measured minus commanded.
    pub difference: Option<[f64; 2]>,
    /// Commanded plus the drift measured at the nearest frames at the
    /// reference's position before and after this one, interpolated
    /// linearly in frame order (the nearer one alone at either end). Those
    /// frames themselves are predicted at their measured position. `None`
    /// without a measured frame at the reference's position.
    pub predicted: Option<[f64; 2]>,
}

/// Least-squares line `measured = slope * counts + intercept` for one axis,
/// over frames commanded on that axis or at zero on both.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AxisFit {
    pub sites_per_count: f64,
    pub intercept: f64,
    pub rms_residual: f64,
    pub frames: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShiftReport {
    pub reference: usize,
    pub sites_per_count: [f64; 2],
    pub frames: Vec<ShiftFrame>,
    /// Spread (largest minus smallest) of the measured displacement among
    /// frames commanded to the reference's position.
    pub zero_spread: Option<[f64; 2]>,
    pub fit: [Option<AxisFit>; 2],
}

fn line_fit(points: &[(f64, f64)]) -> Option<AxisFit> {
    let n = points.len() as f64;
    if points.len() < 3 {
        return None;
    }
    let mx = points.iter().map(|p| p.0).sum::<f64>() / n;
    let my = points.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = points.iter().map(|p| (p.0 - mx).powi(2)).sum();
    if sxx <= 0.0 {
        return None;
    }
    let slope = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum::<f64>() / sxx;
    let intercept = my - slope * mx;
    let rms = (points
        .iter()
        .map(|p| (p.1 - slope * p.0 - intercept).powi(2))
        .sum::<f64>()
        / n)
        .sqrt();
    Some(AxisFit {
        sites_per_count: slope,
        intercept,
        rms_residual: rms,
        frames: points.len(),
    })
}

/// `alignment` with every accepted frame placed at its predicted
/// displacement (a translation). Rejected frames stay rejected; frames
/// without a prediction keep their measured transform.
pub fn predicted_alignment(report: &ShiftReport, alignment: &Alignment) -> Alignment {
    let mut a = alignment.clone();
    for (f, s) in a.frames.iter_mut().zip(&report.frames) {
        if let (Some(_), Some([dx, dy])) = (f.to_frame, s.predicted) {
            f.to_frame = Some(Affine::translation(dx, dy));
        }
    }
    a
}

/// Commanded against measured displacement for every frame.
pub fn report(plan: &ShiftPlan, alignment: &Alignment) -> ShiftReport {
    let (cx, cy) = (alignment.width as f64 / 2.0, alignment.height as f64 / 2.0);
    let r = alignment.reference;
    let frames: Vec<ShiftFrame> = alignment
        .frames
        .iter()
        .enumerate()
        .map(|(k, f)| {
            let commanded = plan.commanded(k, r);
            let measured = f.to_frame.map(|t| {
                let (u, v) = t.apply(cx, cy);
                [u - cx, v - cy]
            });
            ShiftFrame {
                label: f.label.clone(),
                counts: plan.counts[k],
                commanded,
                measured,
                difference: measured.map(|m| [m[0] - commanded[0], m[1] - commanded[1]]),
                predicted: None,
            }
        })
        .collect();
    let at_reference = |k: usize| plan.counts[k] == plan.counts[r];
    // Drift samples: measured minus commanded at the reference's position.
    let samples: Vec<(usize, [f64; 2])> = frames
        .iter()
        .enumerate()
        .filter(|(k, _)| at_reference(*k))
        .filter_map(|(k, f)| f.difference.map(|d| (k, d)))
        .collect();
    let mut frames = frames;
    for (k, f) in frames.iter_mut().enumerate() {
        let before = samples.iter().rev().find(|s| s.0 <= k);
        let after = samples.iter().find(|s| s.0 >= k);
        let drift = match (before, after) {
            (Some(&(i, a)), Some(&(j, b))) if j > i => {
                let t = (k - i) as f64 / (j - i) as f64;
                Some([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t])
            }
            (Some(&(_, a)), _) | (None, Some(&(_, a))) => Some(a),
            (None, None) => None,
        };
        f.predicted = drift.map(|d| [f.commanded[0] + d[0], f.commanded[1] + d[1]]);
    }
    let zeros: Vec<[f64; 2]> = frames
        .iter()
        .enumerate()
        .filter(|(k, _)| at_reference(*k))
        .filter_map(|(_, f)| f.measured)
        .collect();
    let zero_spread = (zeros.len() > 1).then(|| {
        let span = |a: usize| {
            let v = zeros.iter().map(|z| z[a]);
            v.clone().fold(f64::MIN, f64::max) - v.fold(f64::MAX, f64::min)
        };
        [span(0), span(1)]
    });
    let fit = [0usize, 1].map(|a| {
        let other = 1 - a;
        let points: Vec<(f64, f64)> = frames
            .iter()
            .filter(|f| f.counts[other] == plan.counts[r][other])
            .filter_map(|f| f.measured.map(|m| (f.counts[a] - plan.counts[r][a], m[a])))
            .collect();
        line_fit(&points)
    });
    ShiftReport {
        reference: r,
        sites_per_count: plan.sites_per_count,
        frames,
        zero_spread,
        fit,
    }
}
