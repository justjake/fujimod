//! Resolution and noise measurements.
//!
//! Slanted-edge MTF follows the ISO 12233 method: per-row edge centroids, a
//! fitted edge line, a 4x oversampled edge spread function along the edge
//! normal, its derivative under a Hamming window, and a discrete Fourier
//! transform corrected for the derivative filter.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EdgeMtf {
    /// Edge angle from the nearest axis, in degrees.
    pub angle_deg: f64,
    /// Frequency where the MTF falls to 0.5, in cycles per pixel of the
    /// measured image.
    pub mtf50: f64,
    /// MTF at 0.25 cycles per pixel.
    pub mtf_at_quarter: f64,
    /// `(cycles per pixel, mtf)`.
    pub curve: Vec<(f64, f64)>,
}

/// White-balanced luma of an interleaved RGB image.
pub fn luma(rgb: &[f32], wb: [f32; 3]) -> Vec<f32> {
    rgb.chunks_exact(3)
        .map(|p| 0.2126 * p[0] * wb[0] + 0.7152 * p[1] * wb[1] + 0.0722 * p[2] * wb[2])
        .collect()
}

/// Crops an interleaved image with `channels` per pixel.
pub fn crop(
    data: &[f32],
    width: usize,
    channels: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
) -> Vec<f32> {
    let mut out = Vec::with_capacity(w * h * channels);
    for row in y..y + h {
        out.extend_from_slice(
            &data[(row * width + x) * channels..(row * width + x + w) * channels],
        );
    }
    out
}

fn transpose(data: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            out[x * h + y] = data[y * w + x];
        }
    }
    out
}

fn fit_line(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    let n = points.len() as f64;
    if n < 4.0 {
        return None;
    }
    let my = points.iter().map(|p| p.0).sum::<f64>() / n;
    let mx = points.iter().map(|p| p.1).sum::<f64>() / n;
    let syy: f64 = points.iter().map(|p| (p.0 - my).powi(2)).sum();
    let sxy: f64 = points.iter().map(|p| (p.0 - my) * (p.1 - mx)).sum();
    if syy <= 0.0 {
        return None;
    }
    let b = sxy / syy;
    Some((mx - b * my, b))
}

/// Measures a single straight edge filling the image (any polarity, within
/// 45 degrees of vertical or horizontal).
pub fn slanted_edge(image: &[f32], width: usize, height: usize) -> Result<EdgeMtf, String> {
    let (mut gx, mut gy) = (0f64, 0f64);
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            gx += (image[y * width + x + 1] - image[y * width + x - 1]) as f64;
            gy += (image[(y + 1) * width + x] - image[(y - 1) * width + x]) as f64;
        }
    }
    // Signed sums: an edge has one polarity, while thin lines and texture
    // (scale ticks, fabric) rise and fall and cancel, so they cannot flip the
    // orientation.
    let (data, w, h) = if gx.abs() >= gy.abs() {
        (image.to_vec(), width, height)
    } else {
        (transpose(image, width, height), height, width)
    };
    // Edge polarity along the measurement axis.
    let polarity = if (if gx.abs() >= gy.abs() { gx } else { gy }) >= 0.0 {
        1.0
    } else {
        -1.0
    };
    // Per row: the peak of the smoothed, polarity-signed derivative, refined
    // by the centroid of its positive lobe. Texture and thin lines barely
    // move a peak, where they would drag a whole-row centroid.
    let mut pts = Vec::new();
    for y in 0..h {
        let row = &data[y * w..(y + 1) * w];
        let d: Vec<f64> = (0..w)
            .map(|x| {
                if x == 0 || x + 1 >= w {
                    0.0
                } else {
                    polarity * (row[x + 1] - row[x - 1]) as f64
                }
            })
            .collect();
        let smooth =
            |x: usize| 0.25 * d[x.saturating_sub(1)] + 0.5 * d[x] + 0.25 * d[(x + 1).min(w - 1)];
        let Some(peak) = (2..w - 2).max_by(|&i, &j| smooth(i).total_cmp(&smooth(j))) else {
            continue;
        };
        let (mut s, mut sx) = (0f64, 0f64);
        for x in peak.saturating_sub(4)..(peak + 5).min(w) {
            if d[x] > 0.0 {
                s += d[x];
                sx += d[x] * x as f64;
            }
        }
        if s > 0.0 {
            pts.push((y as f64, sx / s));
        }
    }
    // Line fit with outlier rejection (rows where texture outshone the edge).
    let mut line = fit_line(&pts).ok_or("no edge found")?;
    for _ in 0..4 {
        let res: Vec<f64> = pts
            .iter()
            .map(|p| (p.1 - (line.0 + line.1 * p.0)).abs())
            .collect();
        let mut sorted = res.clone();
        sorted.sort_by(f64::total_cmp);
        let limit = (3.0 * 1.4826 * sorted[sorted.len() / 2]).max(0.5);
        let kept: Vec<(f64, f64)> = pts
            .iter()
            .zip(&res)
            .filter(|(_, r)| **r <= limit)
            .map(|(p, _)| *p)
            .collect();
        if kept.len() < pts.len() / 2 {
            return Err("edge line fit is unstable; the region holds more than one edge".into());
        }
        line = fit_line(&kept).ok_or("no edge found")?;
    }
    let line = Some(line);
    let (a, b) = line.unwrap();
    let angle = b.atan();
    if b.abs() * (h as f64) < 4.0 {
        return Err(format!(
            "edge slant {:.2} degrees over {h} rows gives too few phases; use a taller region or a more slanted edge",
            angle.to_degrees()
        ));
    }
    let cos = angle.cos();
    let delta = 0.25;
    let half = (((w as f64) / 2.0 - 2.0).min(16.0) / delta) as i64;
    let nbins = (2 * half + 1) as usize;
    let mut sum = vec![0f64; nbins];
    let mut count = vec![0usize; nbins];
    for y in 0..h {
        for x in 0..w {
            let d = (x as f64 - (a + b * y as f64)) * cos;
            let k = (d / delta).round() as i64 + half;
            if k >= 0 && (k as usize) < nbins {
                sum[k as usize] += data[y * w + x] as f64;
                count[k as usize] += 1;
            }
        }
    }
    let mut esf: Vec<Option<f64>> = sum
        .iter()
        .zip(&count)
        .map(|(s, &c)| (c > 0).then(|| s / c as f64))
        .collect();
    // Fill empty bins by linear interpolation.
    let known: Vec<usize> = (0..nbins).filter(|&k| esf[k].is_some()).collect();
    if known.len() < nbins / 2 {
        return Err("edge spread function is too sparse".into());
    }
    for k in 0..nbins {
        if esf[k].is_none() {
            let lo = known.iter().rev().find(|&&i| i < k);
            let hi = known.iter().find(|&&i| i > k);
            esf[k] = Some(match (lo, hi) {
                (Some(&l), Some(&h)) => {
                    let t = (k - l) as f64 / (h - l) as f64;
                    esf[l].unwrap() * (1.0 - t) + esf[h].unwrap() * t
                }
                (Some(&l), None) => esf[l].unwrap(),
                (None, Some(&h)) => esf[h].unwrap(),
                (None, None) => 0.0,
            });
        }
    }
    let esf: Vec<f64> = esf.into_iter().map(|v| v.unwrap()).collect();
    let mut lsf = vec![0f64; nbins];
    for k in 1..nbins - 1 {
        lsf[k] = 0.5 * (esf[k + 1] - esf[k - 1]);
    }
    let total: f64 = lsf.iter().sum();
    if total.abs() < 1e-9 {
        return Err("edge has no contrast".into());
    }
    let center = lsf
        .iter()
        .enumerate()
        .map(|(k, v)| k as f64 * v)
        .sum::<f64>()
        / total;
    let span = nbins as f64;
    for (k, v) in lsf.iter_mut().enumerate() {
        let t = (k as f64 - center) / span;
        let window = if t.abs() <= 0.5 {
            0.54 + 0.46 * (2.0 * std::f64::consts::PI * t).cos()
        } else {
            0.0
        };
        *v *= window;
    }
    let dc: f64 = lsf.iter().sum();
    let mut curve = Vec::new();
    let mut f = 0.0;
    while f <= 1.0 + 1e-9 {
        let (mut re, mut im) = (0f64, 0f64);
        for (k, v) in lsf.iter().enumerate() {
            let phase = -2.0 * std::f64::consts::PI * f * (k as f64 - center) * delta;
            re += v * phase.cos();
            im += v * phase.sin();
        }
        let arg = 2.0 * std::f64::consts::PI * f * delta;
        let correction = if arg > 1e-9 { arg.sin() / arg } else { 1.0 };
        curve.push((f, (re * re + im * im).sqrt() / dc.abs() / correction));
        f += 0.005;
    }
    let mtf50 = curve
        .windows(2)
        .find(|p| p[0].1 >= 0.5 && p[1].1 < 0.5)
        .map(|p| p[0].0 + (p[0].1 - 0.5) / (p[0].1 - p[1].1) * (p[1].0 - p[0].0))
        .unwrap_or(1.0);
    let mtf_at_quarter = curve
        .iter()
        .min_by(|x, y| (x.0 - 0.25).abs().total_cmp(&(y.0 - 0.25).abs()))
        .unwrap()
        .1;
    Ok(EdgeMtf {
        angle_deg: angle.to_degrees(),
        mtf50,
        mtf_at_quarter,
        curve,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PatchNoise {
    pub mean: [f64; 3],
    pub std: [f64; 3],
    /// Standard deviation of white-balanced luma (plane removed) relative to
    /// its mean.
    pub luma_relative_std: f64,
}

/// Residual of a least-squares plane `a + b*x + c*y` fitted to `v`.
fn detrend(v: &[f64], width: usize) -> Vec<f64> {
    let mut ata = [[0f64; 3]; 3];
    let mut atb = [0f64; 3];
    for (i, &z) in v.iter().enumerate() {
        let row = [1.0, (i % width) as f64, (i / width) as f64];
        for a in 0..3 {
            atb[a] += row[a] * z;
            for b in 0..3 {
                ata[a][b] += row[a] * row[b];
            }
        }
    }
    let k = crate::fit::solve(ata, atb).unwrap_or([0.0; 3]);
    v.iter()
        .enumerate()
        .map(|(i, &z)| z - (k[0] + k[1] * (i % width) as f64 + k[2] * (i / width) as f64))
        .collect()
}

/// Noise of a flat patch `width` pixels wide: standard deviations after
/// removing a fitted plane, so slow shading does not count as noise.
pub fn patch_noise(rgb: &[f32], width: usize, wb: [f32; 3]) -> PatchNoise {
    let n = (rgb.len() / 3) as f64;
    let rms = |r: &[f64]| (r.iter().map(|v| v * v).sum::<f64>() / n).sqrt();
    let mut mean = [0f64; 3];
    let mut std = [0f64; 3];
    for c in 0..3 {
        let v: Vec<f64> = rgb.chunks_exact(3).map(|p| p[c] as f64).collect();
        mean[c] = v.iter().sum::<f64>() / n;
        std[c] = rms(&detrend(&v, width));
    }
    let l: Vec<f64> = luma(rgb, wb).iter().map(|&v| v as f64).collect();
    let lm = l.iter().sum::<f64>() / n;
    PatchNoise {
        mean,
        std,
        luma_relative_std: rms(&detrend(&l, width)) / lm.abs().max(1e-12),
    }
}

/// Root-mean-square difference of two equally sized images.
pub fn rmse(a: &[f32], b: &[f32]) -> f64 {
    (a.iter()
        .zip(b)
        .map(|(x, y)| ((x - y) as f64).powi(2))
        .sum::<f64>()
        / a.len() as f64)
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(w: usize, h: usize, angle_deg: f64, blur: f64) -> Vec<f32> {
        let t = angle_deg.to_radians().tan();
        let mut img = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                // Box-integrated edge with Gaussian-like blur via supersampling.
                let mut s = 0.0;
                for sy in 0..8 {
                    for sx in 0..8 {
                        let xx = x as f64 + (sx as f64 + 0.5) / 8.0 - 0.5;
                        let yy = y as f64 + (sy as f64 + 0.5) / 8.0 - 0.5;
                        let d = xx - (w as f64 / 2.0 + t * yy);
                        s += 0.5 * (1.0 + (d / blur.max(1e-6) / 2f64.sqrt()).tanh());
                    }
                }
                img[y * w + x] = 0.1 + 0.6 * (s / 64.0) as f32;
            }
        }
        img
    }

    #[test]
    fn sharper_edge_measures_higher() {
        let a = slanted_edge(&edge(40, 60, 5.0, 0.3), 40, 60).unwrap();
        let b = slanted_edge(&edge(40, 60, 5.0, 1.2), 40, 60).unwrap();
        assert!((a.angle_deg - 5.0).abs() < 0.3, "{}", a.angle_deg);
        assert!(a.mtf50 > b.mtf50 * 1.5, "{} vs {}", a.mtf50, b.mtf50);
        // A horizontal edge measures the same as a vertical one.
        let v = edge(48, 64, 5.0, 0.6);
        let ht = transpose(&v, 48, 64);
        let c = slanted_edge(&v, 48, 64).unwrap();
        let d = slanted_edge(&ht, 64, 48).unwrap();
        assert!((c.mtf50 - d.mtf50).abs() < 0.01);
    }
}
