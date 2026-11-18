//! XP5 display DMA geometry. Emulator encoding is explicit and never inferred on hardware.
pub mod emulator_codec;
use fujimod_framebuffer::raster::{PixelOrder, Plane};
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Encoding {
    Unresolved,
    Linear(PixelOrder),
    EmulatorConverted,
}
pub const BASES: [u64; 2] = [0xff990000, 0xff9a0000];
pub const REGISTER_BYTES: usize = 0x490;

/// Address, packing and phase words of one picture group, as the stock
/// commit (4.31: 0x1430824 builds the bank1-layout image, 0x1433724 and
/// 0x1434c6c copy it; image +10/+14 swap into registers +14/+10) programs
/// them from a layer set through 0x141c6e0 (addresses), 0x141c5c0 (work
/// rectangle) and 0x141c854 (descriptor).
#[derive(Clone, Copy, Debug, PartialEq)]
struct PictureWords {
    /// +00 bits 0..2 and +04: first plane (luma or packed pixels).
    address_high: u32,
    address: u64,
    /// +08 bits 0..2 and +0c: second plane (chroma for split layouts).
    chroma_high: u32,
    chroma: u64,
    /// +10: bits 0..1 descriptor format (+0x20 of the 0x141c854 input),
    /// bit 8 the 3-samples-per-32-bit packing flag (+0x1c), bits 16..17
    /// the descriptor word +0x14.
    control: u32,
    /// +14 bits 0..19, bytes per line.
    stride_word: u32,
    stride: u32,
    /// +28 crop origin (half pixels) and +2c start phase.
    crop: u32,
    phase: u32,
}
/// Bit 8 of the control word: 10-bit samples packed three per 32-bit word,
/// pitch (width / 6) * 8 (0x14312d8), luma then chroma plane.
const PICTURE_PACKED_10BIT: u32 = 1 << 8;
impl PictureWords {
    fn read(r: impl Fn(u64) -> u32) -> Self {
        let address_high = r(0);
        let chroma_high = r(8);
        let stride_word = r(0x14);
        Self {
            address_high,
            address: ((address_high & 7) as u64) << 32 | r(4) as u64,
            chroma_high,
            chroma: ((chroma_high & 7) as u64) << 32 | r(0xc) as u64,
            control: r(0x10),
            stride_word,
            stride: stride_word & 0xfffff,
            crop: r(0x28),
            phase: r(0x2c),
        }
    }
    /// Only 8-bit packed 4:2:2 (control 0, pitch = 2 * width from
    /// 0x14312a0) is scanned out. Split-plane and 10-bit layouts stay
    /// refused until their sample order is decoded.
    fn unsupported(&self) -> Option<&'static str> {
        if self.address_high & !7 != 0 || self.chroma_high & !7 != 0 {
            return Some("address-high control bits");
        }
        if self.stride_word & !0xfffff != 0 {
            return Some("stride control bits");
        }
        if self.control & PICTURE_PACKED_10BIT != 0 {
            return Some("10-bit three-per-word packing (split luma/chroma planes)");
        }
        if self.control & 3 != 0 {
            return Some("descriptor format other than packed 4:2:2");
        }
        if self.control != 0 {
            return Some("undecoded control bits");
        }
        // Packed 4:2:2 reads one plane. Playback (playcont 0x56e5d20 ->
        // 0x56e5d84) programs the packed buffer as both planes; live view
        // programs zero. Any other second plane is not a packed layer.
        if self.chroma != 0 && self.chroma != self.address {
            return Some("second plane differs from the packed 4:2:2 plane");
        }
        None
    }
}
impl std::fmt::Display for PictureWords {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "+00={:#x} +04={:#x} +08={:#x} +0c={:#x} +10(control)={:#x} \
             +14(stride)={:#x} +28(crop)={:#x} +2c(phase)={:#x}",
            self.address_high,
            self.address as u32,
            self.chroma_high,
            self.chroma as u32,
            self.control,
            self.stride_word,
            self.crop,
            self.phase,
        )
    }
}
pub struct Decoder<'a> {
    pub bank: usize,
    pub regs: &'a [u32],
    pub formats: [Encoding; 4],
}
impl Decoder<'_> {
    fn r(&self, o: u64) -> u32 {
        self.regs[o as usize / 4]
    }
    pub fn planes(
        &self,
        mut read: impl FnMut(u64, usize) -> Result<Vec<u8>, String>,
    ) -> Result<(Vec<Plane>, Vec<String>), String> {
        if self.bank > 1 || self.regs.len() < REGISTER_BYTES / 4 {
            return Err("XP5 display register snapshot is truncated".into());
        }
        let enables = self.r(8);
        // Bit24 through context+95b is output control,
        // separate from the six plane-enable bits below.
        if enables & !0x01111111 != 0 {
            return Err("unresolved display plane-enable bits".into());
        }
        // Commit packs OSD0/1 into bits20/16; picture groups use12/8/4/0.
        let mut planes = Vec::new();
        let mut messages = Vec::new();
        // The output publishes four picture groups from the three source
        // descriptors (some output modes split a source into two groups).
        // Consume packed422 payload only through the programmed DMA address.
        for (n, a) in if self.bank == 0 {
            [0x370, 0x3c0, 0x410, 0x460]
        } else {
            [0x370, 0x3b8, 0x3fc, 0x440]
        }
        .into_iter()
        .enumerate()
        {
            if enables & (1 << [12, 8, 4, 0][n]) == 0 {
                continue;
            }
            let words = PictureWords::read(|o| self.r(a + o));
            if let Some(reason) = words.unsupported() {
                return Err(format!(
                    "unsupported picture address/packing/fractional phase: bank{} group{n} \
                     (registers {:#x}+{a:#x}): {reason}; {words}",
                    self.bank, BASES[self.bank],
                ));
            }
            for (offset, mask) in [
                (0x18, 0x3fff),
                (0x1c, 0x1fff),
                (0x20, 0x1fff),
                (0x24, 0x7fff),
                (0x28, 0x7fff),
                (0x2c, 0x7fff),
            ] {
                if self.r(a + offset) & !(mask | mask << 16) != 0 {
                    return Err(format!(
                        "unresolved picture geometry control bits: bank{} group{n} \
                         +{offset:#x}={:#x}; {words}",
                        self.bank,
                        self.r(a + offset),
                    ));
                }
            }
            let xy = |v: u32, m: u32| (v & m, (v >> 16) & m);
            let (source_width, source_height) = xy(self.r(a + 0x18), 0x3fff);
            let (wx, wy) = xy(self.r(a + 0x1c), 0x1fff);
            let (ww, wh) = xy(self.r(a + 0x20), 0x1fff);
            let (qx, qy) = xy(self.r(a + 0x24), 0x7fff);
            if ww == 0 || wh == 0 {
                continue;
            }
            // Picture groups use the same dimensions/work/Q12 layout as OSD.
            // +28 crop origins are in half-pixel units (0x1421384 doubles
            // the clipped pixel offset). +2c start phases are written as
            // zero by the stock commit (0x14213a0); a non-zero phase is
            // treated as a Q12 start offset whose integer part moves the
            // source origin. Its fraction is dropped by nearest sampling.
            let (crop_x, crop_y) = xy(self.r(a + 0x28), 0x7fff);
            let (phase_x, phase_y) = xy(self.r(a + 0x2c), 0x7fff);
            if qx == 0 || qy == 0 || (crop_x | crop_y) & 1 != 0 {
                return Err(format!(
                    "picture sampling/crop origin invalid: bank{} group{n} ratio={:#x} crop={:#x}",
                    self.bank,
                    self.r(a + 0x24),
                    self.r(a + 0x28),
                ));
            }
            let (sx, sy) = (crop_x / 2 + phase_x / 4096, crop_y / 2 + phase_y / 4096);
            let sw = ((ww - 1) * qx / 4096) + 1;
            let sh = ((wh - 1) * qy / 4096) + 1;
            if sx + sw > source_width || sy + sh > source_height {
                return Err(format!(
                    "picture source rectangle exceeds programmed dimensions: bank{} group{n} \
                     source {sx},{sy}+{sw}x{sh} > {source_width}x{source_height}",
                    self.bank,
                ));
            }
            planes.push(Plane {
                address: words.address,
                stride: words.stride,
                source_x: sx,
                source_y: sy,
                source_width: sw,
                source_height: sh,
                sampling: Some([qx, qy]),
                work_x: wx,
                work_y: wy,
                work_width: ww,
                work_height: wh,
                order: PixelOrder::YyCbCr,
            });
        }
        for n in 0..2 {
            if enables & (1 << [20, 16][n]) == 0 {
                continue;
            }
            let a = if n == 0 {
                if self.bank == 0 { 0x50 } else { 0x58 }
            } else if self.bank == 0 {
                0x1e0
            } else {
                0x1e4
            };
            let ctrl = self.r(a + 0x10);
            let format = ((ctrl >> 16) & 3) as usize;
            let encoding = self.formats[format];
            if encoding == Encoding::Unresolved {
                return Err("XP5 native converted plane encoding is unresolved".into());
            }
            let raw_order = match encoding {
                Encoding::Linear(p) => Some(p),
                _ => None,
            };
            let allowed = if raw_order.is_some() {
                0x30000
            } else {
                0x30101
            };
            if ctrl & !allowed != 0 {
                return Err(format!("OSD control bits unresolved: {ctrl:#x}"));
            }
            if self.r(a) > 7 || self.r(a + 8) > 7 || self.r(a + 0x14) > 0xfffff {
                return Err("OSD address-high/stride control bits unresolved".into());
            }
            for (offset, mask) in [
                (0x18, 0x3fff),
                (0x1c, 0x1fff),
                (0x20, 0x1fff),
                (0x24, 0x7fff),
            ] {
                if self.r(a + offset) & !(mask | mask << 16) != 0 {
                    return Err(format!(
                        "OSD geometry control bits unresolved at {:#x}",
                        a + offset
                    ));
                }
            }
            let xy = |v: u32, mask: u32| (v & mask, (v >> 16) & mask);
            let (mut sx, mut sy) = xy(self.r(a + 0x18), 0x3fff);
            let (wx, wy) = xy(self.r(a + 0x1c), 0x1fff);
            let (ww, wh) = xy(self.r(a + 0x20), 0x1fff);
            let (mut sw, mut sh) = xy(self.r(a + 0x24), 0x7fff);
            if ww == 0 || wh == 0 {
                continue;
            }
            let address = ((self.r(a) & 7) as u64) << 32 | self.r(a + 4) as u64;
            let mut sampling = None;
            let (order, stride) = if let Some(order) = raw_order {
                (order, self.r(a + 0x14) & 0xfffff)
            } else {
                if !matches!(format, 0 | 3) {
                    return Err(format!(
                        "unsupported OSD private transport control {ctrl:#x}"
                    ));
                }
                let secondary = ((self.r(a + 8) & 7) as u64) << 32 | self.r(a + 12) as u64;
                let header = read(secondary, emulator_codec::HEADER_BYTES)?;
                // Newly allocated planes can be enabled before the first
                // converter completion. They have no published image yet.
                if header.iter().all(|b| *b == 0) {
                    continue;
                }
                if ctrl & 1 == 0 {
                    return Err("OSD private plane requires converter control bit0".into());
                }
                let publication = emulator_codec::decode(&header)?;
                if publication.primary != address {
                    // Register writes may select the new primary before its
                    // secondary. A valid publication for the previous pair
                    // does not publish pixels for this pair.
                    messages.push(format!("display bank{} unpublished OSD pair primary={address:#x} secondary={secondary:#x} published_primary={:#x}",self.bank,publication.primary));
                    continue;
                }
                let h = publication.geometry;
                // A descriptor publishes its dimensions at+18,
                // Q12 sampling ratios at+24. Work dimensions can
                // crop the padded pixmap (LCD900 versus source912).
                let (source_width, source_height) = (sx, sy);
                let (qx, qy) = (sw, sh);
                if qx == 0 || qy == 0 || source_width > h.width || source_height > h.height {
                    return Err("OSD source dimensions/sampling ratios invalid".into());
                }
                sx = 0;
                sy = 0;
                sw = ((ww - 1) * qx / 4096) + 1;
                sh = ((wh - 1) * qy / 4096) + 1;
                sampling = Some([qx, qy]);
                if sx + sw > h.width || sy + sh > h.height {
                    return Err("OSD source rectangle exceeds converted pixmap".into());
                }
                (PixelOrder::Bgra, h.stride)
            };
            planes.push(Plane {
                address,
                stride,
                source_x: sx,
                source_y: sy,
                source_width: sw,
                source_height: sh,
                sampling,
                work_x: wx,
                work_y: wy,
                work_width: ww,
                work_height: wh,
                order,
            });
        }
        Ok((planes, messages))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_decoder_never_accepts_the_emulator_header() {
        let mut regs = vec![0; REGISTER_BYTES / 4];
        regs[2] = 1 << 20;
        let d = Decoder {
            bank: 0,
            regs: &regs,
            formats: [Encoding::Unresolved; 4],
        };
        let error = d
            .planes(|_, _| panic!("Unresolved encoding must fail before any memory read"))
            .unwrap_err();
        assert!(error.contains("native converted plane encoding is unresolved"));
    }

    /// Picture group words as the stock commit writes them for a packed
    /// 4:2:2 layer: address, second plane, control, stride, source size,
    /// work origin/size, Q12 ratio, crop and phase.
    struct Group {
        address: u64,
        chroma: u64,
        control: u32,
        stride: u32,
        source: (u32, u32),
        work: (u32, u32, u32, u32),
        ratio: (u32, u32),
        crop: u32,
        phase: u32,
    }
    fn regs_with(bank: usize, group: usize, g: &Group) -> Vec<u32> {
        let mut regs = vec![0; REGISTER_BYTES / 4];
        let a = if bank == 0 {
            [0x370, 0x3c0, 0x410, 0x460]
        } else {
            [0x370, 0x3b8, 0x3fc, 0x440]
        }[group]
            / 4;
        regs[2] = 1 << [12, 8, 4, 0][group];
        regs[a] = (g.address >> 32) as u32;
        regs[a + 1] = g.address as u32;
        regs[a + 2] = (g.chroma >> 32) as u32;
        regs[a + 3] = g.chroma as u32;
        regs[a + 4] = g.control;
        regs[a + 5] = g.stride;
        regs[a + 6] = g.source.1 << 16 | g.source.0;
        regs[a + 7] = g.work.1 << 16 | g.work.0;
        regs[a + 8] = g.work.3 << 16 | g.work.2;
        regs[a + 9] = g.ratio.1 << 16 | g.ratio.0;
        regs[a + 10] = g.crop;
        regs[a + 11] = g.phase;
        regs
    }
    fn playback_preview() -> Group {
        // playcont 0x56e5c68: layer 1, 640x480 packed 4:2:2 at the decoder
        // output, the same buffer as second plane, pitch 2 * 640.
        Group {
            address: 0x12c74000,
            chroma: 0x12c74000,
            control: 0,
            stride: 0x500,
            source: (640, 480),
            work: (0, 0, 1280, 960),
            ratio: (0x800, 0x800),
            crop: 0,
            phase: 0,
        }
    }
    fn decode(bank: usize, regs: &[u32]) -> Result<Vec<Plane>, String> {
        Decoder {
            bank,
            regs,
            formats: [Encoding::Unresolved; 4],
        }
        .planes(|_, _| panic!("picture groups need no header read"))
        .map(|(planes, _)| planes)
    }
    #[test]
    fn playback_preview_scales_to_the_panel() {
        let regs = regs_with(0, 1, &playback_preview());
        let planes = decode(0, &regs).unwrap();
        assert_eq!(planes.len(), 1);
        let p = planes[0];
        assert_eq!(p.address, 0x12c74000);
        assert_eq!(p.stride, 0x500);
        assert_eq!((p.source_x, p.source_y), (0, 0));
        assert_eq!((p.source_width, p.source_height), (640, 480));
        assert_eq!((p.work_width, p.work_height), (1280, 960));
        assert_eq!(p.sampling, Some([0x800, 0x800]));
        assert_eq!(p.order, PixelOrder::YyCbCr);
        // Live view programs no second plane; that stays accepted.
        let mut live = playback_preview();
        live.chroma = 0;
        assert_eq!(decode(1, &regs_with(1, 0, &live)).unwrap()[0], p);
    }
    #[test]
    fn start_phase_moves_the_source_origin() {
        let mut g = playback_preview();
        g.work = (0, 0, 600, 400);
        g.ratio = (0x1000, 0x1000);
        // Crop 2 pixels (half-pixel units), phase 1.5 and 3.0 pixels (Q12).
        g.crop = 8 << 16 | 4;
        g.phase = 0x3000 << 16 | 0x1800;
        let p = decode(1, &regs_with(1, 1, &g)).unwrap()[0];
        assert_eq!((p.source_x, p.source_y), (3, 7));
        assert_eq!((p.source_width, p.source_height), (600, 400));
        // The offset still has to fit inside the programmed source.
        g.work = (0, 0, 640, 480);
        let error = decode(1, &regs_with(1, 1, &g)).unwrap_err();
        assert!(error.contains("exceeds programmed dimensions"), "{error}");
        // Bit 15 of a phase half is not decoded.
        g.work = (0, 0, 600, 400);
        g.phase = 0x8000;
        let error = decode(1, &regs_with(1, 1, &g)).unwrap_err();
        assert!(error.contains("+0x2c=0x8000"), "{error}");
    }
    #[test]
    fn refusals_name_the_group_and_its_words() {
        // 10-bit split layout (playcont 0x56e5d28): chroma after the luma.
        let mut g = playback_preview();
        g.control = PICTURE_PACKED_10BIT;
        g.chroma = 0x12c74000 + 0x64000;
        g.stride = 640 / 6 * 8;
        let error = decode(0, &regs_with(0, 1, &g)).unwrap_err();
        for part in [
            "unsupported picture address/packing/fractional phase",
            "bank0 group1",
            "0xff990000+0x3c0",
            "10-bit",
            "+04=0x12c74000",
            "+0c=0x12cd8000",
            "+10(control)=0x100",
            "+14(stride)=0x350",
        ] {
            assert!(error.contains(part), "{part}: {error}");
        }
        for (control, chroma, reason) in [
            (1, 0, "descriptor format"),
            (1 << 16, 0, "undecoded control bits"),
            (0, 0x1000_0000, "second plane differs"),
        ] {
            let mut g = playback_preview();
            g.control = control;
            g.chroma = chroma;
            let error = decode(1, &regs_with(1, 2, &g)).unwrap_err();
            assert!(error.contains(reason), "{reason}: {error}");
            assert!(error.contains("bank1 group2"), "{error}");
        }
        let mut regs = regs_with(0, 0, &playback_preview());
        regs[0x370 / 4] = 8;
        let error = decode(0, &regs).unwrap_err();
        assert!(error.contains("address-high"), "{error}");
    }
}
