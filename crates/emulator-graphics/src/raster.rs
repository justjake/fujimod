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
pub fn capture(
    read: impl Fn(u64, usize) -> Result<Vec<u8>, String>,
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
    if rgba.len() != width as usize * height as usize * 4 {
        return Err("OSD canvas extent mismatch".into());
    }
    let p = s.plane;
    let yuv = matches!(p.order, PixelOrder::YyCbCr);
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
    read: impl Fn(u64, usize) -> Result<Vec<u8>, String>,
    plane: Plane,
    width: u32,
    height: u32,
    rgba: &mut [u8],
) -> Result<(), String> {
    composite_snapshot(&capture(read, plane, width, height)?, width, height, rgba)
}
