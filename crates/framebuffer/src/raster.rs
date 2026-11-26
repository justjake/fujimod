//! Linear DMA planes, Q12 sampling and straight-alpha pixel composition.
//! A register model must explicitly select a transport and its geometry.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PixelOrder {
    Rgba,
    Bgra,
    Argb,
    Abgr,
    /// Packed 4:2:2 transport, two luma then Cb/Cr.
    YyCbCr,
}
impl PixelOrder {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "rgba" => Ok(Self::Rgba),
            "bgra" => Ok(Self::Bgra),
            "argb" => Ok(Self::Argb),
            "abgr" => Ok(Self::Abgr),
            _ => Err(format!("unsupported raw pixel order {s:?}")),
        }
    }
    fn rgba(self, b: &[u8]) -> [u8; 4] {
        match self {
            Self::Rgba => [b[0], b[1], b[2], b[3]],
            Self::Bgra => [b[2], b[1], b[0], b[3]],
            Self::Argb => [b[1], b[2], b[3], b[0]],
            Self::Abgr => [b[3], b[2], b[1], b[0]],
            Self::YyCbCr => {
                let y = (b[0] as i32 - 16).max(0) * 298;
                let cb = b[1] as i32 - 128;
                let cr = b[2] as i32 - 128;
                let c = |v: i32| ((v + 128) >> 8).clamp(0, 255) as u8;
                [
                    c(y + 409 * cr),
                    c(y - 100 * cb - 208 * cr),
                    c(y + 516 * cb),
                    255,
                ]
            }
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub address: u64,
    pub stride: u32,
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
    /// Stock Q12 sampling increments; raw diagnostic fixtures use extents.
    pub sampling: Option<[u32; 2]>,
    pub work_x: u32,
    pub work_y: u32,
    pub work_width: u32,
    pub work_height: u32,
    pub order: PixelOrder,
}
/// Rows captured at the existing scanout phase, before any host composition.
#[derive(PartialEq)]
pub struct Snapshot {
    plane: Plane,
    rows: Vec<Vec<u8>>,
}

impl Snapshot {
    /// Returns true if this snapshot covers the entire `(width, height)` canvas.
    pub fn covers(&self, width: u32, height: u32) -> bool {
        let p = self.plane;
        p.work_x == 0
            && p.work_y == 0
            && p.work_width >= width
            && p.work_height >= height
            && self.rows.len() >= height as usize
    }
}
pub fn capture(
    mut read: impl FnMut(u64, usize) -> Result<Vec<u8>, String>,
    p: Plane,
    width: u32,
    height: u32,
) -> Result<Snapshot, String> {
    if p.source_width == 0 || p.source_height == 0 || p.work_width == 0 || p.work_height == 0 {
        return Ok(Snapshot {
            plane: p,
            rows: Vec::new(),
        });
    }
    if p.source_width > 8192
        || p.source_height > 8192
        || p.work_width > 8192
        || p.work_height > 8192
    {
        return Err("OSD extent exceeds8192".into());
    }
    let yuv = matches!(p.order, PixelOrder::YyCbCr);
    let row_start = if yuv { p.source_x & !1 } else { p.source_x };
    let row_end = if yuv {
        (p.source_x + p.source_width + 1) & !1
    } else {
        p.source_x + p.source_width
    };
    let bpp = if yuv { 2 } else { 4 };
    if row_end as usize * bpp > p.stride as usize {
        return Err("OSD row exceeds programmed stride".into());
    }
    let mut rows = Vec::new();
    if p.source_width != 0
        && p.source_height != 0
        && p.work_width != 0
        && p.work_height != 0
        && width != 0
    {
        for y in p.work_y..(p.work_y + p.work_height).min(height) {
            let sy = p.source_y
                + p.sampling.map_or_else(
                    || (y - p.work_y) * p.source_height / p.work_height,
                    |q| (y - p.work_y) * q[1] / 4096,
                );
            let address = p
                .address
                .checked_add(sy as u64 * p.stride as u64)
                .and_then(|a| a.checked_add(row_start as u64 * bpp as u64))
                .ok_or("OSD DMA overflow")?;
            rows.push(read(address, (row_end - row_start) as usize * bpp)?);
        }
    }
    Ok(Snapshot { plane: p, rows })
}
/// Same nearest-neighbor pixels, alpha law and row order as live scanout.
pub fn composite_snapshot(
    s: &Snapshot,
    width: u32,
    height: u32,
    rgba: &mut [u8],
) -> Result<(), String> {
    composite_snapshot_inner::<false>(s, width, height, rgba)
}

/// Same as `composite_snapshot`, writing `[B, G, R, A]` byte order directly.
pub fn composite_snapshot_bgra(
    s: &Snapshot,
    width: u32,
    height: u32,
    bgra: &mut [u8],
) -> Result<(), String> {
    composite_snapshot_inner::<true>(s, width, height, bgra)
}

fn composite_snapshot_inner<const BGRA: bool>(
    s: &Snapshot,
    width: u32,
    height: u32,
    rgba: &mut [u8],
) -> Result<(), String> {
    if rgba.len() != width as usize * height as usize * 4 {
        return Err("OSD canvas extent mismatch".into());
    }
    let p = s.plane;
    let yuv = matches!(p.order, PixelOrder::YyCbCr);
    let shown_width = p.work_width.min(width) as usize;
    if yuv
        && p.sampling.is_none()
        && p.source_x == 0
        && p.work_x == 0
        && p.work_y == 0
        && p.source_width == p.work_width
        && shown_width != 0
        && shown_width.is_multiple_of(2)
    {
        // Packed 4:2:2 pairs share chroma and replace both pixels at full
        // opacity. Unscaled scanout needs neither coordinate nor blend math.
        let mut previous: Option<(&[u8], &[u8])> = None;
        for (row, output) in s.rows.iter().zip(rgba.chunks_exact_mut(width as usize * 4)) {
            let row = &row[..shown_width * 2];
            let output = &mut output[..shown_width * 4];
            if let Some((previous_row, pixels)) = previous
                && previous_row == row
            {
                output.copy_from_slice(pixels);
            } else {
                let mut pair_idx = 0usize;
                let pairs = row.as_chunks::<4>().0;
                let out_pairs = output.as_chunks_mut::<8>().0;
                #[cfg(target_arch = "aarch64")]
                {
                    // Process 4 YUV 4:2:2 pairs (8 pixels) per NEON iteration.
                    let n4 = pairs.len() & !3;
                    if n4 > 0 {
                        unsafe { yuv_pairs_neon::<BGRA>(&pairs[..n4], &mut out_pairs[..n4]) };
                        pair_idx = n4;
                    }
                }
                for (pair, pixels) in pairs[pair_idx..].iter().zip(out_pairs[pair_idx..].iter_mut()) {
                    let [left, right, cb, cr] = *pair;
                    let cb = cb as i32 - 128;
                    let cr = cr as i32 - 128;
                    let r_cr = 409 * cr + 128;
                    let g_cbcr = -100 * cb - 208 * cr + 128;
                    let b_cb = 516 * cb + 128;
                    let y0 = (left as i32 - 16).max(0) * 298;
                    let y1 = (right as i32 - 16).max(0) * 298;
                    let r0 = ((y0 + r_cr) >> 8).clamp(0, 255) as u8;
                    let g0 = ((y0 + g_cbcr) >> 8).clamp(0, 255) as u8;
                    let b0 = ((y0 + b_cb) >> 8).clamp(0, 255) as u8;
                    let r1 = ((y1 + r_cr) >> 8).clamp(0, 255) as u8;
                    let g1 = ((y1 + g_cbcr) >> 8).clamp(0, 255) as u8;
                    let b1 = ((y1 + b_cb) >> 8).clamp(0, 255) as u8;
                    *pixels = if BGRA {
                        [b0, g0, r0, 255, b1, g1, r1, 255]
                    } else {
                        [r0, g0, b0, 255, r1, g1, b1, 255]
                    };
                }
            }
            previous = Some((row, output));
        }
        return Ok(());
    }
    #[inline(always)]
    fn blend_pixel<const BGRA: bool>(out: &mut [u8], mut c: [u8; 4]) {
        if BGRA {
            c.swap(0, 2);
        }
        let a = c[3] as u32;
        if a == 0 {
            out[3] = 255;
        } else if a == 255 {
            out[0] = c[0];
            out[1] = c[1];
            out[2] = c[2];
            out[3] = 255;
        } else {
            let inv = 255 - a;
            for n in 0..3 {
                let x = c[n] as u32 * a + out[n] as u32 * inv + 128;
                out[n] = ((x + (x >> 8)) >> 8) as u8;
            }
            out[3] = 255;
        }
    }
    let x_start = p.work_x;
    let x_end = (p.work_x + p.work_width).min(width);
    let stride = width as usize * 4;
    if !yuv && p.sampling.is_none() && p.source_width == p.work_width {
        let count = (x_end.saturating_sub(x_start)) as usize;
        for (n, row) in s.rows.iter().enumerate() {
            let y = (p.work_y + n as u32) as usize;
            let row_out = &mut rgba[y * stride + x_start as usize * 4..][..count * 4];
            for (src, out) in row[..count * 4]
                .chunks_exact(4)
                .zip(row_out.chunks_exact_mut(4))
            {
                let c = p.order.rgba(src);
                blend_pixel::<BGRA>(out, c);
            }
        }
        return Ok(());
    }
    let row_start = if yuv { p.source_x & !1 } else { p.source_x };
    for (n, row) in s.rows.iter().enumerate() {
        let y = (p.work_y + n as u32) as usize;
        let row_out = &mut rgba[y * stride..][..stride];
        for x in x_start..x_end {
            let sx = p.sampling.map_or_else(
                || (x - x_start) * p.source_width / p.work_width,
                |q| (x - x_start) * q[0] / 4096,
            );
            let c = if yuv {
                let pixel = sx + p.source_x - row_start;
                let i = (pixel & !1) as usize * 2;
                p.order
                    .rgba(&[row[i + (pixel & 1) as usize], row[i + 2], row[i + 3], 0])
            } else {
                p.order.rgba(&row[sx as usize * 4..sx as usize * 4 + 4])
            };
            blend_pixel::<BGRA>(&mut row_out[x as usize * 4..][..4], c);
        }
    }
    Ok(())
}
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn yuv_pairs_neon<const BGRA: bool>(pairs: &[[u8; 4]], out: &mut [[u8; 8]]) {
    use std::arch::aarch64::*;
    let v16 = vdup_n_u8(16);
    let v128 = vdupq_n_s16(128);
    let c298 = vdup_n_s16(298);
    let c409 = vdup_n_s16(409);
    let c_100 = vdup_n_s16(-100);
    let c_208 = vdup_n_s16(-208);
    let c516 = vdup_n_s16(516);
    let round128 = vdupq_n_s32(128);
    let alpha = vdup_n_u8(255);

    for (chunk, dst) in pairs.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
        let p = unsafe { vld4_u8(chunk.as_ptr().cast()) };
        let y0_u = vqsub_u8(p.0, v16);
        let y1_u = vqsub_u8(p.1, v16);
        let y0_s = vreinterpret_s16_u16(vget_low_u16(vmovl_u8(y0_u)));
        let y1_s = vreinterpret_s16_u16(vget_low_u16(vmovl_u8(y1_u)));
        let y0 = vmull_s16(y0_s, c298);
        let y1 = vmull_s16(y1_s, c298);

        let cb = vget_low_s16(vsubq_s16(vreinterpretq_s16_u16(vmovl_u8(p.2)), v128));
        let cr = vget_low_s16(vsubq_s16(vreinterpretq_s16_u16(vmovl_u8(p.3)), v128));

        let r_cr = vaddq_s32(vmull_s16(cr, c409), round128);
        let g_cbcr = vaddq_s32(vmlal_s16(vmull_s16(cb, c_100), cr, c_208), round128);
        let b_cb = vaddq_s32(vmull_s16(cb, c516), round128);

        let clamp_u8 = |lo: int32x4_t, hi: int32x4_t| -> uint8x8_t {
            let s_lo = vqmovn_s32(vshrq_n_s32::<8>(lo));
            let s_hi = vqmovn_s32(vshrq_n_s32::<8>(hi));
            vqmovun_s16(vcombine_s16(s_lo, s_hi))
        };

        let r_evn = clamp_u8(vaddq_s32(y0, r_cr), vaddq_s32(y1, r_cr));
        let g_evn = clamp_u8(vaddq_s32(y0, g_cbcr), vaddq_s32(y1, g_cbcr));
        let b_evn = clamp_u8(vaddq_s32(y0, b_cb), vaddq_s32(y1, b_cb));

        let r_inter = vzip_u8(r_evn, vext_u8::<4>(r_evn, r_evn)).0;
        let g_inter = vzip_u8(g_evn, vext_u8::<4>(g_evn, g_evn)).0;
        let b_inter = vzip_u8(b_evn, vext_u8::<4>(b_evn, b_evn)).0;

        let out_vec = if BGRA {
            uint8x8x4_t(b_inter, g_inter, r_inter, alpha)
        } else {
            uint8x8x4_t(r_inter, g_inter, b_inter, alpha)
        };
        unsafe { vst4_u8(dst.as_mut_ptr().cast(), out_vec) };
    }
}

pub fn composite(
    read: impl FnMut(u64, usize) -> Result<Vec<u8>, String>,
    plane: Plane,
    width: u32,
    height: u32,
    rgba: &mut [u8],
) -> Result<(), String> {
    composite_snapshot(&capture(read, plane, width, height)?, width, height, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unscaled_opaque_yuv_matches_identity_sampling_with_clipping() {
        let rows = [
            [8, 235, 190, 64, 82, 120, 15, 218],
            [15, 16, 30, 211, 39, 70, 221, 29],
            [15, 16, 30, 211, 39, 70, 221, 29],
        ];
        let unscaled = Plane {
            address: 0,
            stride: 8,
            source_x: 0,
            source_y: 0,
            source_width: 4,
            source_height: 3,
            sampling: None,
            work_x: 0,
            work_y: 0,
            work_width: 4,
            work_height: 3,
            order: PixelOrder::YyCbCr,
        };
        let read = |at: u64, bytes: usize| Ok(rows[at as usize / 8][..bytes].to_vec());
        for (width, height) in [(2, 2), (3, 2), (4, 3), (6, 4)] {
            let mut fast = vec![64; (width * height * 4) as usize];
            let mut reference = fast.clone();
            composite(read, unscaled, width, height, &mut fast).unwrap();
            let sampled = Plane {
                sampling: Some([4096, 4096]),
                ..unscaled
            };
            composite(read, sampled, width, height, &mut reference).unwrap();
            assert_eq!(fast, reference);
        }
    }
}
