//! Physical capture uses the processor decoders directly. No firmware RAM shadows.
use fujimod_catalog::reference::Soc;
use fujimod_framebuffer::bmp;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Image {
    pub name: String,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug)]
pub enum Profile {
    Exr2 { ram_bytes: u64 },
    Xpp,
    Xp4 { picture: bool, video: bool },
    Xp5,
}
impl Profile {
    /// Decoder selection follows the resolved SoC, independently of firmware identity.
    pub fn from_soc(soc: &Soc, ram_bytes: Option<u64>) -> Option<Self> {
        let block = |id: &str, base: u64, bytes: u64| {
            soc.blocks
                .iter()
                .any(|b| b.id == id && b.base == base && b.size >= bytes)
        };
        match soc.id.as_str() {
            "exr2"
                if block("scanout", fujimod_framebuffer_exr2::BASES[0], 0x1a0)
                    && block("scanout-aux", fujimod_framebuffer_exr2::BASES[1], 0x1a0) =>
            {
                Some(Self::Exr2 {
                    ram_bytes: ram_bytes?,
                })
            }
            "xpp"
                if block(
                    "display",
                    fujimod_framebuffer_xpp::BASES[1],
                    fujimod_framebuffer_xpp::REGISTER_BYTES as u64,
                ) =>
            {
                Some(Self::Xpp)
            }
            "xp4"
                if block(
                    "lcdc1",
                    fujimod_framebuffer_xp4::BASES[1],
                    fujimod_framebuffer_xp4::REGISTER_BYTES as u64,
                ) =>
            {
                use fujimod_catalog::soc::BlockHardware;
                let b = soc.hardware.as_ref()?.blocks.get("lcdc1")?;
                let BlockHardware::Display {
                    picture_layout,
                    video_layout,
                } = b
                else {
                    return None;
                };
                Some(Self::Xp4 {
                    picture: picture_layout.is_some(),
                    video: video_layout.is_some(),
                })
            }
            "xp5"
                if block(
                    "display0",
                    fujimod_framebuffer_xp5::BASES[0],
                    fujimod_framebuffer_xp5::REGISTER_BYTES as u64,
                ) && block(
                    "display1",
                    fujimod_framebuffer_xp5::BASES[1],
                    fujimod_framebuffer_xp5::REGISTER_BYTES as u64,
                ) =>
            {
                Some(Self::Xp5)
            }
            _ => None,
        }
    }
    pub fn outputs(&self) -> Vec<u32> {
        match self {
            Self::Xpp => vec![1],
            Self::Xp4 { .. } => vec![1],
            _ => vec![0, 1],
        }
    }
    fn register_reads(&self, output: u32) -> Result<Vec<(u64, usize)>, String> {
        if !self.outputs().contains(&output) {
            return Err("Unknown display output".into());
        }
        Ok(match self {
            Self::Exr2 { .. } => fujimod_framebuffer_exr2::register_ranges(output as usize)?,
            Self::Xpp => fujimod_framebuffer_xpp::register_ranges(output as usize)?,
            Self::Xp4 { .. } => vec![(
                fujimod_framebuffer_xp4::BASES[output as usize],
                fujimod_framebuffer_xp4::REGISTER_BYTES,
            )],
            Self::Xp5 => vec![(
                fujimod_framebuffer_xp5::BASES[output as usize],
                fujimod_framebuffer_xp5::REGISTER_BYTES,
            )],
        })
    }
    pub fn capture(
        &self,
        output: u32,
        mut read: impl FnMut(u64, usize) -> Result<Vec<u8>, String>,
    ) -> Result<Vec<Image>, String> {
        let ranges = self.register_reads(output)?;
        for _ in 0..3 {
            let mut before = Vec::new();
            for &(a, n) in &ranges {
                let b = read(a, n)?;
                if b.len() != n {
                    return Err("Short display register read".into());
                }
                before.push(b);
            }
            let mut words = BTreeMap::new();
            let base = ranges[0].0;
            for ((a, _), bytes) in ranges.iter().zip(&before) {
                for (i, b) in bytes.chunks_exact(4).enumerate() {
                    words.insert(
                        a - base + i as u64 * 4,
                        u32::from_le_bytes(b.try_into().unwrap()),
                    );
                }
            }
            let mut read_error: Option<String> = None;
            let mut remaining = 128usize << 20;
            let mut guarded = |a, n| {
                if let Some(error) = &read_error {
                    return Err(error.clone());
                }
                if n > remaining {
                    let error = String::from("Display capture exceeds its byte budget");
                    read_error = Some(error.clone());
                    return Err(error);
                }
                remaining -= n;
                match read(a, n) {
                    Ok(bytes) => {
                        if bytes.len() != n {
                            let error = String::from("Short framebuffer read");
                            read_error = Some(error.clone());
                            Err(error)
                        } else {
                            Ok(bytes)
                        }
                    }
                    Err(error) => {
                        read_error = Some(error.clone());
                        Err(error)
                    }
                }
            };
            let images = self.decode(output, &words, &mut guarded);
            drop(guarded);
            if let Some(error) = read_error {
                return Err(error);
            }
            let mut images = images?;
            let mut stable = true;
            for (&(a, n), b) in ranges.iter().zip(&before) {
                if read(a, n)? != *b {
                    stable = false;
                }
            }
            if stable {
                images.push(Image{name:"state.json".into(),bytes:serde_json::to_vec_pretty(&serde_json::json!({"output":output,"register_base":base,"registers":words,"snapshot":"registers-unchanged-during-capture"})).map_err(|e|e.to_string())?});
                return Ok(images);
            }
        }
        Err("Display registers changed during all three capture attempts".into())
    }
    fn decode(
        &self,
        output: u32,
        words: &BTreeMap<u64, u32>,
        read: &mut impl FnMut(u64, usize) -> Result<Vec<u8>, String>,
    ) -> Result<Vec<Image>, String> {
        let mut images = vec![];
        let mut emit = |name: String, w: u32, h: u32, rgba: Vec<u8>| -> Result<(), String> {
            images.push(Image {
                name,
                bytes: bmp(w, h, &rgba)?,
            });
            Ok(())
        };
        match self {
            Self::Exr2 { ram_bytes } => {
                use fujimod_framebuffer_exr2::{
                    BackdropProfile, Compositor, GraphicsProfile, Observation, RectangleComposition,
                };
                let mut c = Compositor {
                    pipes: Default::default(),
                    packing: "yycbcr".into(),
                    matrix: "bt601-full".into(),
                    graphics_profile: Some(GraphicsProfile::Abgr8888Straight),
                    backdrop_profile: None,
                    rectangle_composition: Some(RectangleComposition::SignedYCbCr),
                    ram_bytes: *ram_bytes as i64,
                    max_pixels: 16_777_216,
                };
                let p = &mut c.pipes[output as usize];
                for (&o, &v) in words {
                    p.regs.insert(o as u32, v);
                    p.written.insert(o as u32);
                }
                let convert=|frame:fujimod_framebuffer::frame::Frame|->Result<(u32,u32,Vec<u8>),String>{let rgb=frame.rgb.ok_or("Display has no pixels")?;Ok((frame.width as u32,frame.height as u32,rgb.chunks_exact(3).flat_map(|p|[p[0],p[1],p[2],255]).collect()))};
                let render = |c: &Compositor,
                              read: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>, String>|
                 -> Result<Option<(u32, u32, Vec<u8>)>, String> {
                    c.frame(
                        output as usize,
                        0,
                        true,
                        &mut Observation {
                            read,
                            diagnostic_hashes: false,
                        },
                    )
                    .map_err(|e| e.0)?
                    .map(convert)
                    .transpose()
                };
                if let Ok(Some((w, h, rgba))) = render(&c, read) {
                    emit("frame.bmp".into(), w, h, rgba)?;
                }
                // Each isolated hardware group/channel is rendered at its programmed output position.
                for (register, channel) in [(0x00, 0)]
                    .into_iter()
                    .chain((0..2).flat_map(|g| (0..4).map(move |ch| (0x40 + g * 0x50, ch))))
                {
                    if words.get(&(register as u64)).copied().unwrap_or(0) & (1 << (channel * 4))
                        == 0
                    {
                        continue;
                    }
                    let mut layer = c.clone();
                    layer.backdrop_profile = Some(BackdropProfile::Black);
                    let p = &mut layer.pipes[output as usize];
                    for r in [0x00, 0x40, 0x90, 0x108] {
                        let v = p.regs.get_or_zero(r);
                        p.regs.insert(
                            r,
                            if r == register {
                                if r == 0 { v } else { 1 << (channel * 4) }
                            } else {
                                if r == 0 { v & !1 } else { 0 }
                            },
                        );
                    }
                    if let Ok(Some((w, h, rgba))) = render(&layer, read) {
                        emit(format!("layer/{register:x}.{channel}.bmp"), w, h, rgba)?;
                    }
                }
            }
            Self::Xpp => {
                let compose=|w:&BTreeMap<u64,u32>,read:&mut dyn FnMut(u64,usize)->Result<Vec<u8>,String>|fujimod_framebuffer_xpp::Decoder{words:w}.compose(read);
                if let Ok(Some((w, h, rgba, _))) = compose(words, read) {
                    emit("frame.bmp".into(), w, h, rgba)?;
                }
                for (r, bit) in [(0x00, 2)].into_iter().chain((0..4).map(|i| (0x200, i))) {
                    if words.get(&r).copied().unwrap_or(0) & (1 << bit) == 0 {
                        continue;
                    }
                    let mut layer = words.clone();
                    layer.insert(0, words.get(&0).copied().unwrap_or(0) & !(1 << 2));
                    layer.insert(0x200, 0);
                    layer.insert(r, if r == 0 { words[&0] } else { 1 << bit });
                    if let Ok(Some((w, h, rgba, _))) = compose(&layer, read) {
                        emit(format!("layer/{r:x}.{bit}.bmp"), w, h, rgba)?;
                    }
                }
            }
            Self::Xp4 { picture, video } => {
                let mut regs = vec![0; fujimod_framebuffer_xp4::REGISTER_BYTES / 4];
                for (&o, &v) in words {
                    regs[o as usize / 4] = v;
                }
                let decode = |r: &[u32]| {
                    fujimod_framebuffer_xp4::decode(
                        r,
                        picture.then_some(fujimod_framebuffer_xp4::Picture::Bgra8888Straight),
                        video.then_some(fujimod_framebuffer_xp4::Video::YyCbCr422),
                    )
                };
                if let Ok(Some(out)) = decode(&regs) {
                    emit(
                        "frame.bmp".into(),
                        out.width,
                        out.height,
                        out.render(&mut *read)?,
                    )?;
                }
                for bit in [4, 12] {
                    let mut r = regs.clone();
                    r[2] &= 1 << bit;
                    if r[2] == 0 {
                        continue;
                    }
                    if let Ok(Some(out)) = decode(&r) {
                        emit(
                            format!("layer/{bit}.bmp"),
                            out.width,
                            out.height,
                            out.render(&mut *read)?,
                        )?;
                    }
                }
            }
            Self::Xp5 => {
                use fujimod_framebuffer_xp5::{Decoder, Encoding, REGISTER_BYTES};
                let mut regs = vec![0; REGISTER_BYTES / 4];
                for (&o, &v) in words {
                    regs[o as usize / 4] = v;
                }
                let render = |r: &[u32],
                              read: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>, String>|
                 -> Result<Option<(u32, u32, Vec<u8>)>, String> {
                    if r[1] != 1 {
                        return Ok(None);
                    }
                    let (planes, _) = Decoder {
                        bank: output as usize,
                        regs: r,
                        formats: [Encoding::Unresolved; 4],
                    }
                    .planes(&mut *read)?;
                    let width = r[0x14 / 4] & 0x3fff;
                    let height = r[if output == 0 { 0x20 / 4 } else { 0x24 / 4 }] & 0x1fff;
                    let mut rgba = fujimod_framebuffer::canvas(width, height)?;
                    for p in planes {
                        fujimod_framebuffer::raster::composite(
                            &mut *read, p, width, height, &mut rgba,
                        )?;
                    }
                    Ok(Some((width, height, rgba)))
                };
                if let Ok(Some((w, h, rgba))) = render(&regs, read) {
                    emit("frame.bmp".into(), w, h, rgba)?;
                }
                for bit in [0, 4, 8, 12, 16, 20] {
                    let mut r = regs.clone();
                    r[2] &= 1 << bit;
                    if r[2] == 0 {
                        continue;
                    }
                    if let Ok(Some((w, h, rgba))) = render(&r, read) {
                        emit(format!("layer/{bit}.bmp"), w, h, rgba)?;
                    }
                }
            }
        }
        Ok(images)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_follows_soc_and_does_not_require_firmware_framebuffer_facts() {
        let c = fujimod_catalog::ReferenceCatalog::bundled().unwrap();
        for (soc, outputs) in [
            ("exr2", vec![0, 1]),
            ("xpp", vec![1]),
            ("xp4", vec![1]),
            ("xp5", vec![0, 1]),
        ] {
            assert_eq!(
                Profile::from_soc(c.soc(soc).unwrap(), Some(0x10000000))
                    .unwrap()
                    .outputs(),
                outputs
            );
        }
    }
    #[test]
    fn a_catalog_register_bank_must_match_the_processor_decoder() {
        let catalog = fujimod_catalog::ReferenceCatalog::bundled().unwrap();
        let mut soc = catalog.soc("xp4").unwrap().clone();
        soc.blocks
            .iter_mut()
            .find(|b| b.id == "lcdc1")
            .unwrap()
            .base += 0x1000;
        assert!(Profile::from_soc(&soc, Some(0x10000000)).is_none());
    }
    #[test]
    fn changing_registers_have_a_bounded_retry_and_disabled_outputs_have_no_frame() {
        let p = Profile::Xp4 {
            picture: true,
            video: true,
        };
        let mut count = 0;
        let result = p.capture(1, |_, n| {
            count += 1;
            Ok(vec![count; n])
        });
        assert!(result.unwrap_err().contains("three capture attempts"));
        assert_eq!(count, 6);
        let images = p.capture(1, |_, n| Ok(vec![0; n])).unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].name, "state.json");
    }
}

#[cfg(test)]
mod capture_tests {
    use super::*;
    fn fixture(a: u64, n: usize) -> Vec<u8> {
        let words: BTreeMap<u64, u32> = [
            (0, 4),
            (0x10, 0x333),
            (0x14, 0x10000),
            (0x2c, 0x10000),
            (0x30, 0x1000),
            (0x200, 0x10),
        ]
        .into_iter()
        .collect();
        if a == 0x1000 {
            assert_eq!(n, 4);
            return vec![16, 235, 128, 128];
        }
        (0..n / 4)
            .flat_map(|i| {
                words
                    .get(&(a - 0xfff64000 + i as u64 * 4))
                    .copied()
                    .unwrap_or(0)
                    .to_le_bytes()
            })
            .collect()
    }
    #[test]
    fn an_unknown_enabled_layer_cannot_be_published_as_a_complete_frame() {
        let images = Profile::Xpp.capture(1, |a, n| Ok(fixture(a, n))).unwrap();
        assert!(!images.iter().any(|i| i.name == "frame.bmp"));
        let layer = images.iter().find(|i| i.name == "layer/0.2.bmp").unwrap();
        assert_eq!(&layer.bytes[122..], &[0, 0, 0, 255, 255, 255, 255, 255]);
    }
    #[test]
    fn a_failed_pixel_read_ends_capture_without_retry_or_completion_reads() {
        let mut reads = 0;
        let result = Profile::Xpp.capture(1, |a, n| {
            reads += 1;
            if a == 0x1000 {
                Err("completion unknown".into())
            } else {
                Ok(fixture(a, n))
            }
        });
        assert_eq!(result.unwrap_err(), "completion unknown");
        assert_eq!(reads, 3);
    }
}
