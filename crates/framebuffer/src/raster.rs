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
    /// 8-bit 4:2:0 luma with an interleaved Cb/Cr plane of the same byte stride.
    Nv12 {
        chroma_address: u64,
    },
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
            Self::YyCbCr | Self::Nv12 { .. } => {
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
    let yuv = matches!(p.order, PixelOrder::YyCbCr | PixelOrder::Nv12 { .. });
    let row_start = if yuv { p.source_x & !1 } else { p.source_x };
    let row_end = if yuv {
        (p.source_x + p.source_width + 1) & !1
    } else {
        p.source_x + p.source_width
    };
    let bpp = match p.order {
        PixelOrder::Nv12 { .. } => 1,
        PixelOrder::YyCbCr => 2,
        _ => 4,
    };
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
            let columns = (row_end - row_start) as usize;
            let row = read(address, columns * bpp)?;
            if let PixelOrder::Nv12 { chroma_address } = p.order {
                let address = chroma_address
                    .checked_add(u64::from(sy / 2) * u64::from(p.stride))
                    .and_then(|a| a.checked_add(u64::from(row_start)))
                    .ok_or("chroma DMA overflow")?;
                let chroma = read(address, columns)?;
                if row.len() != columns || chroma.len() != columns {
                    return Err("short NV12 DMA row".into());
                }
                // The compositor consumes a packed 4:2:2 row. Every pair in
                // this row uses the chroma pair from its 2x2 source block.
                let mut packed = Vec::with_capacity(columns * 2);
                for (y, c) in row.as_chunks::<2>().0.iter().zip(chroma.as_chunks::<2>().0) {
                    packed.extend_from_slice(&[y[0], y[1], c[0], c[1]]);
                }
                rows.push(packed);
            } else {
                rows.push(row);
            }
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
    if rgba.len() != width as usize * height as usize * 4 {
        return Err("OSD canvas extent mismatch".into());
    }
    let p = s.plane;
    let yuv = matches!(p.order, PixelOrder::YyCbCr | PixelOrder::Nv12 { .. });
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
                for (pair, pixels) in row
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(output.as_chunks_mut::<8>().0)
                {
                    let [left, right, cb, cr] = *pair;
                    pixels[..4].copy_from_slice(&p.order.rgba(&[left, cb, cr, 0]));
                    pixels[4..].copy_from_slice(&p.order.rgba(&[right, cb, cr, 0]));
                }
            }
            previous = Some((row, output));
        }
        return Ok(());
    }
    let row_start = if yuv { p.source_x & !1 } else { p.source_x };
    for (n, row) in s.rows.iter().enumerate() {
        let y = p.work_y + n as u32;
        for x in p.work_x..(p.work_x + p.work_width).min(width) {
            let sx = p.sampling.map_or_else(
                || (x - p.work_x) * p.source_width / p.work_width,
                |q| (x - p.work_x) * q[0] / 4096,
            );
            let c = if yuv {
                let pixel = sx + p.source_x - row_start;
                let i = (pixel & !1) as usize * 2;
                p.order
                    .rgba(&[row[i + (pixel & 1) as usize], row[i + 2], row[i + 3], 0])
            } else {
                p.order.rgba(&row[sx as usize * 4..sx as usize * 4 + 4])
            };
            let out = &mut rgba[(y as usize * width as usize + x as usize) * 4..][..4];
            let a = c[3] as u32;
            for n in 0..3 {
                out[n] = ((c[n] as u32 * a + out[n] as u32 * (255 - a) + 127) / 255) as u8;
            }
            out[3] = 255;
        }
    }
    Ok(())
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
