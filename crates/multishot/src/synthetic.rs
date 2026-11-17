//! Synthetic X-Trans bursts of a known scene, for tests and demonstrations.
//!
//! The scene is defined in reference sensor-site coordinates. Each frame sees
//! it through a known reference-to-frame transform, blurs it with a Gaussian
//! lens, integrates it over each site's full aperture, samples one color per
//! site through the CFA and adds signal-dependent Gaussian noise.

use crate::fit::Affine;
use crate::merge::OutputGrid;
use crate::source::{Calibration, MemoryFrame};
use crate::xtrans::XTrans;

/// Camera-native response of each channel to the white-balanced scene.
pub const CHANNEL_GAIN: [f32; 3] = [0.5, 1.0, 0.6];
pub const WB: [f32; 3] = [2.0, 1.0, 1.0 / 0.6];

/// Gaussian lens blur standard deviation in sites.
pub const LENS_BLUR: f64 = 0.5;

fn erf(x: f64) -> f64 {
    // Abramowitz and Stegun 7.1.26.
    let t = 1.0 / (1.0 + 0.3275911 * x.abs());
    let y = 1.0
        - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t
            + 0.254829592)
            * t
            * (-x * x).exp();
    if x >= 0.0 { y } else { -y }
}

/// Sample offsets and weights of a one-site box aperture convolved with the
/// lens blur, on an 8x8 grid (weights sum to 1).
pub fn aperture_taps() -> Vec<(f64, f64, f32)> {
    const N: usize = 8;
    let half = 0.5 + 2.0 * LENS_BLUR;
    let step = 2.0 * half / N as f64;
    let s = LENS_BLUR * std::f64::consts::SQRT_2;
    let w1: Vec<(f64, f64)> = (0..N)
        .map(|k| {
            let d = -half + (k as f64 + 0.5) * step;
            (d, 0.5 * (erf((d + 0.5) / s) - erf((d - 0.5) / s)))
        })
        .collect();
    let total: f64 = w1.iter().map(|p| p.1).sum();
    let mut taps = Vec::with_capacity(N * N);
    for &(dy, wy) in &w1 {
        for &(dx, wx) in &w1 {
            taps.push((dx, dy, (wx * wy / (total * total)) as f32));
        }
    }
    taps
}

#[derive(Clone, Debug)]
pub struct MovingDisc {
    pub center: (f64, f64),
    pub radius: f64,
    /// Displacement per frame index, in sites.
    pub velocity: (f64, f64),
    pub color: [f32; 3],
}

/// A test chart: slanted edge, Siemens star, fine color texture, a flat
/// patch and an oblique grating, left to right.
#[derive(Clone, Debug)]
pub struct Scene {
    pub width: f64,
    pub height: f64,
    pub moving: Option<MovingDisc>,
}

fn hash(a: i64, b: i64) -> u64 {
    let mut z =
        (a as u64).wrapping_mul(0x9e3779b97f4a7c15) ^ (b as u64).wrapping_mul(0xc2b2ae3d27d4eb4f);
    z ^= z >> 31;
    z = z.wrapping_mul(0xbf58476d1ce4e5b9);
    z ^ (z >> 29)
}

impl Scene {
    pub fn chart(width: usize, height: usize) -> Self {
        Self {
            width: width as f64,
            height: height as f64,
            moving: None,
        }
    }

    /// Region of the slanted edge in sites `(x, y, w, h)`.
    pub fn edge_region(&self) -> (usize, usize, usize, usize) {
        let w = (self.width / 4.0) as usize;
        (
            w / 8,
            (self.height * 0.1) as usize,
            w * 3 / 4,
            (self.height * 0.8) as usize,
        )
    }

    /// Region of fine detail (star and color texture).
    pub fn detail_region(&self) -> (usize, usize, usize, usize) {
        let w = (self.width / 4.0) as usize;
        (w + 4, 4, 2 * w - 8, self.height as usize - 8)
    }

    /// Region of the flat patch.
    pub fn flat_region(&self) -> (usize, usize, usize, usize) {
        let w = (self.width / 4.0) as usize;
        (3 * w + 6, 6, w - 12, (self.height / 2.0) as usize - 12)
    }

    pub fn radiance(&self, x: f64, y: f64, frame: usize) -> [f32; 3] {
        if let Some(m) = &self.moving {
            let cx = m.center.0 + m.velocity.0 * frame as f64;
            let cy = m.center.1 + m.velocity.1 * frame as f64;
            if (x - cx).powi(2) + (y - cy).powi(2) < m.radius * m.radius {
                return m.color;
            }
        }
        let q = self.width / 4.0;
        if x < q {
            // Edge 5 degrees from vertical through the panel center.
            let t = 5f64.to_radians().tan();
            let x0 = q / 2.0 + t * (y - self.height / 2.0);
            return if x < x0 {
                [0.06, 0.06, 0.06]
            } else {
                [0.55, 0.5, 0.45]
            };
        }
        if x < 2.0 * q {
            let (cx, cy) = (1.5 * q, self.height / 2.0);
            let theta = (y - cy).atan2(x - cx);
            let v = if (theta * 24.0).sin() >= 0.0 {
                0.5
            } else {
                0.08
            };
            return [v, v, v];
        }
        if x < 3.0 * q {
            let cell = 1.7;
            let h = hash((x / cell).floor() as i64, (y / cell).floor() as i64);
            let c = |k: u32| 0.08 + 0.45 * ((h >> (k * 8)) & 0xff) as f32 / 255.0;
            return [c(0), c(1), c(2)];
        }
        if y < self.height / 2.0 {
            return [0.3, 0.3, 0.3];
        }
        let v = 0.3 + 0.2 * (2.0 * std::f64::consts::PI * 0.42 * (x * 0.8 + y * 0.6)).sin();
        [v as f32, v as f32, v as f32]
    }

    /// Camera-native scene through the lens and one site's aperture
    /// centered at `(x, y)` in reference coordinates.
    pub fn site_average(&self, x: f64, y: f64, frame: usize) -> [f32; 3] {
        let mut acc = [0f32; 3];
        for (dx, dy, w) in aperture_taps() {
            let r = self.radiance(x + dx, y + dy, frame);
            for c in 0..3 {
                acc[c] += r[c] * CHANNEL_GAIN[c] * w;
            }
        }
        acc
    }

    /// What a full-color sensor at the reference position would record, on
    /// `out`'s grid (camera-native RGB).
    pub fn truth(&self, out: &OutputGrid, frame: usize) -> Vec<f32> {
        let mut rgb = Vec::with_capacity(out.width * out.height * 3);
        for j in 0..out.height {
            for i in 0..out.width {
                let (x, y) = out.to_site(i as f64, j as f64);
                rgb.extend_from_slice(&self.site_average(x, y, frame));
            }
        }
        rgb
    }
}

#[derive(Clone, Copy, Debug)]
pub struct NoiseSpec {
    /// Shot-noise variance per unit of linear signal.
    pub shot: f32,
    /// Read-noise standard deviation in linear units.
    pub read: f32,
}

struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    pub fn gaussian(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }
}

pub const BLACK: f32 = 1024.0;
pub const WHITE: f32 = 16383.0;

/// Renders frame `index` seen through `to_frame` (reference to frame).
pub fn render(
    scene: &Scene,
    width: usize,
    height: usize,
    to_frame: &Affine,
    index: usize,
    noise: Option<NoiseSpec>,
    seed: u64,
) -> MemoryFrame {
    let cfa = XTrans::standard();
    let to_ref = to_frame.inverse().expect("invertible transform");
    let mut rng = Rng(seed ^ (index as u64).wrapping_mul(0x2545f4914f6cdd1d));
    let taps = aperture_taps();
    let mut data = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let c = cfa.color(x, y) as usize;
            let mut v = 0f32;
            for &(dx, dy, w) in &taps {
                let (rx, ry) = to_ref.apply(x as f64 + dx, y as f64 + dy);
                v += scene.radiance(rx, ry, index)[c] * CHANNEL_GAIN[c] * w;
            }
            if let Some(n) = noise {
                let sigma = (n.shot * v.max(0.0) + n.read * n.read).sqrt();
                v += sigma * rng.gaussian() as f32;
            }
            let dn = (BLACK + v * (WHITE - BLACK)).round().clamp(0.0, WHITE);
            data.push(dn as u16);
        }
    }
    MemoryFrame {
        label: format!("synthetic-{index:02}"),
        width,
        height,
        cfa,
        calibration: Calibration::uniform(BLACK, WHITE, WB),
        data,
    }
}

/// Like [`render`] with one sample per site and no lens blur: fast enough
/// for full-size frames (timing and memory checks, not quality).
pub fn render_fast(
    scene: &Scene,
    width: usize,
    height: usize,
    to_frame: &Affine,
    index: usize,
    noise: Option<NoiseSpec>,
    seed: u64,
) -> Vec<u16> {
    let cfa = XTrans::standard();
    let to_ref = to_frame.inverse().expect("invertible transform");
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(16);
    let rows_per = height.div_ceil(threads);
    let mut data = vec![0u16; width * height];
    std::thread::scope(|s| {
        for (t, chunk) in data.chunks_mut(rows_per * width).enumerate() {
            s.spawn(move || {
                let mut rng =
                    Rng(seed ^ (index as u64).wrapping_mul(0x2545f4914f6cdd1d) ^ (t as u64) << 40);
                for (r, row) in chunk.chunks_mut(width).enumerate() {
                    let y = t * rows_per + r;
                    for (x, o) in row.iter_mut().enumerate() {
                        let c = cfa.color(x, y) as usize;
                        let (rx, ry) = to_ref.apply(x as f64, y as f64);
                        let mut v = scene.radiance(rx, ry, index)[c] * CHANNEL_GAIN[c];
                        if let Some(n) = noise {
                            v += (n.shot * v.max(0.0) + n.read * n.read).sqrt()
                                * rng.gaussian() as f32;
                        }
                        *o = (BLACK + v * (WHITE - BLACK)).round().clamp(0.0, WHITE) as u16;
                    }
                }
            });
        }
    });
    data
}

/// Deterministic handheld-like transforms: sub-pixel translations spread
/// over a few sites plus small rotations. Frame 0 is the identity.
pub fn handheld_transforms(count: usize, width: usize, height: usize, seed: u64) -> Vec<Affine> {
    let mut rng = Rng(seed);
    (0..count)
        .map(|k| {
            if k == 0 {
                return Affine::IDENTITY;
            }
            let dx = (rng.uniform() - 0.5) * 6.0;
            let dy = (rng.uniform() - 0.5) * 6.0;
            let rot = (rng.uniform() - 0.5) * 0.004;
            Affine::rotation_about(rot, width as f64 / 2.0, height as f64 / 2.0, dx, dy)
        })
        .collect()
}
