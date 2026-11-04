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
            if self.r(a) > 7
                || self.r(a + 8) != 0
                || self.r(a + 12) != 0
                || self.r(a + 0x10) != 0
                || self.r(a + 0x14) > 0xfffff
                || self.r(a + 0x2c) != 0
            {
                return Err("unsupported picture address/packing/fractional phase".into());
            }
            for (offset, mask) in [
                (0x18, 0x3fff),
                (0x1c, 0x1fff),
                (0x20, 0x1fff),
                (0x24, 0x7fff),
                (0x28, 0x7fff),
            ] {
                if self.r(a + offset) & !(mask | mask << 16) != 0 {
                    return Err("unresolved picture geometry control bits".into());
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
            // +28 crop origins are in half-pixel units. Fractional phases
            // remain unsupported; observed zero phases use nearest sampling.
            let (crop_x, crop_y) = xy(self.r(a + 0x28), 0x7fff);
            if qx == 0 || qy == 0 || (crop_x | crop_y) & 1 != 0 {
                return Err("picture sampling/crop origin invalid".into());
            }
            let (sx, sy) = (crop_x / 2, crop_y / 2);
            let sw = ((ww - 1) * qx / 4096) + 1;
            let sh = ((wh - 1) * qy / 4096) + 1;
            if sx + sw > source_width || sy + sh > source_height {
                return Err("picture source rectangle exceeds programmed dimensions".into());
            }
            let address = ((self.r(a) & 7) as u64) << 32 | self.r(a + 4) as u64;
            planes.push(Plane {
                address,
                stride: self.r(a + 0x14) & 0xfffff,
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
}
