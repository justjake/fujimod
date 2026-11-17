//! Frank Markesteijn's X-Trans demosaic (1 or 3 passes), as described by
//! the `xtrans_interpolate` routine of LibRaw (LGPL-2.1, from dcraw), written
//! afresh for linear floating-point values.
//!
//! Green is interpolated at red and blue sites along four directions from
//! the hexagon of green sites around each one, bounded by that hexagon's
//! minimum and maximum. Red and blue follow from color differences for each
//! direction (with three passes, the four directions are refined again into
//! a second set of four). Every direction's result is converted to CIELab,
//! its second derivative along its own direction measures how smooth it is,
//! and each pixel averages the directions that are smooth (homogeneous) over
//! its 5x5 neighborhood.
//!
//! Differences from LibRaw: values are white-balanced linear floats in which
//! the white level is 1.0 (LibRaw: 16-bit integers); green bounds come from
//! each site's own hexagon (LibRaw reuses one site's bounds for its vertical
//! neighbor); CIELab assumes sRGB primaries for the camera's white-balanced
//! RGB (LibRaw: the camera matrix). It works on a region plus a margin; the
//! outer 10 sites of the worked area are not valid output.

use crate::merge::Region;
use crate::plane::Plane;
use crate::source::FrameSource;
use crate::xtrans::{GREEN, XTrans};
use std::io;

/// Sites at the border of the worked area whose output is not valid.
pub const BORDER: usize = 10;

type Offset = (i64, i64);

/// Hexagon of neighbors per position modulo 3: `(dy, dx)` offsets, the
/// solitary green's position modulo 3.
struct Hexagons {
    hex: [[[Offset; 8]; 3]; 3],
    solitary: (i64, i64),
}

fn hexagons(cfa: &XTrans) -> Hexagons {
    // Orthogonal directions as rotation rows, walked around a site.
    const ORTH: [i64; 12] = [1, 0, 0, 1, -1, 0, 0, -1, 1, 0, 0, 1];
    // Neighbor pattern for a non-green and for a green site.
    const PATT: [[i64; 16]; 2] = [
        [0, 1, 0, -1, 2, 0, -1, 0, 1, 1, 1, -1, 0, 0, 0, 0],
        [0, 1, 0, -2, 1, 0, -2, 0, 1, 1, -2, -2, 1, -1, -1, 1],
    ];
    let color =
        |row: i64, col: i64| cfa.color(col.rem_euclid(6) as usize, row.rem_euclid(6) as usize);
    let mut hex = [[[(0i64, 0i64); 8]; 3]; 3];
    let mut solitary = (0, 0);
    for row in 0..3i64 {
        for col in 0..3i64 {
            let g = (color(row, col) == GREEN) as usize;
            let mut ng = 0;
            for d in (0..10).step_by(2) {
                if color(row + ORTH[d], col + ORTH[d + 2]) == GREEN {
                    ng = 0;
                } else {
                    ng += 1;
                }
                if ng == 4 {
                    solitary = (row, col);
                }
                if ng == g + 1 {
                    for c in 0..8 {
                        let (a, b) = (PATT[g][c * 2], PATT[g][c * 2 + 1]);
                        let v = ORTH[d] * a + ORTH[d + 1] * b;
                        let h = ORTH[d + 2] * a + ORTH[d + 3] * b;
                        hex[row as usize][col as usize][c ^ (g * 2 & d)] = (v, h);
                    }
                }
            }
        }
    }
    Hexagons { hex, solitary }
}

/// CIELab of white-balanced linear RGB (sRGB primaries, D65, white 1.0).
fn lab(rgb: [f32; 3]) -> [f32; 3] {
    const M: [[f32; 3]; 3] = [
        [0.4124 / 0.95047, 0.3576 / 0.95047, 0.1805 / 0.95047],
        [0.2126, 0.7152, 0.0722],
        [0.0193 / 1.08883, 0.1192 / 1.08883, 0.9505 / 1.08883],
    ];
    let f = |t: f32| {
        let t = t.clamp(0.0, 1.0);
        if t > 0.008856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    };
    let xyz = M.map(|r| f(r[0] * rgb[0] + r[1] * rgb[1] + r[2] * rgb[2]));
    [
        116.0 * xyz[1] - 16.0,
        500.0 * (xyz[0] - xyz[1]),
        200.0 * (xyz[1] - xyz[2]),
    ]
}

/// Demosaiced linear camera RGB planes (white level 1.0, not white-balanced)
/// for `region` plus `margin` sites (clamped to the frame), with the covered
/// rectangle. `margin` should be at least [`BORDER`].
pub fn demosaic(
    frame: &dyn FrameSource,
    region: Region,
    margin: usize,
    passes: usize,
) -> io::Result<(Region, [Plane; 3])> {
    let (fw, fh) = (frame.width(), frame.height());
    let x0 = region.x.saturating_sub(margin);
    let y0 = region.y.saturating_sub(margin);
    let x1 = (region.x + region.width + margin).min(fw);
    let y1 = (region.y + region.height + margin).min(fh);
    let (w, h) = (x1 - x0, y1 - y0);
    let mut raw = vec![0u16; h * fw];
    frame.read_rows(y0, &mut raw)?;
    let cfa = frame.cfa();
    let cal = frame.calibration();
    let wb = cal.wb;
    let hx = hexagons(&cfa);
    let ndir = if passes > 1 { 8 } else { 4 };
    let n = w * h;
    // Absolute positions modulo 3 and 6 drive the pattern.
    let color_at = |x: i64, y: i64| cfa.color((x0 as i64 + x) as usize, (y0 as i64 + y) as usize);
    let at = |x: i64, y: i64| -> usize {
        (y.clamp(0, h as i64 - 1) as usize) * w + x.clamp(0, w as i64 - 1) as usize
    };
    let mut v = vec![0f32; n];
    let mut col = vec![0u8; n];
    for y in 0..h {
        for x in 0..w {
            let (val, _) = cal.linear(raw[y * fw + x0 + x], x0 + x, y0 + y);
            let c = cfa.color(x0 + x, y0 + y);
            v[y * w + x] = (val * wb[c as usize]).clamp(0.0, 1.0);
            col[y * w + x] = c;
        }
    }
    let raw_ch = |p: usize, ch: usize| if col[p] as usize == ch { v[p] } else { 0.0 };
    let hex_of = |x: i64, y: i64| {
        &hx.hex[((y0 as i64 + y).rem_euclid(3)) as usize][((x0 as i64 + x).rem_euclid(3)) as usize]
    };
    let row_phase = |y: i64| (y0 as i64 + y - hx.solitary.0).rem_euclid(3);
    let col_phase = |x: i64| (x0 as i64 + x - hx.solitary.1).rem_euclid(3);
    let clip = |x: f32| x.clamp(0.0, 1.0);

    // Green bounds from each red or blue site's hexagon.
    let mut gmin = vec![0f32; n];
    let mut gmax = vec![0f32; n];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let p = at(x, y);
            if col[p] == GREEN {
                continue;
            }
            let hex = hex_of(x, y);
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for &(dy, dx) in &hex[..6] {
                let g = raw_ch(at(x + dx, y + dy), 1);
                lo = lo.min(g);
                hi = hi.max(g);
            }
            gmin[p] = lo;
            gmax[p] = hi;
        }
    }

    let mut rgb: Vec<Vec<[f32; 3]>> = vec![
        (0..n)
            .map(|p| {
                let mut px = [0f32; 3];
                px[col[p] as usize] = v[p];
                px
            })
            .collect();
        ndir
    ];

    // Green along four directions at red and blue sites.
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let p = at(x, y);
            let f = col[p] as usize;
            if f == GREEN as usize {
                continue;
            }
            let hex = hex_of(x, y);
            let g = |k: i64, o: Offset| raw_ch(at(x + k * o.1, y + k * o.0), 1);
            let r = |k: i64, o: Offset| raw_ch(at(x + k * o.1, y + k * o.0), f);
            let own = v[p];
            let mut c = [0f32; 4];
            c[0] = 174.0 * (g(1, hex[1]) + g(1, hex[0])) - 46.0 * (g(2, hex[1]) + g(2, hex[0]));
            c[1] = 223.0 * g(1, hex[3]) + 33.0 * g(1, hex[2]) + 92.0 * (own - r(-1, hex[2]));
            for k in 0..2 {
                let o = hex[4 + k];
                c[2 + k] =
                    164.0 * g(1, o) + 92.0 * g(-2, o) + 33.0 * (2.0 * own - r(3, o) - r(-3, o));
            }
            let s = (row_phase(y) == 0) as usize;
            for (k, ck) in c.iter().enumerate() {
                rgb[k ^ s][p][1] = (ck / 256.0).clamp(gmin[p], gmax[p]);
            }
        }
    }

    let mut base = 0usize;
    for pass in 0..passes {
        if pass == 1 {
            for d in 0..4 {
                rgb[4 + d] = rgb[d].clone();
            }
            base = 4;
        }
        // Green again from interpolated values of nearer sites.
        if pass > 0 {
            for y in 0..h as i64 {
                for x in 0..w as i64 {
                    let p = at(x, y);
                    let f = col[p] as usize;
                    if f == GREEN as usize {
                        continue;
                    }
                    let hex = hex_of(x, y);
                    let s = (row_phase(y) == 0) as usize;
                    for d in 3..6 {
                        let k = base + ((d - 2) ^ s);
                        let o = hex[d];
                        let a = rgb[k][at(x - 2 * o.1, y - 2 * o.0)];
                        let b = rgb[k][at(x + o.1, y + o.0)];
                        let val = a[1] + 2.0 * b[1] - a[f] - 2.0 * b[f] + 3.0 * rgb[k][p][f];
                        rgb[k][p][1] = (val / 3.0).clamp(gmin[p], gmax[p]);
                    }
                }
            }
        }

        // Red and blue at solitary green sites.
        for y in 0..h as i64 {
            if row_phase(y) != 0 {
                continue;
            }
            for x in 0..w as i64 {
                if col_phase(x) != 0 {
                    continue;
                }
                let p = at(x, y);
                let mut hc = color_at(x + 1, y) as usize;
                if hc == GREEN as usize {
                    continue;
                }
                let mut color = [[0f32; 6]; 3];
                let mut diff = [0f32; 6];
                let mut k = base;
                for d in 0..6 {
                    let (sy, sx) = if d % 2 == 0 { (0, 1) } else { (1, 0) };
                    for c in 0..2 {
                        let m = 1i64 << c;
                        let a = rgb[k][at(x + m * sx, y + m * sy)];
                        let b = rgb[k][at(x - m * sx, y - m * sy)];
                        let g = 2.0 * rgb[k][p][1] - a[1] - b[1];
                        color[hc][d] = g + a[hc] + b[hc];
                        if d > 1 {
                            diff[d] += (a[1] - b[1] - a[hc] + b[hc]).powi(2) + g * g;
                        }
                        hc ^= 2;
                    }
                    if d > 1 && d % 2 == 1 && diff[d - 1] < diff[d] {
                        color[0][d] = color[0][d - 1];
                        color[2][d] = color[2][d - 1];
                    }
                    if d < 2 || d % 2 == 1 {
                        rgb[k][p][0] = clip(color[0][d] / 2.0);
                        rgb[k][p][2] = clip(color[2][d] / 2.0);
                        k += 1;
                    }
                    hc ^= 2;
                }
            }
        }

        // Red at blue sites and blue at red sites.
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                let p = at(x, y);
                if col[p] == GREEN {
                    continue;
                }
                let f = 2 - col[p] as usize;
                let vertical = row_phase(y) != 0;
                let c_off: Offset = if vertical { (1, 0) } else { (0, 1) };
                let h_off: Offset = if vertical { (0, 3) } else { (3, 0) };
                for d in 0..4 {
                    let k = base + d;
                    let px = |o: Offset, s: i64| rgb[k][at(x + s * o.1, y + s * o.0)];
                    let g0 = rgb[k][p][1];
                    let parity = if vertical { d % 2 == 1 } else { d % 2 == 0 };
                    let use_c = d > 1
                        || parity
                        || ((g0 - px(c_off, 1)[1]).abs() + (g0 - px(c_off, -1)[1]).abs())
                            < 2.0 * ((g0 - px(h_off, 1)[1]).abs() + (g0 - px(h_off, -1)[1]).abs());
                    let i = if use_c { c_off } else { h_off };
                    let (a, b) = (px(i, 1), px(i, -1));
                    rgb[k][p][f] = clip((a[f] + b[f] + 2.0 * g0 - a[1] - b[1]) / 2.0);
                }
            }
        }

        // Red and blue in 2x2 blocks of green.
        for y in 0..h as i64 {
            if row_phase(y) == 0 {
                continue;
            }
            for x in 0..w as i64 {
                if col_phase(x) == 0 {
                    continue;
                }
                let p = at(x, y);
                let hex = hex_of(x, y);
                for dd in (0..8).step_by(2) {
                    let k = base + dd / 2;
                    let (oa, ob) = (hex[dd], hex[dd + 1]);
                    let a = rgb[k][at(x + oa.1, y + oa.0)];
                    let b = rgb[k][at(x + ob.1, y + ob.0)];
                    let g0 = rgb[k][p][1];
                    if (oa.0 + ob.0, oa.1 + ob.1) != (0, 0) {
                        let g = 3.0 * g0 - 2.0 * a[1] - b[1];
                        for c in [0, 2] {
                            rgb[k][p][c] = clip((g + 2.0 * a[c] + b[c]) / 3.0);
                        }
                    } else {
                        let g = 2.0 * g0 - a[1] - b[1];
                        for c in [0, 2] {
                            rgb[k][p][c] = clip((g + a[c] + b[c]) / 2.0);
                        }
                    }
                }
            }
        }
    }

    // Smoothness of every direction in CIELab along its own direction.
    const DIRS: [Offset; 4] = [(0, 1), (1, 0), (1, 1), (1, -1)];
    let mut drv = vec![vec![0f32; n]; ndir];
    for d in 0..ndir {
        let lab: Vec<[f32; 3]> = rgb[d].iter().map(|&p| lab(p)).collect();
        let (dy, dx) = DIRS[d & 3];
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                let l0 = lab[at(x, y)];
                let a = lab[at(x + dx, y + dy)];
                let b = lab[at(x - dx, y - dy)];
                let g = 2.0 * l0[0] - a[0] - b[0];
                drv[d][at(x, y)] = g * g
                    + (2.0 * l0[1] - a[1] - b[1] + g * 500.0 / 232.0).powi(2)
                    + (2.0 * l0[2] - a[2] - b[2] - g * 500.0 / 580.0).powi(2);
            }
        }
    }
    // Homogeneity: neighbors whose derivative is within 8x the smallest.
    let mut homo = vec![vec![0u8; n]; ndir];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let p = at(x, y);
            let tr = 8.0 * (0..ndir).map(|d| drv[d][p]).fold(f32::MAX, f32::min);
            for d in 0..ndir {
                let mut count = 0u8;
                for vy in -1..=1 {
                    for vx in -1..=1 {
                        if drv[d][at(x + vx, y + vy)] <= tr {
                            count += 1;
                        }
                    }
                }
                homo[d][p] = count;
            }
        }
    }
    // Average the most homogeneous directions over 5x5.
    let mut planes = [Plane::new(w, h), Plane::new(w, h), Plane::new(w, h)];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let p = at(x, y);
            let mut hm = [0u32; 8];
            for (d, hmd) in hm.iter_mut().enumerate().take(ndir) {
                for vy in -2..=2 {
                    for vx in -2..=2 {
                        *hmd += homo[d][at(x + vx, y + vy)] as u32;
                    }
                }
            }
            for d in 0..ndir - 4 {
                if hm[d] < hm[d + 4] {
                    hm[d] = 0;
                } else if hm[d] > hm[d + 4] {
                    hm[d + 4] = 0;
                }
            }
            let mut max = hm[..ndir].iter().copied().max().unwrap_or(0);
            max -= max >> 3;
            let mut avg = [0f32; 3];
            let mut count = 0f32;
            for d in 0..ndir {
                if hm[d] >= max {
                    for c in 0..3 {
                        avg[c] += rgb[d][p][c];
                    }
                    count += 1.0;
                }
            }
            for c in 0..3 {
                planes[c].data[p] = avg[c] / count / wb[c];
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{Calibration, MemoryFrame};

    #[test]
    fn flat_field_stays_flat() {
        // A uniform gray (after white balance) must come back uniform in
        // every channel, whatever the CFA phase of the region.
        let wb = [2.0f32, 1.0, 1.5];
        let (black, white) = (1024.0f32, 16383.0f32);
        let (w, h) = (60usize, 48usize);
        let cfa = XTrans::standard();
        let level = 0.3f32;
        let data = (0..w * h)
            .map(|i| {
                let c = cfa.color(i % w, i / w) as usize;
                (black + level / wb[c] * (white - black)).round() as u16
            })
            .collect();
        let frame = MemoryFrame {
            label: "flat".into(),
            width: w,
            height: h,
            cfa,
            calibration: Calibration::uniform(black, white, wb),
            data,
        };
        for (x, y) in [(15, 12), (16, 13), (17, 14)] {
            let region = Region {
                x,
                y,
                width: 20,
                height: 16,
            };
            let (cov, planes) = demosaic(&frame, region, BORDER + 2, 3).unwrap();
            for yy in region.y..region.y + region.height {
                for xx in region.x..region.x + region.width {
                    let i = (yy - cov.y) * cov.width + (xx - cov.x);
                    for c in 0..3 {
                        let v = planes[c].data[i] * wb[c];
                        assert!((v - level).abs() < 2e-4, "({xx}, {yy}) channel {c}: {v}");
                    }
                }
            }
        }
    }
}
