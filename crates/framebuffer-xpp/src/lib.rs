//! X-Processor Pro framebuffer composition from a read-only register snapshot.
use fujimod_framebuffer::raster::{self, PixelOrder, Plane};
use std::collections::BTreeMap;
pub const CONTROL: u64 = 0x000;
pub const VIDEO_ENABLE: u32 = 1 << 2;
pub const VIDEO_FORMAT: u64 = 0x010;
pub const PACKED_422: u32 = 0x333;
pub const OUTPUT_SIZE: u64 = 0x014;
pub const VIDEO_SIZE: u64 = 0x02c;
pub const VIDEO_BUFFER: u64 = 0x030;
pub const LAYER_ENABLE: u64 = 0x200;
pub const LAYER_ORIGIN: u64 = 0x210;
pub const LAYER_SIZE: u64 = 0x220;
pub const LAYER_BUFFER: u64 = 0x230;

pub const BASES: [u64; 2] = [0xfff59000, 0xfff64000];
pub fn register_ranges(output: usize) -> Result<Vec<(u64, usize)>, String> {
    let b = *BASES.get(output).ok_or("Unknown XPP output")?;
    Ok(vec![(b, 0x34), (b + 0x200, 0x40)])
}
pub const REGISTER_BYTES: usize = 0x240;
pub struct Decoder<'a> {
    pub words: &'a BTreeMap<u64, u32>,
}
impl Decoder<'_> {
    fn word(&self, o: u64) -> u32 {
        self.words.get(&o).copied().unwrap_or(0)
    }
    /// `(width, height)` of a size word, or `None` for unsupported bits.
    fn extent(word: u32) -> Option<(u32, u32)> {
        (word & 0xf000_f000 == 0).then_some(((word >> 16) + 1, (word & 0xfff) + 1))
    }
    /// Composite the video layer and the enabled OSD layers; `Ok(None)` when
    /// no layer is enabled.
    pub fn compose(
        &self,
        mut read: impl FnMut(u64, usize) -> Result<Vec<u8>, String>,
    ) -> Result<Option<(u32, u32, Vec<u8>, u64)>, ()> {
        let enable = self.word(LAYER_ENABLE);
        let video = self.word(CONTROL) & VIDEO_ENABLE != 0;
        if enable & !0xf != 0 {
            return Err(());
        }
        if enable == 0 && !video {
            return Ok(None);
        }
        let (width, height) = Self::extent(self.word(OUTPUT_SIZE)).ok_or(())?;
        let mut rgba = vec![0; width as usize * height as usize * 4];
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        let mut top = 0;
        if video {
            let (w, h) = Self::extent(self.word(VIDEO_SIZE)).ok_or(())?;
            let buffer = u64::from(self.word(VIDEO_BUFFER));
            if self.word(VIDEO_FORMAT) != PACKED_422
                || w > width
                || h > height
                || w & 1 != 0
                || buffer & 3 != 0
            {
                return Err(());
            }
            let plane = Plane {
                address: buffer,
                stride: w * 2,
                source_x: 0,
                source_y: 0,
                source_width: w,
                source_height: h,
                sampling: None,
                work_x: 0,
                work_y: 0,
                work_width: w,
                work_height: h,
                order: PixelOrder::YyCbCr,
            };
            raster::composite(|a, n| read(a, n), plane, width, height, &mut rgba)
                .map_err(|_| ())?;
            for pixel in rgba.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            top = buffer;
        }
        for layer in (0..4).filter(|n| enable & 1 << n != 0) {
            let at = |offset: u64| offset + 4 * layer as u64;
            let (w, h) = Self::extent(self.word(at(LAYER_SIZE))).ok_or(())?;
            let buffer = u64::from(self.word(at(LAYER_BUFFER)));
            if self.word(at(LAYER_ORIGIN)) != 0 || w > width || h > height || buffer & 3 != 0 {
                return Err(());
            }
            let pixels = read(buffer, w as usize * h as usize * 4).map_err(|_| ())?;
            for (y, row) in pixels.chunks_exact(w as usize * 4).enumerate() {
                let line = &mut rgba[y * width as usize * 4..][..w as usize * 4];
                for (out, src) in line.chunks_exact_mut(4).zip(row.chunks_exact(4)) {
                    let keep = 255 - u32::from(src[3]);
                    for n in 0..3 {
                        let value = u32::from(src[n]) + (u32::from(out[n]) * keep + 127) / 255;
                        out[n] = value.min(255) as u8;
                    }
                }
            }
            top = buffer;
        }
        Ok(Some((width, height, rgba, top)))
    }
}
