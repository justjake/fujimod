//! Luma planes from camera JPEGs, for registering bursts recorded without
//! RAW. The plane is linearized and box-averaged 3x3, matching the scale of
//! the RAF block planes.

use crate::plane::Plane;
use std::io;
use std::path::Path;

fn linear(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Decodes a JPEG's luma and averages it over 3x3 pixel blocks.
pub fn block_luma(path: impl AsRef<Path>) -> io::Result<Plane> {
    let bytes = std::fs::read(path)?;
    let options = zune_core::options::DecoderOptions::new_safe()
        .set_max_width(20_000)
        .set_max_height(20_000)
        .jpeg_set_out_colorspace(zune_core::colorspace::ColorSpace::Luma);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(
        zune_core::bytestream::ZCursor::new(&bytes),
        options,
    );
    let samples = decoder
        .decode()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("JPEG decode: {e:?}")))?;
    let (w, h) = decoder
        .dimensions()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "JPEG has no dimensions"))?;
    if samples.len() != w * h {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "JPEG luma has an unexpected size",
        ));
    }
    let lut: Vec<f32> = (0..=255u8).map(linear).collect();
    let (bw, bh) = (w / 3, h / 3);
    let mut out = Plane::new(bw, bh);
    for j in 0..bh {
        for i in 0..bw {
            let mut s = 0f32;
            for dy in 0..3 {
                let row = &samples[(3 * j + dy) * w + 3 * i..][..3];
                s += lut[row[0] as usize] + lut[row[1] as usize] + lut[row[2] as usize];
            }
            out.data[j * bw + i] = s / 9.0;
        }
    }
    Ok(out)
}
