//! Local alignment: per-tile residual shifts on top of each frame's global
//! transform.
//!
//! One affine transform per frame leaves sub-site errors that differ across
//! the frame (lens distortion, rolling shutter, parallax). For every tile of
//! the reference, a translation (with gain and offset) is refined by
//! Gauss-Newton on full-resolution green, sampled at the reference's real
//! green sites, starting from the global transform. A tile is measured only
//! when its texture determines the shift to within `max_error` sites.
//! Outliers against the 3x3 median are dropped and unmeasured tiles are
//! filled from their neighbors, so a missing tile falls back toward the
//! global transform rather than to an arbitrary shift.

use crate::align::Alignment;
use crate::fit::{Affine, solve};
use crate::merge::Region;
use crate::plane::{Plane, green_band};
use crate::source::FrameSource;
use crate::xtrans::GREEN;
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalParams {
    /// Tile size in sites.
    pub tile: usize,
    /// Largest residual shift, in sites, a tile may take.
    pub max_shift: f64,
    pub iterations: usize,
    /// Largest standard error of a measured tile's shift, in sites.
    pub max_error: f64,
    /// Smallest ratio of the tile's gradient tensor eigenvalues. A tile
    /// whose texture runs in one direction (a lone edge) does not determine
    /// the shift along it.
    pub min_isotropy: f64,
    /// Distance from the 3x3 median, in sites, beyond which a tile's shift
    /// is an outlier.
    pub outlier: f64,
}

impl Default for LocalParams {
    fn default() -> Self {
        Self {
            tile: 96,
            max_shift: 1.5,
            iterations: 8,
            max_error: 0.05,
            min_isotropy: 0.3,
            outlier: 0.3,
        }
    }
}

/// Residual shifts of one frame, in frame sites, to add after its global
/// transform. Tile `(i, j)` is centered on reference site
/// `(x0 + (i + 0.5) * tile, y0 + (j + 0.5) * tile)`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShiftField {
    pub x0: f64,
    pub y0: f64,
    pub tile: f64,
    pub cols: usize,
    pub rows: usize,
    pub shifts: Vec<[f32; 2]>,
}

impl ShiftField {
    /// Shift at reference site `(x, y)`, bilinear between tile centers and
    /// constant beyond the outer centers.
    #[inline(always)]
    pub fn at(&self, x: f64, y: f64) -> (f64, f64) {
        let u = ((x - self.x0) / self.tile - 0.5).clamp(0.0, (self.cols - 1) as f64);
        let v = ((y - self.y0) / self.tile - 0.5).clamp(0.0, (self.rows - 1) as f64);
        let (i0, j0) = (u.floor() as usize, v.floor() as usize);
        let (i1, j1) = ((i0 + 1).min(self.cols - 1), (j0 + 1).min(self.rows - 1));
        let (fx, fy) = (u - i0 as f64, v - j0 as f64);
        let s = |i: usize, j: usize, c: usize| self.shifts[j * self.cols + i][c] as f64;
        let lerp = |c: usize| {
            let top = s(i0, j0, c) + (s(i1, j0, c) - s(i0, j0, c)) * fx;
            let bottom = s(i0, j1, c) + (s(i1, j1, c) - s(i0, j1, c)) * fx;
            top + (bottom - top) * fy
        };
        (lerp(0), lerp(1))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FrameLocalStats {
    pub label: String,
    pub tiles: usize,
    pub measured: usize,
    pub outliers: usize,
    /// RMS and largest length of the final shifts, in sites.
    pub rms_shift: f64,
    pub max_shift: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LocalReport {
    pub frames: Vec<FrameLocalStats>,
    pub seconds: f64,
}

/// Translation, gain and offset of one tile; `None` when the tile's texture
/// does not determine the shift.
#[allow(clippy::too_many_arguments)]
fn refine_tile(
    reference: &Plane,
    ref_y0: usize,
    frame: &Plane,
    frame_y0: usize,
    to_frame: &Affine,
    sites: &[(usize, usize)],
    params: &LocalParams,
) -> Option<[f64; 2]> {
    if sites.len() < 64 {
        return None;
    }
    let mut d = [0f64; 2];
    let (mut gain, mut offset) = (1f64, 0f64);
    let mut error = f64::INFINITY;
    for _ in 0..params.iterations {
        let mut ata = [[0f64; 4]; 4];
        let mut atb = [0f64; 4];
        let (mut ss, mut n) = (0f64, 0usize);
        for &(x, y) in sites {
            let (u, v) = to_frame.apply(x as f64, y as f64);
            let (u, v) = (u + d[0], v + d[1] - frame_y0 as f64);
            let (Some(f), Some(fl), Some(fr), Some(fu), Some(fd)) = (
                frame.bicubic(u, v),
                frame.bicubic(u - 0.5, v),
                frame.bicubic(u + 0.5, v),
                frame.bicubic(u, v - 0.5),
                frame.bicubic(u, v + 0.5),
            ) else {
                continue;
            };
            let (f, fx, fy) = (f as f64, (fr - fl) as f64, (fd - fu) as f64);
            let r = reference.at(x, y - ref_y0) as f64 - (gain * f + offset);
            let j = [gain * fx, gain * fy, f, 1.0];
            for p in 0..4 {
                atb[p] += j[p] * r;
                for q in p..4 {
                    ata[p][q] += j[p] * j[q];
                }
            }
            ss += r * r;
            n += 1;
        }
        if n < 64 {
            return None;
        }
        for p in 0..4 {
            for q in 0..p {
                ata[p][q] = ata[q][p];
            }
        }
        // Standard error of the shift: residual variance times the largest
        // eigenvalue of the shift block of the inverse normal matrix.
        let var = ss / (n - 4).max(1) as f64;
        let (a, b, c) = (ata[0][0], ata[0][1], ata[1][1]);
        let disc = (0.25 * (a - c) * (a - c) + b * b).sqrt();
        let (l1, l2) = (0.5 * (a + c) + disc, 0.5 * (a + c) - disc);
        if l1.is_nan() || l1 <= 0.0 || l2 < params.min_isotropy * l1 {
            return None;
        }
        let step = solve(ata, atb)?;
        let mut worst = 0f64;
        for axis in 0..2 {
            let mut e = [0f64; 4];
            e[axis] = 1.0;
            let col = solve(ata, e)?;
            worst = worst.max(col[axis]);
        }
        error = (var * worst).sqrt();
        d[0] += step[0];
        d[1] += step[1];
        gain += step[2];
        offset += step[3];
        if d[0].hypot(d[1]) > params.max_shift || !(gain > 0.5 && gain < 2.0) {
            return None;
        }
        if step[0].hypot(step[1]) < 1e-3 {
            break;
        }
    }
    (error <= params.max_error).then_some(d)
}

fn median(mut v: Vec<f32>) -> f32 {
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// Drops outliers against the 3x3 median of measured neighbors and fills
/// unmeasured tiles from measured neighbors, growing outward. Returns the
/// number of outliers.
fn clean(measured: &mut [Option<[f32; 2]>], cols: usize, rows: usize, outlier: f64) -> usize {
    let snapshot = measured.to_vec();
    let mut outliers = 0;
    for j in 0..rows {
        for i in 0..cols {
            let Some(s) = snapshot[j * cols + i] else {
                continue;
            };
            let mut nx = Vec::new();
            let mut ny = Vec::new();
            for jj in j.saturating_sub(1)..(j + 2).min(rows) {
                for ii in i.saturating_sub(1)..(i + 2).min(cols) {
                    if (ii, jj) != (i, j)
                        && let Some(n) = snapshot[jj * cols + ii]
                    {
                        nx.push(n[0]);
                        ny.push(n[1]);
                    }
                }
            }
            if nx.len() < 3 {
                continue;
            }
            let (mx, my) = (median(nx), median(ny));
            if ((s[0] - mx) as f64).hypot((s[1] - my) as f64) > outlier {
                measured[j * cols + i] = None;
                outliers += 1;
            }
        }
    }
    outliers
}

fn fill(measured: &[Option<[f32; 2]>], cols: usize, rows: usize) -> Vec<[f32; 2]> {
    let mut cur = measured.to_vec();
    loop {
        let missing = cur.iter().filter(|s| s.is_none()).count();
        if missing == 0 || missing == cur.len() {
            break;
        }
        let prev = cur.clone();
        for j in 0..rows {
            for i in 0..cols {
                if prev[j * cols + i].is_some() {
                    continue;
                }
                let (mut sx, mut sy, mut n) = (0f32, 0f32, 0f32);
                for jj in j.saturating_sub(1)..(j + 2).min(rows) {
                    for ii in i.saturating_sub(1)..(i + 2).min(cols) {
                        if let Some(s) = prev[jj * cols + ii] {
                            sx += s[0];
                            sy += s[1];
                            n += 1.0;
                        }
                    }
                }
                if n > 0.0 {
                    cur[j * cols + i] = Some([sx / n, sy / n]);
                }
            }
        }
    }
    cur.into_iter().map(|s| s.unwrap_or([0.0, 0.0])).collect()
}

/// Residual shift fields of every accepted non-reference frame over the
/// tiles that cover `region` plus `margin` sites, plus one tile of context
/// so that tiles without usable texture are filled from textured
/// neighbors.
pub fn residual_fields(
    frames: &[&dyn FrameSource],
    alignment: &Alignment,
    region: Region,
    margin: f64,
    params: &LocalParams,
) -> io::Result<(Vec<Option<ShiftField>>, LocalReport)> {
    let started = std::time::Instant::now();
    let reference = frames[alignment.reference];
    let (w, h) = (reference.width(), reference.height());
    let cfa = reference.cfa();
    let tile = params.tile.max(12);
    let margin = margin + tile as f64;
    let x0 = (region.x as f64 - margin).max(0.0).floor() as usize;
    let y0 = (region.y as f64 - margin).max(0.0).floor() as usize;
    let x1 = ((region.x + region.width) as f64 + margin)
        .min(w as f64)
        .ceil() as usize;
    let y1 = ((region.y + region.height) as f64 + margin)
        .min(h as f64)
        .ceil() as usize;
    let cols = (x1 - x0).div_ceil(tile).max(1);
    let rows = (y1 - y0).div_ceil(tile).max(1);
    let targets: Vec<(usize, Affine)> = alignment
        .frames
        .iter()
        .enumerate()
        .filter(|(k, _)| *k != alignment.reference)
        .filter_map(|(k, f)| f.to_frame.map(|t| (k, t)))
        .collect();
    let mut measured: Vec<Vec<Option<[f32; 2]>>> = vec![vec![None; cols * rows]; targets.len()];
    let pad = 4usize;
    for tj in 0..rows {
        let ty0 = y0 + tj * tile;
        let ty1 = (ty0 + tile).min(h);
        let ry0 = ty0.saturating_sub(pad);
        let ry1 = (ty1 + pad).min(h);
        let ref_band = green_band(reference, ry0, ry1 - ry0)?;
        // Reference green sites per tile of this row, away from the frame
        // edge.
        let tiles: Vec<Vec<(usize, usize)>> = (0..cols)
            .map(|ti| {
                let tx0 = x0 + ti * tile;
                let tx1 = (tx0 + tile).min(w);
                let mut sites = Vec::new();
                for y in ty0.max(4)..ty1.min(h - 4) {
                    for x in tx0.max(4)..tx1.min(w - 4) {
                        if cfa.color(x, y) == GREEN {
                            sites.push((x, y));
                        }
                    }
                }
                sites
            })
            .collect();
        let results: Vec<io::Result<Vec<Option<[f32; 2]>>>> = std::thread::scope(|scope| {
            let handles: Vec<_> = targets
                .iter()
                .map(|&(k, to_frame)| {
                    let (ref_band, tiles) = (&ref_band, &tiles);
                    scope.spawn(move || -> io::Result<Vec<Option<[f32; 2]>>> {
                        let frame = frames[k];
                        let (mut vmin, mut vmax) = (f64::MAX, f64::MIN);
                        for (x, y) in [
                            (x0 as f64, ty0 as f64),
                            (x1 as f64, ty0 as f64),
                            (x0 as f64, ty1 as f64),
                            (x1 as f64, ty1 as f64),
                        ] {
                            let (_, v) = to_frame.apply(x, y);
                            vmin = vmin.min(v);
                            vmax = vmax.max(v);
                        }
                        let m = params.max_shift.ceil() + pad as f64;
                        let fy0 = (vmin - m).floor().clamp(0.0, h as f64) as usize;
                        let fy1 = (vmax + m).ceil().clamp(0.0, h as f64) as usize;
                        if fy1 <= fy0 + 4 {
                            return Ok(vec![None; cols]);
                        }
                        let band = green_band(frame, fy0, fy1 - fy0)?;
                        Ok(tiles
                            .iter()
                            .map(|sites| {
                                refine_tile(ref_band, ry0, &band, fy0, &to_frame, sites, params)
                                    .map(|d| [d[0] as f32, d[1] as f32])
                            })
                            .collect())
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for (t, r) in results.into_iter().enumerate() {
            measured[t][tj * cols..(tj + 1) * cols].copy_from_slice(&r?);
        }
    }
    let mut fields = vec![None; frames.len()];
    let mut report = LocalReport::default();
    for (t, &(k, _)) in targets.iter().enumerate() {
        let m = &mut measured[t];
        let count = m.iter().filter(|s| s.is_some()).count();
        let outliers = clean(m, cols, rows, params.outlier);
        let shifts = fill(m, cols, rows);
        let lengths: Vec<f64> = shifts
            .iter()
            .map(|s| (s[0] as f64).hypot(s[1] as f64))
            .collect();
        report.frames.push(FrameLocalStats {
            label: alignment.frames[k].label.clone(),
            tiles: cols * rows,
            measured: count,
            outliers,
            rms_shift: (lengths.iter().map(|l| l * l).sum::<f64>() / lengths.len() as f64).sqrt(),
            max_shift: lengths.iter().copied().fold(0.0, f64::max),
        });
        fields[k] = Some(ShiftField {
            x0: x0 as f64,
            y0: y0 as f64,
            tile: tile as f64,
            cols,
            rows,
            shifts,
        });
    }
    report.seconds = started.elapsed().as_secs_f64();
    Ok((fields, report))
}
