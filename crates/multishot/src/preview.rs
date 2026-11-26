//! sRGB PNG previews of linear camera RGB.

use crate::merge::StripSink;
use std::fs::File;
use std::io::{self, BufWriter};
use std::path::Path;

fn srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let e = if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (e * 255.0 + 0.5) as u8
}

/// Gain that maps the 99.5th percentile of white-balanced green to 0.9.
pub fn auto_gain(rgb: &[f32], wb: [f32; 3]) -> f32 {
    let mut g: Vec<f32> = rgb
        .as_chunks::<3>()
        .0
        .iter()
        .step_by(7)
        .map(|p| p[1] * wb[1])
        .collect();
    if g.is_empty() {
        return 1.0;
    }
    let k = (g.len() as f32 * 0.995) as usize;
    let k = k.min(g.len() - 1);
    g.select_nth_unstable_by(k, f32::total_cmp);
    0.9 / g[k].max(1e-6)
}

/// Writes white-balanced, gained, sRGB-encoded 8-bit PNG.
pub fn write_png(
    path: impl AsRef<Path>,
    rgb: &[f32],
    width: usize,
    height: usize,
    wb: [f32; 3],
    gain: f32,
) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(width * height * 3);
    for p in rgb.as_chunks::<3>().0.iter() {
        for c in 0..3 {
            bytes.push(srgb(p[c] * wb[c] * gain));
        }
    }
    let file = BufWriter::new(File::create(path)?);
    let mut enc = png::Encoder::new(file, width as u32, height as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let mut w = enc.write_header().map_err(io::Error::other)?;
    w.write_image_data(&bytes).map_err(io::Error::other)?;
    Ok(())
}

/// Box-downscales the merged image by an integer factor as strips arrive.
pub struct PreviewSink {
    pub factor: usize,
    pub src_width: usize,
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<f32>,
    counts: Vec<u32>,
}

impl PreviewSink {
    pub fn new(src_width: usize, src_height: usize, factor: usize) -> Self {
        let factor = factor.max(1);
        let (width, height) = (src_width.div_ceil(factor), src_height.div_ceil(factor));
        Self {
            factor,
            src_width,
            width,
            height,
            rgb: vec![0.0; width * height * 3],
            counts: vec![0; width * height],
        }
    }

    pub fn finish(mut self, path: impl AsRef<Path>, wb: [f32; 3]) -> io::Result<()> {
        for (k, &n) in self.counts.iter().enumerate() {
            if n > 0 {
                for c in 0..3 {
                    self.rgb[k * 3 + c] /= n as f32;
                }
            }
        }
        let gain = auto_gain(&self.rgb, wb);
        write_png(path, &self.rgb, self.width, self.height, wb, gain)
    }
}

impl StripSink for PreviewSink {
    fn write_strip(&mut self, y0: usize, rows: usize, rgb: &[f32]) -> io::Result<()> {
        for r in 0..rows {
            let py = (y0 + r) / self.factor;
            for x in 0..self.src_width {
                let k = py * self.width + x / self.factor;
                let s = (r * self.src_width + x) * 3;
                for c in 0..3 {
                    self.rgb[k * 3 + c] += rgb[s + c];
                }
                self.counts[k] += 1;
            }
        }
        Ok(())
    }
}
