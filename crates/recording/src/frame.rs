//! Transport-independent RGB captures. PNG is computed only when requested.
use sha2::{Digest, Sha256};
use std::{
    io::Cursor,
    path::Path,
    sync::{Arc, OnceLock},
};

pub type Result<T> = std::result::Result<T, String>;

struct Pixels {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
    png: OnceLock<Vec<u8>>,
}

#[derive(Clone)]
pub struct Frame(Arc<Pixels>);

impl Frame {
    pub fn rgb(width: u32, height: u32, rgb: Vec<u8>) -> Result<Self> {
        if width == 0
            || height == 0
            || width > 8192
            || height > 8192
            || u64::from(width) * u64::from(height) * 3 != rgb.len() as u64
        {
            return Err("Invalid RGB frame dimensions or extent".into());
        }
        Ok(Self(Arc::new(Pixels {
            width,
            height,
            rgb,
            png: OnceLock::new(),
        })))
    }
    /// Straight BGRA8888, composited on black; padding on the last row is optional.
    pub fn bgra(width: u32, height: u32, stride: usize, data: &[u8]) -> Result<Self> {
        if width == 0
            || height == 0
            || width > 8192
            || height > 8192
            || !stride.is_multiple_of(4)
            || stride < width as usize * 4
            || data.len() != (height as usize - 1) * stride + width as usize * 4
        {
            return Err("Invalid BGRA frame dimensions, stride or extent".into());
        }
        let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
        for row in 0..height as usize {
            for pixel in data[row * stride..row * stride + width as usize * 4]
                .as_chunks::<4>()
                .0
            {
                for channel in [2, 1, 0] {
                    rgb.push(((u32::from(pixel[channel]) * u32::from(pixel[3]) + 127) / 255) as u8);
                }
            }
        }
        Self::rgb(width, height, rgb)
    }
    pub fn from_png(data: &[u8]) -> Result<Self> {
        let mut decoder = png::Decoder::new(Cursor::new(data));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
        let size = reader.output_buffer_size().ok_or("PNG extent overflow")?;
        if size > 8192 * 8192 * 4 {
            return Err("PNG exceeds capture limits".into());
        }
        let mut bytes = vec![0; size];
        let info = reader.next_frame(&mut bytes).map_err(|e| e.to_string())?;
        let bytes = &bytes[..info.buffer_size()];
        let rgb = match info.color_type {
            png::ColorType::Rgb => bytes.to_vec(),
            png::ColorType::Rgba => bytes
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| p[..3].iter().copied())
                .collect(),
            png::ColorType::Grayscale => bytes.iter().flat_map(|v| [*v; 3]).collect(),
            png::ColorType::GrayscaleAlpha => bytes
                .as_chunks::<2>()
                .0
                .iter()
                .flat_map(|v| [v[0]; 3])
                .collect(),
            _ => return Err("Unsupported expanded PNG color type".into()),
        };
        let frame = Self::rgb(info.width, info.height, rgb)?;
        let _ = frame.0.png.set(data.to_vec());
        Ok(frame)
    }
    pub fn width(&self) -> u32 {
        self.0.width
    }
    pub fn height(&self) -> u32 {
        self.0.height
    }
    pub fn pixels(&self) -> &[u8] {
        &self.0.rgb
    }
    pub fn same_pixels(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
            || (self.width() == other.width()
                && self.height() == other.height()
                && self.pixels() == other.pixels())
    }
    pub fn png(&self) -> Result<&[u8]> {
        if let Some(bytes) = self.0.png.get() {
            return Ok(bytes);
        }
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, self.width(), self.height());
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::Fast);
            let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
            writer
                .write_image_data(self.pixels())
                .map_err(|e| e.to_string())?;
        }
        let _ = self.0.png.set(bytes);
        Ok(self.0.png.get().unwrap())
    }
    pub fn save(&self, path: &Path) -> Result<String> {
        let bytes = self.png()?;
        std::fs::write(path, bytes).map_err(|e| e.to_string())?;
        self.sha256()
    }
    pub fn sha256(&self) -> Result<String> {
        Ok(format!("{:x}", Sha256::digest(self.png()?)))
    }
    /// Keep the first frame's video canvas; screenshots retain native dimensions.
    pub fn fit(&self, width: u32, height: u32) -> Result<Self> {
        if (self.width(), self.height()) == (width, height) {
            return Ok(self.clone());
        }
        let scale = (f64::from(width) / f64::from(self.width()))
            .min(f64::from(height) / f64::from(self.height()));
        let w = (f64::from(self.width()) * scale).round().max(1.) as u32;
        let h = (f64::from(self.height()) * scale).round().max(1.) as u32;
        let mut pixels = vec![0; width as usize * height as usize * 3];
        let (left, top) = ((width - w) / 2, (height - h) / 2);
        for y in 0..h {
            for x in 0..w {
                let source = (((u64::from(y) * u64::from(self.height()) / u64::from(h))
                    * u64::from(self.width())
                    + u64::from(x) * u64::from(self.width()) / u64::from(w))
                    * 3) as usize;
                let destination = (((y + top) * width + x + left) * 3) as usize;
                pixels[destination..destination + 3]
                    .copy_from_slice(&self.pixels()[source..source + 3]);
            }
        }
        Self::rgb(width, height, pixels)
    }
}
