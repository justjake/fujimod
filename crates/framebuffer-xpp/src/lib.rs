//! X-Processor Pro framebuffer composition from a read-only register snapshot.
use fujimod_framebuffer::raster::{self, PixelOrder, Plane};
use std::collections::BTreeMap;
pub const CONTROL: u64 = 0x000;
/// +0x000 bit 0 enables the output and its video plane, bit 2 the OSD layer group.
pub const VIDEO_ENABLE: u32 = 1 << 0;
pub const OSD_ENABLE: u32 = 1 << 2;
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
    pub fn extent(word: u32) -> Option<(u32, u32)> {
        (word & 0xf000_f000 == 0).then_some(((word >> 16) + 1, (word & 0xfff) + 1))
    }
    /// Whether the video plane is shown. A plane whose words are all zero
    /// (format, size and buffer) is not yet programmed: the firmware enables
    /// the output (0x1010101) before REC sets it up.
    pub fn video_shown(&self) -> bool {
        self.word(CONTROL) & VIDEO_ENABLE != 0
            && (self.word(VIDEO_FORMAT) != 0
                || self.word(VIDEO_SIZE) != 0
                || self.word(VIDEO_BUFFER) != 0)
    }
    /// The enabled OSD layers, or none while the OSD group is off.
    pub fn layers_shown(&self) -> u32 {
        if self.word(CONTROL) & OSD_ENABLE != 0 {
            self.word(LAYER_ENABLE)
        } else {
            0
        }
    }
    /// Composite the video layer and the enabled OSD layers; `Ok(None)` when
    /// no layer is enabled.
    ///
    /// The output size (+0x014) is the active area the panel reads each frame.
    /// A plane larger than it shows its top-left part: rows and columns past the
    /// active area are not fetched. The X-T20 2.11 VIEW MODE switch from the EVF
    /// back to the rear screen produces this: the firmware writes the rear
    /// screen's timing (720 x 480) and re-enables the OSD group in one step,
    /// while the plane sizes still hold the EVF's 1024 x 768 (OSD) and
    /// 1024 x 682 (video) until its next commit, and a frame is scanned in
    /// between. The law is inferred from that sequence, not measured on the
    /// panel.
    // Err(()) only says the layer setup is unsupported; callers fall back without a reason.
    #[allow(clippy::result_unit_err)]
    pub fn compose(
        &self,
        mut read: impl FnMut(u64, usize) -> Result<Vec<u8>, String>,
    ) -> Result<Option<(u32, u32, Vec<u8>, u64)>, ()> {
        if self.word(LAYER_ENABLE) & !0xf != 0 {
            return Err(());
        }
        let video = self.video_shown();
        let enable = self.layers_shown();
        if enable == 0 && !video {
            return Ok(None);
        }
        let (width, height) = Self::extent(self.word(OUTPUT_SIZE)).ok_or(())?;
        let mut rgba = vec![0; width as usize * height as usize * 4];
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
        let mut top = 0;
        if video {
            let (w, h) = Self::extent(self.word(VIDEO_SIZE)).ok_or(())?;
            let buffer = u64::from(self.word(VIDEO_BUFFER));
            // The video path can be on with no plane committed: the X-T20's
            // update page switches bit 2 of +0x000 on while +0x02c and +0x030
            // both stay zero, and the layers then compose over black. A plane
            // with only one of them set is still refused.
            // A plane larger than the output is clipped to the output's active
            // area (see `compose`).
            let committed = self.word(VIDEO_SIZE) != 0 || buffer != 0;
            if committed && (self.word(VIDEO_FORMAT) != PACKED_422 || w & 1 != 0 || buffer & 3 != 0)
            {
                return Err(());
            }
            if committed {
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
                raster::composite(&mut read, plane, width, height, &mut rgba).map_err(|_| ())?;
                for pixel in rgba.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                }
                top = buffer;
            }
        }
        for layer in (0..4).filter(|n| enable & 1 << n != 0) {
            let at = |offset: u64| offset + 4 * layer as u64;
            let (w, h) = Self::extent(self.word(at(LAYER_SIZE))).ok_or(())?;
            let buffer = u64::from(self.word(at(LAYER_BUFFER)));
            if self.word(at(LAYER_ORIGIN)) != 0 || buffer & 3 != 0 {
                return Err(());
            }
            let (shown_w, shown_h) = (w.min(width) as usize, h.min(height) as usize);
            let pixels = read(buffer, w as usize * shown_h * 4).map_err(|_| ())?;
            for (y, row) in pixels.chunks_exact(w as usize * 4).enumerate() {
                let row = &row[..shown_w * 4];
                let line = &mut rgba[y * width as usize * 4..][..shown_w * 4];
                for (out, src) in line
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(row.as_chunks::<4>().0)
                {
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
