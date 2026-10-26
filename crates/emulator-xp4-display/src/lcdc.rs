//! XP4 LCDC timing and explicitly selected raw picture transport.
//! X-T3 010e4920 programs totals/porches; 010f6010 selects physical
//! phase routes. The input pixel clock and phase law are explicit functional
//! assumptions, not measured silicon timing. No guest software is inspected.
use fujimod_emulator_graphics::raster::{self, PixelOrder, Plane};
use fujimod_emulator_devices::{graphics::scanout::Frame, Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
struct State {
    regs: Vec<u32>,
    deadline: Option<u64>,
    phase: usize,
    frames: u64,
    sequence: u64,
    image_hash: Option<[u8; 32]>,
}
/// Selected by the processor graph; no implicit pixel-format fallback.
#[derive(Clone, Copy)]
pub enum Picture {
    Bgra8888Straight,
}
/// Explicit linear video transport; hardware packing is not inferred from RAM.
#[derive(Clone, Copy)]
pub enum Video {
    YyCbCr422,
}
pub struct Lcdc {
    base: u64,
    ticks_per_pixel: u64,
    irqs: [u32; 2],
    picture: Option<Picture>,
    video: Option<Video>,
    state: State,
}
impl Lcdc {
    pub fn new(
        base: u64,
        ticks_per_pixel: u64,
        irqs: [u32; 2],
        picture: Option<Picture>,
        video: Option<Video>,
    ) -> Result<Self, String> {
        if ![0xfff7a000, 0xfffa9000].contains(&base)
            || !(1..=1024).contains(&ticks_per_pixel)
            || irqs.iter().any(|i| !(32..512).contains(i))
            || irqs[0] == irqs[1]
        {
            return Err("Invalid XP4 LCDC base, pixel clock or phase IRQ routes".into());
        }
        Ok(Self {
            base,
            ticks_per_pixel,
            irqs,
            picture,
            video,
            state: State {
                regs: vec![0; 0x604 / 4],
                deadline: None,
                phase: 0,
                frames: 0,
                sequence: 0,
                image_hash: None,
            },
        })
    }
    fn r(&self, offset: usize) -> u32 {
        self.state.regs[offset / 4]
    }
    fn phases(&self) -> Result<[u64; 2], String> {
        let h = self.r(0x14);
        let hp = self.r(0x18);
        let v = self.r(0x20);
        let vp = self.r(0x24);
        let width = h & 0x3fff;
        let height = v & 0x1fff;
        let blank = ((v >> 16) & 0x1ff) + (vp & 0xfff) + ((vp >> 16) & 0xfff);
        let total = width + ((h >> 16) & 0x1ff) + (hp & 0xfff) + ((hp >> 16) & 0xfff);
        if width == 0
            || height == 0
            || blank == 0
            || total == 0
            || self.r(0x10) != total
            || self.r(0x1c) != height + blank
        {
            return Err("XP4 LCDC active area, porches or horizontal total not programmed".into());
        }
        let line = u64::from(total) * self.ticks_per_pixel;
        // End of active picture -> vertical blank -> next active picture.
        Ok([line * u64::from(height), line * u64::from(blank)])
    }
    fn output(&self) -> usize {
        usize::from(self.base == 0xfffa9000)
    }
    fn status(
        &self,
        cx: &mut Cx<'_>,
        active: bool,
        source: u64,
        format: Option<&str>,
    ) -> Result<(), String> {
        cx.output_status(
            self.output(),
            serde_json::json!({
                "enabled": self.r(4) == 1, "active": active,
                "format": format,
                "source": format!("0x{source:x}"), "sequence": self.state.sequence,
                "width": self.r(0x14) & 0x3fff, "height": self.r(0x20) & 0x1fff,
                "tick": cx.now(), "capture": "active-picture-phase",
            }),
        )
    }
    fn pair(&self, offset: usize, mask: u32) -> Result<(u32, u32), String> {
        let word = self.r(offset);
        if word & !(mask | mask << 16) != 0 {
            return Err("Unsupported XP4 LCDC geometry bits".into());
        }
        Ok((word & mask, word >> 16))
    }
    fn plane(&self, video: bool, width: u32, height: u32) -> Result<Plane, String> {
        let (address, stride, geometry, order) = if video {
            if self.video.is_none() || self.r(0x164) > 3 || self.r(0x16c) & !0xfff != 0 {
                return Err("Unsupported XP4 LCDC video transport/address/packing".into());
            }
            // 010ed130 writes descriptor+1ea (pitch in pixels) /4 to +16c.
            // This selected packed422 transport therefore consumes 8 bytes
            // per pitch unit, unlike the raw BGRA plane's pixel pitch at+50.
            (
                ((u64::from(self.r(0x164))) << 32) | u64::from(self.r(0x168)),
                (self.r(0x16c) & 0xfff) * 8,
                0x170,
                PixelOrder::YyCbCr,
            )
        } else {
            let source = (u64::from(self.r(0x40)) << 32) | u64::from(self.r(0x44));
            let alternate = (u64::from(self.r(0x48)) << 32) | u64::from(self.r(0x4c));
            if self.picture.is_none() || self.r(0x40) > 3 || source != alternate {
                return Err("Unsupported XP4 LCDC alternate picture selection".into());
            }
            if self.r(0x50) & !0xfff != 0x10000 {
                return Err("Unsupported XP4 LCDC raw picture mode".into());
            }
            (source, (self.r(0x50) & 0xfff) * 4, 0x54, PixelOrder::Bgra)
        };
        if self.r(geometry + 0x10) != 0 || self.r(geometry + 0x14) != 0 {
            return Err("Unsupported XP4 LCDC picture crop or fractional phase".into());
        }
        let (sw, sh) = self.pair(geometry, 0xfff)?;
        let (x, y) = self.pair(geometry + 4, 0x1fff)?;
        let (w, h) = self.pair(geometry + 8, 0xfff)?;
        let (dx, dy) = self.pair(geometry + 12, 0x7fff)?;
        if sw == 0
            || sh == 0
            || w == 0
            || h == 0
            || dx == 0
            || dy == 0
            || address == 0
            || address & 3 != 0
            || x + w > width
            || y + h > height
            || ((w - 1) * dx >> 12) >= sw
            || ((h - 1) * dy >> 12) >= sh
        {
            return Err("Invalid XP4 LCDC picture geometry/address".into());
        }
        Ok(Plane {
            address,
            stride,
            source_x: 0,
            source_y: 0,
            source_width: sw,
            source_height: sh,
            sampling: Some([dx, dy]),
            work_x: x,
            work_y: y,
            work_width: w,
            work_height: h,
            order,
        })
    }
    fn capture(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if self.picture.is_none() && self.video.is_none() {
            return Ok(());
        }
        let enable = self.r(8);
        if enable & !0x11010 != 0 {
            return Err("Unsupported XP4 LCDC active layer selection".into());
        }
        // 010ed130 maps context+e0 bit4 to the video group at+164.
        // +8 bit16 is output control; it does not enable the group at+130.
        let width = self.r(0x14) & 0x3fff;
        let height = self.r(0x20) & 0x1fff;
        if width as u64 * height as u64 > 16_777_216 {
            return Err("XP4 LCDC output exceeds capture bounds".into());
        }
        let mut planes = Vec::new();
        for (bit, video) in [(4, true), (12, false)] {
            if enable & (1 << bit) != 0 {
                planes.push(self.plane(video, width, height)?);
            }
        }
        let Some(top) = planes.last() else {
            return self.status(cx, false, 0, None);
        };
        let source = top.address;
        let format = if top.order == PixelOrder::YyCbCr {
            "yycbcr422"
        } else {
            "bgra8888-straight"
        };
        let mut rgba = vec![0; width as usize * height as usize * 4];
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        for plane in planes {
            raster::composite(
                |address, count| cx.ram_bytes(address, count),
                plane,
                width,
                height,
                &mut rgba,
            )?;
        }
        let mut digest = Sha256::new();
        digest.update(width.to_le_bytes());
        digest.update(height.to_le_bytes());
        digest.update(&rgba);
        let hash: [u8; 32] = digest.finalize().into();
        if self.state.image_hash != Some(hash) {
            self.state.sequence += 1;
            self.state.image_hash = Some(hash);
        }
        cx.scanout(Frame {
            output: self.output(),
            sequence: self.state.sequence,
            width,
            height,
            rgba,
        })?;
        self.status(cx, true, source, Some(format))
    }
    fn register(offset: u64) -> bool {
        matches!(
            offset,
            0 | 4 | 8 | 0x10 | 0x14 | 0x18 | 0x1c | 0x20 | 0x24 | 0x2c | 0x600
        ) || ((0x40..=0x320).contains(&offset) && offset != 0x1c4)
            || (0x400..=0x514).contains(&offset)
    }
}
impl Device for Lcdc {
    fn name(&self) -> &'static str {
        if self.base == 0xfff7a000 {
            "xp4-lcdc0"
        } else {
            "xp4-lcdc1"
        }
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.advance(cx);
        let offset = address.wrapping_sub(self.base);
        if size != 4 || offset & 3 != 0 || !Self::register(offset) {
            cx.unmodeled();
            return 0;
        }
        if offset == 0 {
            1
        } else {
            u64::from(self.r(offset as usize))
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.advance(cx);
        let offset = address.wrapping_sub(self.base);
        if size != 4 || offset & 3 != 0 || !Self::register(offset) {
            cx.unmodeled();
            return;
        }
        let value = value as u32;
        if offset == 0 {
            if value != 0 {
                cx.fail("Unsupported XP4 LCDC reset value");
            }
            return; // Reset handshake retains programmed timing/output latches.
        }
        if offset == 4 {
            if value > 1 {
                cx.fail("Unsupported XP4 LCDC output-enable bits");
                return;
            }
            if value == self.r(4) {
                return;
            }
            if value != 0 {
                match self.phases() {
                    Ok(delays) => {
                        self.state.phase = 0;
                        self.state.deadline = cx.now().checked_add(delays[0]);
                        if self.state.deadline.is_none() {
                            cx.fail("LCDC deadline overflow");
                            return;
                        }
                    }
                    Err(error) => {
                        cx.fail(error);
                        return;
                    }
                }
            } else {
                self.state.deadline = None;
            }
        }
        self.state.regs[offset as usize / 4] = value;
        if offset == 4 && value == 0 {
            if let Err(error) = self.status(cx, false, 0, None) {
                cx.fail(error);
            }
        }
        if offset == 8 && value != 0 && self.picture.is_none() && self.video.is_none() {
            // Pixel transport is a separate unsupported hardware boundary.
            // Discovery may retain its writes, but this never publishes pixels.
            cx.unmodeled();
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        while let Some(at) = self.state.deadline.filter(|at| *at <= cx.now()) {
            let delays = match self.phases() {
                Ok(delays) => delays,
                Err(error) => {
                    self.state.deadline = None;
                    cx.fail(error);
                    return;
                }
            };
            let phase = self.state.phase;
            if phase == 1 {
                if let Err(error) = self.capture(cx) {
                    self.state.deadline = None;
                    cx.fail(error);
                    return;
                }
            }
            cx.set_irq(self.irqs[phase], true);
            cx.set_irq(self.irqs[phase], false);
            if phase == 0 {
                self.state.frames += 1;
            }
            self.state.phase ^= 1;
            self.state.deadline = at.checked_add(delays[self.state.phase]);
            if self.state.deadline.is_none() {
                cx.fail("LCDC deadline overflow");
                return;
            }
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state.deadline
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let s: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if s.regs.len() != 0x604 / 4
            || s.phase > 1
            || s.regs[1] > 1
            || s.deadline.is_some() != (s.regs[1] == 1)
        {
            return Err("Invalid XP4 LCDC timing checkpoint".into());
        }
        self.state = s;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{Host, Record};
    #[derive(Default)]
    struct Clock {
        now: u64,
        edges: Vec<(u64, u32)>,
        ram: Vec<u8>,
        images: Vec<Frame>,
        outputs: Vec<Value>,
    }
    impl Host for Clock {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            unreachable!()
        }
        fn set_irq(&mut self, irq: u32, level: bool) {
            if level {
                self.edges.push((self.now, irq));
            }
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            unreachable!()
        }
        fn ram_read(&self, address: u64, out: &mut [u8]) -> Result<(), String> {
            let bytes = self
                .ram
                .get(address as usize..address as usize + out.len())
                .ok_or("Test DMA outside RAM")?;
            out.copy_from_slice(bytes);
            Ok(())
        }
        fn scanout(&mut self, frame: Frame) -> Result<(), String> {
            self.images.push(frame);
            Ok(())
        }
        fn output_status(&mut self, _: usize, state: Value) -> Result<(), String> {
            self.outputs.push(state);
            Ok(())
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            unreachable!()
        }
        fn record(&mut self, _: Record) {}
    }
    fn write(d: &mut Lcdc, h: &mut Clock, pc: u64, offset: u64, value: u64) {
        let base = d.base;
        let mut cx = Cx::new(h, 3, pc, 0);
        d.write(&mut cx, base + offset, 4, value);
        assert!(cx.refusal().is_none(), "{:?}", cx.refusal());
    }
    fn experiment(pc: u64, noisy: bool) -> (Value, Vec<(u64, u32)>) {
        let mut d = Lcdc::new(0xfffa9000, 2, [117, 118], None, None).unwrap();
        let mut h = Clock::default();
        for (offset, value) in [
            (0x10, 8),
            (0x14, 0x10004),
            (0x18, 0x10002),
            (0x1c, 5),
            (0x20, 0x10002),
            (0x24, 0x10001),
            (4, 1),
        ] {
            write(&mut d, &mut h, pc, offset, value);
        }
        assert_eq!(d.next_deadline(0), Some(32));
        for tick in 1..=160 {
            h.now = tick;
            if noisy {
                let before = d.save();
                assert_eq!(d.save(), before);
                d.advance(&mut Cx::new(&mut h, 1, pc + 4, 0));
                if tick < 32 {
                    assert_eq!(d.save(), before);
                }
            } else if d.next_deadline(tick).is_some_and(|at| at <= tick) {
                d.advance(&mut Cx::new(&mut h, 0, pc, 0));
            }
        }
        assert_eq!(h.edges, [(32, 117), (80, 118), (112, 117), (160, 118)]);
        assert_eq!(d.next_deadline(160), Some(192));
        let before = d.save();
        let mut restored = Lcdc::new(0xfffa9000, 2, [117, 118], None, None).unwrap();
        restored.restore(&before).unwrap();
        assert_eq!(restored.save(), before);
        write(&mut d, &mut h, pc, 4, 0);
        assert_eq!(d.next_deadline(160), None);
        (before, h.edges)
    }
    #[test]
    fn exact_phase_deadlines_early_ticks_and_observations_are_program_independent() {
        assert_eq!(experiment(0x1000, false), experiment(0x987654, true));
    }
    fn picture() -> (Lcdc, Clock) {
        let mut d = Lcdc::new(
            0xfffa9000,
            2,
            [117, 118],
            Some(Picture::Bgra8888Straight),
            None,
        )
        .unwrap();
        let mut h = Clock {
            ram: vec![0; 64],
            ..Clock::default()
        };
        h.ram[16..24].copy_from_slice(&[30, 20, 10, 255, 100, 80, 60, 128]);
        // Two source pixels scaled horizontally by 2, over opaque black.
        for (offset, value) in [
            (0x10, 8),
            (0x14, 0x10004),
            (0x18, 0x10002),
            (0x1c, 5),
            (0x20, 0x10002),
            (0x24, 0x10001),
            (0x44, 16),
            (0x4c, 16),
            (0x50, 0x10002),
            (0x54, 0x10002),
            (0x5c, 0x20004),
            (0x60, 0x8000800),
            (8, 0x11000),
            (4, 1),
        ] {
            write(&mut d, &mut h, 0xdeadbeef, offset, value);
        }
        (d, h)
    }
    #[test]
    fn dma_capture_is_phase_driven_scaled_alpha_composited_and_observational() {
        let (mut d, mut h) = picture();
        for at in [0, 31, 32, 79] {
            h.now = at;
            d.advance(&mut Cx::new(&mut h, 2, 0x1234, 0));
            assert!(h.images.is_empty());
        }
        h.now = 80;
        d.advance(&mut Cx::new(&mut h, 0, 0, 0));
        let frame = &h.images[0];
        assert_eq!(
            (frame.output, frame.sequence, frame.width, frame.height),
            (1, 1, 4, 2)
        );
        assert_eq!(
            frame.rgba,
            [10, 20, 30, 255, 10, 20, 30, 255, 30, 40, 50, 255, 30, 40, 50, 255].repeat(2)
        );
        let saved = d.save();
        for _ in 0..100 {
            assert_eq!(d.save(), saved);
        }
        h.now = 160;
        d.advance(&mut Cx::new(&mut h, 0, 0, 0));
        assert_eq!(h.images[1], h.images[0]);
        let mut restored = Lcdc::new(
            0xfffa9000,
            2,
            [117, 118],
            Some(Picture::Bgra8888Straight),
            None,
        )
        .unwrap();
        restored.restore(&d.save()).unwrap();
        h.now = 240;
        restored.advance(&mut Cx::new(&mut h, 0, 0, 0));
        assert_eq!(h.images[2], h.images[0]); // Republish even with a fresh host.
        h.ram[16] = 44;
        h.now = 320;
        restored.advance(&mut Cx::new(&mut h, 0, 0, 0));
        assert_eq!(h.images[3].sequence, 2);
        write(&mut restored, &mut h, 0, 4, 0);
        assert_eq!(h.outputs.last().unwrap()["active"], false);
    }
    #[test]
    fn unknown_layers_and_picture_modes_refuse() {
        for (offset, value) in [(0x50, 0x20002), (0x4c, 32), (8, 0x12000), (0x54, 0)] {
            let (mut d, mut h) = picture();
            write(&mut d, &mut h, 0, offset, value);
            h.now = 80;
            let mut cx = Cx::new(&mut h, 0, 0, 0);
            d.advance(&mut cx);
            assert!(cx.refusal().is_some(), "offset {offset:x}");
            assert!(h.images.is_empty());
        }
    }
    #[test]
    fn packed_video_reads_pixel_pitch_at_the_phase_and_composes_below_raw_osd() {
        let (mut d, mut h) = picture();
        d.video = Some(Video::YyCbCr422);
        h.ram.resize(128, 0xaa);
        h.ram[64..72].copy_from_slice(&[16, 235, 128, 128, 81, 145, 128, 128]);
        h.ram[80..88].copy_from_slice(&[235, 16, 128, 128, 145, 81, 128, 128]);
        for (offset, value) in [
            (0x168, 64),
            (0x16c, 2), // 8 pixels / 16 bytes per row, including padding.
            (0x170, 0x20004),
            (0x178, 0x20004),
            (0x17c, 0x10001000),
            (8, 0x10010),
        ] {
            write(&mut d, &mut h, 0x1234, offset, value);
        }
        let before = d.save();
        h.now = 79;
        d.advance(&mut Cx::new(&mut h, 3, 0x9876, 0));
        assert!(h.images.is_empty());
        assert_eq!(d.save()["sequence"], before["sequence"]);
        h.now = 80;
        d.advance(&mut Cx::new(&mut h, 0, 0, 0));
        let gray = |v| [v, v, v, 255];
        assert_eq!(
            h.images[0].rgba,
            [0, 255, 76, 150, 255, 0, 150, 76]
                .into_iter()
                .flat_map(gray)
                .collect::<Vec<_>>()
        );
        assert_eq!(h.outputs.last().unwrap()["format"], "yycbcr422");
        assert_eq!(h.outputs.last().unwrap()["source"], "0x40");
        // Same DMA phase, with the existing straight-alpha raw plane above video.
        write(&mut d, &mut h, 0, 8, 0x11010);
        h.now = 160;
        d.advance(&mut Cx::new(&mut h, 1, 0xabcdef, 0));
        assert_eq!(&h.images[1].rgba[..8], &[10, 20, 30, 255, 10, 20, 30, 255]);
        assert_eq!(&h.images[1].rgba[8..12], &[68, 78, 88, 255]);
        assert_eq!(d.next_deadline(h.now), Some(192));
        // An unsupported packing bit must fail without publishing an image.
        write(&mut d, &mut h, 0, 0x16c, 0x10002);
        h.now = 240;
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        d.advance(&mut cx);
        assert!(cx.refusal().unwrap().contains("packing"));
        assert_eq!(h.images.len(), 2);
    }
    #[test]
    fn unsupported_timing_refuses_before_output_enable() {
        let mut d = Lcdc::new(0xfff7a000, 2, [112, 113], None, None).unwrap();
        let mut h = Clock::default();
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        d.write(&mut cx, 0xfff7a004, 4, 1);
        assert!(cx.refusal().is_some());
        assert_eq!(d.next_deadline(0), None);
        assert_eq!(d.r(4), 0);
    }
}
