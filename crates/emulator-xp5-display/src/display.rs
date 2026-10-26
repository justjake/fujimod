//! Two XP5 LCDC register banks. Guest pixel clocks and timing counts drive
//! physical phase interrupts and DMA scanout; host publication is separate.
//! Register behavior and sampling limits are explicit in the scanout model.
use crate::osd;
use fujimod_emulator_devices::xp5::panel_clocks::Clocks;
use fujimod_emulator_graphics::raster::{self, PixelOrder, Plane};
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest;

#[derive(Clone, Serialize, Deserialize)]
struct State {
    regs: Vec<u32>,
    enabled: bool,
    generation: u64,
    phase: usize,
    deadline: Option<u64>,
    frames: u64,
    last_hash: String,
}
#[derive(Debug)]
struct Timing {
    width: u32,
    height: u32,
    #[cfg(test)]
    htotal: u32,
    #[cfg(test)]
    vtotal: u32,
    period_ns: f64,
    delays_ns: [f64; 3],
}
#[derive(Default)]
pub struct Config {
    /// Explicit timing experiment override; otherwise use programmed clocks.
    pub period_ns: Option<u64>,
    /// Diagnostic transport overrides; converter transport is the default.
    pub raw_formats: [Option<PixelOrder>; 4],
}
pub struct Display {
    bank: usize,
    state: State,
    period_ns: Option<u64>,
    clocks: Clocks,
    formats: [Option<PixelOrder>; 4],
    needs_publication: bool,
    blank: Option<(u32, u32)>,
    input: Option<(u32, u32, Vec<raster::Snapshot>)>,
}
impl Display {
    pub fn new(base: u64, clocks: Clocks, config: Config) -> Result<Self, String> {
        let bank = match base {
            0xff990000 => 0,
            0xff9a0000 => 1,
            _ => return Err("display base must be ff990000 or ff9a0000".into()),
        };
        if config
            .period_ns
            .is_some_and(|n| !(1_000_000..=1_000_000_000).contains(&n))
        {
            return Err("display period must be 1ms..1s".into());
        }
        Ok(Self {
            bank,
            state: State {
                regs: vec![0; 0x7d4 / 4],
                enabled: false,
                generation: 0,
                phase: 0,
                deadline: None,
                frames: 0,
                last_hash: String::new(),
            },
            period_ns: config.period_ns,
            formats: config.raw_formats,
            clocks,
            needs_publication: true,
            blank: None,
            input: None,
        })
    }
    fn base(&self) -> u64 {
        0xff990000 + self.bank as u64 * 0x10000
    }
    fn r(&self, o: u64) -> u32 {
        self.state.regs[o as usize / 4]
    }
    fn status(&self, cx: &mut Cx<'_>) {
        let vo = if self.bank == 0 { 0x20 } else { 0x24 };
        let h = self.r(0x14);
        let v = self.r(vo);
        let hp = self.r(0x18);
        let vp = self.r(vo + 4);
        let enables = self.r(8);
        let value = json!({
            "bank": self.bank, "enabled": self.state.enabled,
            "width": h & 0x3fff, "height": v & 0x1fff,
            "total_width": (h & 0x3fff) + ((h >> 16) & 0x1ff) + (hp & 0xfff) + ((hp >> 16) & 0xfff),
            "total_height": (v & 0x1fff) + ((v >> 16) & 0x1ff) + (vp & 0xfff) + ((vp >> 16) & 0xfff),
            "osd": [enables & (1 << 20) != 0, enables & (1 << 16) != 0],
            "picture": ([12, 8, 4, 0].map(|bit| enables & (1 << bit) != 0)),
            "sequence": self.state.frames,
        });
        if let Err(e) = cx.output_status(self.bank, value) {
            cx.fail(e);
        }
    }
    fn timing(&self) -> Result<Timing, String> {
        let h = self.r(0x14);
        let hp = self.r(0x18);
        let vo = if self.bank == 0 { 0x20 } else { 0x24 };
        let v = self.r(vo);
        let vp = self.r(vo + 4);
        let width = h & 0x3fff;
        let height = v & 0x1fff;
        let hsync = (h >> 16) & 0x1ff;
        let vsync = (v >> 16) & 0x1ff;
        let hfront = hp & 0xfff;
        let hback = (hp >> 16) & 0xfff;
        let vfront = vp & 0xfff;
        let vback = (vp >> 16) & 0xfff;
        let ht = width + hsync + hfront + hback;
        let vt = height + vsync + vfront + vback;
        if width == 0 || height == 0 || ht == 0 || vt == 0 {
            return Err("panel active area/timing not programmed".into());
        }
        let hz = self.clocks.pixel_hz(self.bank)?;
        let period = self
            .period_ns
            .map_or(ht as f64 * vt as f64 * 1e9 / hz, |n| n as f64);
        if !(1_000_000.0..=1_000_000_000.0).contains(&period) {
            return Err(format!("unsupported panel period {period}ns"));
        }
        let per_line = period / vt as f64;
        Ok(Timing {
            width,
            height,
            #[cfg(test)]
            htotal: ht,
            #[cfg(test)]
            vtotal: vt,
            period_ns: period,
            delays_ns: [
                (vsync + vback) as f64 * per_line,
                height as f64 * per_line,
                vfront as f64 * per_line,
            ],
        })
    }
    fn scanout(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let enables = self.r(8);
        // Bit24 through context+95b is output control,
        // separate from the six plane-enable bits below.
        if enables & !0x01111111 != 0 {
            return Err("unresolved display plane-enable bits".into());
        }
        // Commit packs OSD0/1 into bits20/16; picture groups use12/8/4/0.
        let mut planes = Vec::new();
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
            let raw_order = self.formats[format];
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
                let header = cx.ram_bytes(secondary, osd::HEADER_BYTES)?;
                // Newly allocated planes can be enabled before the first
                // converter completion. They have no published image yet.
                if header.iter().all(|b| *b == 0) {
                    continue;
                }
                if ctrl & 1 == 0 {
                    return Err("OSD private plane requires converter control bit0".into());
                }
                let publication = osd::decode(&header)?;
                if publication.primary != address {
                    // Register writes may select the new primary before its
                    // secondary. A valid publication for the previous pair
                    // does not publish pixels for this pair.
                    cx.log(&format!("display bank{} unpublished OSD pair primary={address:#x} secondary={secondary:#x} published_primary={:#x}",self.bank,publication.primary));
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
        let work_width = planes
            .iter()
            .map(|p| p.work_x + p.work_width)
            .max()
            .unwrap_or(0);
        let work_height = planes
            .iter()
            .map(|p| p.work_y + p.work_height)
            .max()
            .unwrap_or(0);
        let (width, height) = self
            .timing()
            .map(|t| (t.width, t.height))
            .unwrap_or((work_width, work_height));
        if width == 0 || height == 0 {
            return Ok(());
        }
        if width > 8192 || height > 8192 {
            return Err("scanout canvas exceeds8192".into());
        }
        let blank = planes.is_empty();
        if blank && self.blank == Some((width, height)) {
            return Ok(());
        }
        if !blank {
            self.blank = None;
        }
        let captured = planes
            .into_iter()
            .map(|p| raster::capture(|a, n| cx.ram_bytes(a, n), p, width, height))
            .collect::<Result<Vec<_>, _>>()?;
        if self
            .input
            .as_ref()
            .is_some_and(|(w, h, old)| (*w, *h) == (width, height) && *old == captured)
        {
            return Ok(());
        }
        let mut rgba = vec![0; width as usize * height as usize * 4];
        for p in rgba.chunks_exact_mut(4) {
            p[3] = 255;
        }
        for p in &captured {
            raster::composite_snapshot(p, width, height, &mut rgba)?;
        }
        self.input = Some((width, height, captured));
        let hash = format!("{:x}", sha2::Sha256::digest(&rgba));
        let hash = format!("{width}x{height}:{hash}");
        let changed = hash != self.state.last_hash;
        if changed {
            self.state.frames = self
                .state
                .frames
                .checked_add(1)
                .ok_or("scanout sequence overflow")?;
            self.state.last_hash = hash;
        } else if !self.needs_publication {
            if blank {
                self.blank = Some((width, height));
            }
            return Ok(());
        }
        cx.scanout(fujimod_emulator_devices::graphics::scanout::Frame {
            output: self.bank,
            sequence: self.state.frames,
            width,
            height,
            rgba,
        })?;
        self.needs_publication = false;
        if changed {
            cx.log(&format!(
                "display bank{} frame{} {}x{}",
                self.bank, self.state.frames, width, height
            ));
        }
        if blank {
            self.blank = Some((width, height));
        }
        Ok(())
    }
}
impl Device for Display {
    fn name(&self) -> &'static str {
        "lcdc"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        self.advance(cx);
        let o = address.wrapping_sub(self.base());
        if n != 4 || o & 3 != 0 || !register(self.bank, o) {
            cx.unmodeled();
            return 0;
        }
        if o == 0 { 1 } else { u64::from(self.r(o)) }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, value: u64) {
        self.advance(cx);
        if cx.refusal().is_some() {
            return;
        }
        let o = address.wrapping_sub(self.base());
        let v = value as u32;
        if n != 4 || o & 3 != 0 || !register(self.bank, o) {
            cx.unmodeled();
            return;
        }
        if o == 0 {
            if v != 0 {
                cx.fail("unsupported display reset value");
                return;
            }
            // Reset occurs after timing and output enable. The handshake retains
            // scanout latches and phase IRQ operation.
            self.status(cx);
            return;
        }
        if o == 4 {
            if v > 1 {
                cx.fail("unsupported display enable value");
                return;
            }
            self.state.regs[1] = v;
            if self.state.enabled != (v == 1) {
                self.state.enabled = v == 1;
                self.state.generation = self.state.generation.wrapping_add(1);
                self.state.phase = 0;
                self.state.deadline = None;
                if self.state.enabled {
                    match self
                        .timing()
                        .map(|t| t.period_ns)
                        .or_else(|e| self.period_ns.map(|n| n as f64).ok_or(e))
                    {
                        Ok(period) => self.schedule(cx, cx.now(), period),
                        Err(e) => cx.fail(e),
                    }
                }
            }
            self.status(cx);
            return;
        }
        self.state.regs[o as usize / 4] = v;
        if matches!(o, 8 | 0x14 | 0x18 | 0x20 | 0x24 | 0x28) {
            self.status(cx);
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token == self.state.generation {
            self.advance(cx);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        while let Some(at) = self.state.deadline.filter(|at| *at <= cx.now()) {
            if !self.state.enabled {
                self.state.deadline = None;
                return;
            }
            // Category2 invokes the native commit. Scanout at the following
            // phase observes the registers published during sync/back porch.
            let category = [2, 0, 1][self.state.phase];
            if self.state.phase == 1 {
                if let Err(e) = self.scanout(cx) {
                    cx.fail(e);
                    return;
                }
                self.status(cx);
                if cx.refusal().is_some() {
                    return;
                }
            }
            let irq = [[177, 178, 183], [186, 187, 192]][self.bank][category];
            cx.set_irq(irq, true);
            cx.set_irq(irq, false);
            self.state.phase = (self.state.phase + 1) % 3;
            let delay = match self.timing() {
                Ok(t) => {
                    t.delays_ns[if self.state.phase == 0 {
                        2
                    } else {
                        self.state.phase - 1
                    }]
                }
                Err(_) if self.period_ns.is_some() => self.period_ns.unwrap() as f64 / 3.0,
                Err(e) => {
                    cx.fail(e);
                    return;
                }
            };
            self.schedule(cx, at, delay);
            if cx.refusal().is_some() {
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
        let state: State = serde_json::from_value(value.clone())
            .map_err(|e| format!("display checkpoint: {e}"))?;
        if state.regs.len() != 0x7d4 / 4
            || state.phase >= 3
            || state.enabled != state.deadline.is_some()
        {
            return Err("invalid display checkpoint register/phase state".into());
        }
        self.state = state;
        // Republish the same deterministic sequence into a new host run;
        // publication is not a new modeled image change.
        self.needs_publication = true;
        self.blank = None;
        self.input = None;
        Ok(())
    }
}
impl Display {
    fn schedule(&mut self, cx: &mut Cx<'_>, from: u64, delay: f64) {
        if let Some(at) = from.checked_add(delay.ceil().max(1.0) as u64) {
            self.state.deadline = Some(at);
            cx.schedule(at, self.state.generation);
        } else {
            cx.fail("display phase deadline overflow");
        }
    }
}

// Context snapshot, commit, and output setup registers. Larger
// windows and reserved holes stay unmapped. Coefficient/control words retain
// guest values; reset state0 is an explicit unmeasured model assumption.
fn register(bank: usize, o: u64) -> bool {
    let list = if bank == 0 { BANK0 } else { BANK1 };
    matches!(
        o,
        0 | 4 | 8 | 0x10 | 0x14 | 0x18 | 0x1c | 0x20 | 0x24 | 0x28 | 0x34 | 0x38
    ) || (bank == 1 && matches!(o, 0x2c | 0x3c | 0x4c))
        || list.binary_search(&o).is_ok()
}

const BANK0: &[u64] = &[
    0x0, 0x4, 0x8, 0x14, 0x20, 0x44, 0x48, 0x4c, 0x50, 0x54, 0x58, 0x5c, 0x60, 0x64, 0x68, 0x6c,
    0x70, 0x74, 0x78, 0x7c, 0x80, 0x84, 0x88, 0x8c, 0x90, 0x94, 0x98, 0x9c, 0xa0, 0xa4, 0xa8, 0xac,
    0xb0, 0xb4, 0xb8, 0xbc, 0xc0, 0xc4, 0xc8, 0xcc, 0xd0, 0xd4, 0xd8, 0xdc, 0xe0, 0xe4, 0xe8, 0xec,
    0xf0, 0xf4, 0xf8, 0xfc, 0x100, 0x104, 0x108, 0x10c, 0x110, 0x114, 0x118, 0x11c, 0x120, 0x124,
    0x128, 0x12c, 0x130, 0x134, 0x138, 0x13c, 0x140, 0x144, 0x148, 0x14c, 0x150, 0x154, 0x158,
    0x15c, 0x160, 0x164, 0x168, 0x16c, 0x170, 0x174, 0x178, 0x17c, 0x180, 0x184, 0x188, 0x18c,
    0x190, 0x194, 0x198, 0x19c, 0x1a0, 0x1a4, 0x1a8, 0x1ac, 0x1b0, 0x1b4, 0x1b8, 0x1bc, 0x1c0,
    0x1c4, 0x1c8, 0x1cc, 0x1d0, 0x1d4, 0x1d8, 0x1e0, 0x1e4, 0x1e8, 0x1ec, 0x1f0, 0x1f4, 0x1f8,
    0x1fc, 0x200, 0x204, 0x208, 0x20c, 0x210, 0x214, 0x218, 0x21c, 0x220, 0x224, 0x228, 0x22c,
    0x230, 0x234, 0x238, 0x23c, 0x240, 0x244, 0x248, 0x24c, 0x250, 0x254, 0x258, 0x25c, 0x260,
    0x264, 0x268, 0x26c, 0x270, 0x274, 0x278, 0x27c, 0x280, 0x284, 0x288, 0x28c, 0x290, 0x294,
    0x298, 0x29c, 0x2a0, 0x2a4, 0x2a8, 0x2ac, 0x2b0, 0x2b4, 0x2b8, 0x2bc, 0x2c0, 0x2c4, 0x2c8,
    0x2cc, 0x2d0, 0x2d4, 0x2d8, 0x2dc, 0x2e0, 0x2e4, 0x2e8, 0x2ec, 0x2f0, 0x2f4, 0x2f8, 0x2fc,
    0x300, 0x304, 0x308, 0x30c, 0x310, 0x314, 0x318, 0x31c, 0x320, 0x324, 0x328, 0x32c, 0x330,
    0x334, 0x338, 0x33c, 0x340, 0x344, 0x348, 0x34c, 0x350, 0x354, 0x358, 0x35c, 0x360, 0x364,
    0x368, 0x370, 0x374, 0x378, 0x37c, 0x380, 0x384, 0x388, 0x38c, 0x390, 0x394, 0x398, 0x39c,
    0x3a0, 0x3a4, 0x3a8, 0x3ac, 0x3b0, 0x3b4, 0x3c0, 0x3c4, 0x3c8, 0x3cc, 0x3d0, 0x3d4, 0x3d8,
    0x3dc, 0x3e0, 0x3e4, 0x3e8, 0x3ec, 0x3f0, 0x3f4, 0x3f8, 0x3fc, 0x400, 0x410, 0x414, 0x418,
    0x41c, 0x420, 0x424, 0x428, 0x42c, 0x430, 0x434, 0x438, 0x43c, 0x440, 0x444, 0x448, 0x44c,
    0x450, 0x460, 0x464, 0x468, 0x46c, 0x470, 0x474, 0x478, 0x47c, 0x480, 0x484, 0x488, 0x48c,
    0x490, 0x494, 0x498, 0x49c, 0x4a0, 0x4a4, 0x4a8, 0x4ac, 0x4c0, 0x4c4, 0x4c8, 0x4cc, 0x4d0,
    0x4d4, 0x4d8, 0x4dc, 0x4e0, 0x4e4, 0x4e8, 0x4ec, 0x4f0, 0x4f4, 0x4f8, 0x4fc, 0x500, 0x504,
    0x508, 0x50c, 0x510, 0x514, 0x518, 0x51c, 0x520, 0x524, 0x528, 0x52c, 0x530, 0x534, 0x538,
    0x53c, 0x540, 0x544, 0x548, 0x54c, 0x550, 0x554, 0x558, 0x55c, 0x560, 0x564, 0x568, 0x56c,
    0x570, 0x574, 0x578, 0x57c, 0x580, 0x584, 0x588, 0x58c, 0x590, 0x594, 0x598, 0x59c, 0x5a0,
    0x5a4, 0x5a8, 0x5ac, 0x5b0, 0x5b4, 0x5b8, 0x5bc, 0x5c0, 0x5c4, 0x5c8, 0x5cc, 0x5d0, 0x5d4,
    0x5d8, 0x5dc, 0x5e0, 0x5e4, 0x5e8, 0x5ec, 0x5f0, 0x5f4, 0x5f8, 0x5fc, 0x600, 0x604, 0x608,
    0x60c, 0x610, 0x614, 0x620, 0x624, 0x628, 0x62c, 0x630, 0x634, 0x638, 0x63c, 0x640, 0x644,
    0x648, 0x64c, 0x650, 0x654, 0x658, 0x65c, 0x660, 0x664, 0x668, 0x66c, 0x670, 0x674, 0x678,
    0x67c, 0x680, 0x684, 0x688, 0x68c, 0x690, 0x694, 0x698, 0x69c, 0x6a0, 0x6a4, 0x6a8, 0x6ac,
    0x6b0, 0x6b4, 0x6b8, 0x6bc, 0x6c0, 0x6c4, 0x6c8, 0x6cc, 0x6d0, 0x6d4, 0x6d8, 0x6dc, 0x6e0,
    0x6e4, 0x6e8, 0x6ec, 0x6f0, 0x6f4, 0x6f8, 0x6fc, 0x700, 0x704, 0x708, 0x70c, 0x710, 0x714,
    0x718, 0x71c, 0x720, 0x724, 0x728, 0x72c, 0x730, 0x734, 0x738, 0x73c, 0x740, 0x744, 0x748,
    0x74c, 0x750, 0x754, 0x758, 0x75c, 0x760, 0x764, 0x768, 0x76c, 0x770, 0x774, 0x778, 0x77c,
    0x780, 0x784, 0x788, 0x78c, 0x790, 0x794, 0x7b8, 0x7c0, 0x7c8, 0x7d0,
];

const BANK1: &[u64] = &[
    0x0, 0x4, 0x8, 0x14, 0x24, 0x48, 0x50, 0x54, 0x58, 0x5c, 0x60, 0x64, 0x68, 0x6c, 0x70, 0x74,
    0x78, 0x7c, 0x80, 0x84, 0x88, 0x8c, 0x90, 0x94, 0x98, 0x9c, 0xa0, 0xa4, 0xa8, 0xac, 0xb0, 0xb4,
    0xb8, 0xbc, 0xc0, 0xc4, 0xc8, 0xcc, 0xd0, 0xd4, 0xd8, 0xdc, 0xe0, 0xe4, 0xe8, 0xec, 0xf0, 0xf4,
    0xf8, 0xfc, 0x100, 0x104, 0x108, 0x10c, 0x110, 0x114, 0x118, 0x11c, 0x120, 0x124, 0x128, 0x12c,
    0x130, 0x134, 0x138, 0x13c, 0x140, 0x144, 0x148, 0x14c, 0x150, 0x154, 0x158, 0x15c, 0x160,
    0x164, 0x168, 0x16c, 0x170, 0x174, 0x178, 0x17c, 0x180, 0x184, 0x188, 0x18c, 0x190, 0x194,
    0x198, 0x19c, 0x1a0, 0x1a4, 0x1a8, 0x1ac, 0x1b0, 0x1b4, 0x1b8, 0x1bc, 0x1c0, 0x1c4, 0x1c8,
    0x1cc, 0x1d0, 0x1d4, 0x1d8, 0x1dc, 0x1e0, 0x1e4, 0x1e8, 0x1ec, 0x1f0, 0x1f4, 0x1f8, 0x1fc,
    0x200, 0x204, 0x208, 0x20c, 0x210, 0x214, 0x218, 0x21c, 0x220, 0x224, 0x228, 0x22c, 0x230,
    0x234, 0x238, 0x23c, 0x240, 0x244, 0x248, 0x24c, 0x250, 0x254, 0x258, 0x25c, 0x260, 0x264,
    0x268, 0x26c, 0x270, 0x274, 0x278, 0x27c, 0x280, 0x284, 0x288, 0x28c, 0x290, 0x294, 0x298,
    0x29c, 0x2a0, 0x2a4, 0x2a8, 0x2ac, 0x2b0, 0x2b4, 0x2b8, 0x2bc, 0x2c0, 0x2c4, 0x2c8, 0x2cc,
    0x2d0, 0x2d4, 0x2d8, 0x2dc, 0x2e0, 0x2e4, 0x2e8, 0x2ec, 0x2f0, 0x2f4, 0x2f8, 0x2fc, 0x300,
    0x304, 0x308, 0x30c, 0x310, 0x314, 0x318, 0x31c, 0x320, 0x324, 0x328, 0x32c, 0x330, 0x334,
    0x338, 0x33c, 0x340, 0x344, 0x348, 0x34c, 0x350, 0x354, 0x358, 0x35c, 0x360, 0x364, 0x368,
    0x36c, 0x370, 0x374, 0x378, 0x37c, 0x380, 0x384, 0x388, 0x38c, 0x390, 0x394, 0x398, 0x39c,
    0x3a0, 0x3a4, 0x3a8, 0x3ac, 0x3b0, 0x3b4, 0x3b8, 0x3bc, 0x3c0, 0x3c4, 0x3c8, 0x3cc, 0x3d0,
    0x3d4, 0x3d8, 0x3dc, 0x3e0, 0x3e4, 0x3e8, 0x3ec, 0x3f0, 0x3f4, 0x3f8, 0x3fc, 0x400, 0x404,
    0x408, 0x40c, 0x410, 0x414, 0x418, 0x41c, 0x420, 0x424, 0x428, 0x42c, 0x430, 0x434, 0x438,
    0x43c, 0x440, 0x444, 0x448, 0x44c, 0x450, 0x454, 0x458, 0x45c, 0x460, 0x464, 0x468, 0x46c,
    0x470, 0x474, 0x478, 0x47c, 0x480, 0x484, 0x488, 0x48c, 0x490, 0x494, 0x498, 0x49c, 0x4a0,
    0x4a4, 0x4a8, 0x4ac, 0x4b0, 0x4b4, 0x4b8, 0x4bc, 0x4c0, 0x4c4, 0x4c8, 0x4cc, 0x500, 0x504,
    0x508, 0x50c, 0x510, 0x514, 0x518, 0x51c, 0x520, 0x524, 0x528, 0x52c, 0x530, 0x534, 0x538,
    0x53c, 0x540, 0x544, 0x548, 0x54c, 0x550, 0x554, 0x558, 0x55c, 0x560, 0x564, 0x568, 0x56c,
    0x570, 0x574, 0x578, 0x57c, 0x580, 0x584, 0x588, 0x58c, 0x590, 0x594, 0x598, 0x59c, 0x5a0,
    0x5a4, 0x5a8, 0x5ac, 0x5b0, 0x5b4, 0x5b8, 0x5bc, 0x5c0, 0x5c4, 0x5c8, 0x5cc, 0x5d0, 0x5d4,
    0x5d8, 0x5dc, 0x5e0, 0x5e4, 0x5ec, 0x5f0, 0x5f4, 0x5f8, 0x5fc, 0x600, 0x604, 0x608, 0x60c,
    0x610, 0x614, 0x618, 0x61c, 0x620, 0x624, 0x628, 0x62c, 0x654, 0x658, 0x65c, 0x660, 0x664,
    0x668, 0x66c, 0x670, 0x674, 0x678, 0x67c, 0x680, 0x684, 0x688, 0x68c, 0x690, 0x694, 0x698,
    0x69c, 0x6a0, 0x6a4, 0x6a8, 0x6ac, 0x6b0, 0x6b4, 0x6b8, 0x6bc, 0x6c0, 0x6c4, 0x6c8, 0x6cc,
    0x6d0, 0x6d4, 0x6d8, 0x6dc, 0x6e0, 0x6e4, 0x6e8, 0x6ec, 0x6f0, 0x6f4, 0x6f8, 0x6fc, 0x700,
    0x704, 0x708, 0x70c, 0x710, 0x714, 0x718, 0x71c, 0x720, 0x724, 0x728, 0x72c, 0x730, 0x734,
    0x738, 0x73c, 0x740, 0x744, 0x748, 0x74c, 0x750, 0x754, 0x758, 0x75c, 0x760, 0x764, 0x768,
    0x76c, 0x770, 0x774, 0x778, 0x77c, 0x780, 0x784, 0x788, 0x78c, 0x7b8, 0x7c0, 0x7c8, 0x7d0,
];

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::Host;
    use fujimod_emulator_devices::graphics::scanout::Frame;
    #[derive(Default)]
    struct OutputHost {
        now: u64,
        ram: Vec<u8>,
        irqs: Vec<(u32, bool)>,
        events: Vec<(u64, u64)>,
        frames: Vec<Frame>,
        logs: Vec<String>,
    }
    impl Host for OutputHost {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, _: usize, at: u64, token: u64) {
            self.events.push((at, token));
        }
        fn set_irq(&mut self, line: u32, level: bool) {
            self.irqs.push((line, level));
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            Err("no per-core IRQ".into())
        }
        fn ram_read(&self, address: u64, out: &mut [u8]) -> Result<(), String> {
            let at = address.checked_sub(1 << 32).ok_or("wrong DMA high bits")? as usize;
            out.copy_from_slice(self.ram.get(at..at + out.len()).ok_or("DMA bounds")?);
            Ok(())
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            Err("scanout must be read-only".into())
        }
        fn record(&mut self, _: fujimod_emulator_devices::Record) {}
        fn log(&mut self, line: &str) {
            self.logs.push(line.into());
        }
        fn scanout(&mut self, frame: Frame) -> Result<(), String> {
            self.frames.push(frame);
            Ok(())
        }
    }
    fn write(d: &mut dyn Device, h: &mut OutputHost, address: u64, value: u64) {
        let mut cx = Cx::new(h, 0, 0xdead, 0);
        d.write(&mut cx, address, 4, value);
        assert_eq!(cx.refusal(), None);
    }
    fn due(d: &mut Display, h: &mut OutputHost) {
        h.now = d.next_deadline(h.now).unwrap();
        let token = d.state.generation;
        let mut cx = Cx::new(h, 0, 0, 0);
        d.event(&mut cx, token);
        assert_eq!(cx.refusal(), None);
    }
    fn setup(bank: usize) -> (Display, OutputHost) {
        let mut h = OutputHost {
            ram: vec![0; 256],
            ..Default::default()
        };
        for p in h.ram.chunks_exact_mut(4) {
            p.copy_from_slice(&[255, 0, 255, 255]);
        }
        h.ram[36..44].copy_from_slice(&[0, 0, 255, 255, 0, 255, 0, 255]);
        h.ram[68..76].copy_from_slice(&[255, 0, 0, 255, 255, 255, 255, 255]);
        let mut d = Display::new(
            0xff990000 + bank as u64 * 0x10000,
            Clocks::new(),
            Config {
                period_ns: Some(30_000_000),
                raw_formats: [Some(PixelOrder::Bgra), None, None, None],
            },
        )
        .unwrap();
        let base = d.base();
        let a = if bank == 0 { 0x50 } else { 0x58 };
        for (o, v) in [
            (0, 0),
            (8, 1 << 20),
            (a, 1),
            (a + 4, 128),
            (a + 0x14, 32),
            (a + 0x18, 0x10001),
            (a + 0x1c, 0),
            (a + 0x20, 0x20002),
            (a + 0x24, 0x20002),
            (4, 1),
        ] {
            write(&mut d, &mut h, base + o, v);
        }
        (d, h)
    }
    #[test]
    fn programmed_panel_timings_and_pll_cadence() {
        let clocks = Clocks::new();
        let mut h = OutputHost::default();
        for (base, n, d) in [(0xffa13000, 99, 0x10f), (0xffa14000, 1, 0x100)] {
            let mut pll = crate::panel_clock::PanelPll::new(base, clocks.clone()).unwrap();
            write(&mut pll, &mut h, base, n);
            write(&mut pll, &mut h, base + 0x40, d);
        }
        assert_eq!(clocks.pixel_hz(0).unwrap(), 148.5e6);
        for (bank, n, divider, w, hgt, hs, hf, hb, vs, vf, vb, ht, vt, period) in [
            (
                1,
                96,
                29,
                900,
                600,
                3,
                100,
                37,
                2,
                10,
                4,
                1040,
                616,
                16_683_333.333,
            ),
            (
                1,
                99,
                8,
                1280,
                960,
                16,
                508,
                16,
                6,
                229,
                15,
                1820,
                1210,
                16_683_333.333,
            ),
            (
                0,
                99,
                8,
                1920,
                1080,
                44,
                88,
                148,
                5,
                4,
                36,
                2200,
                1125,
                16_666_666.667,
            ),
        ] {
            let mut pll =
                crate::panel_clock::PanelPll::new(0xffa11000, clocks.clone()).unwrap();
            write(&mut pll, &mut h, 0xffa11000, n);
            write(&mut pll, &mut h, 0xffa11040, divider);
            let mut d = Display::new(
                0xff990000 + bank * 0x10000,
                clocks.clone(),
                Config::default(),
            )
            .unwrap();
            let base = d.base();
            let vo = if bank == 0 { 0x20 } else { 0x24 };
            for (o, v) in [
                (0x14, w | hs << 16),
                (0x18, hf | hb << 16),
                (vo, hgt | vs << 16),
                (vo + 4, vf | vb << 16),
            ] {
                write(&mut d, &mut h, base + o, v);
            }
            let t = d.timing().unwrap();
            assert_eq!(
                (t.width, t.height, t.htotal, t.vtotal),
                (w as u32, hgt as u32, ht, vt)
            );
            assert!((t.period_ns - period).abs() < 10.0);
            assert!((t.delays_ns.iter().sum::<f64>() - t.period_ns).abs() < 0.001);
        }
        clocks.selector(1);
        assert_eq!(clocks.pixel_hz(0).unwrap(), 297e6);
    }
    #[test]
    fn dma_origin_padding_irq_early_service_and_restore_publication() {
        for bank in 0..2 {
            let (mut d, mut h) = setup(bank);
            let state = d.save();
            let token = d.state.generation;
            d.event(&mut Cx::new(&mut h, 0, 0, 0), token);
            assert_eq!(d.save(), state);
            assert!(h.irqs.is_empty());
            due(&mut d, &mut h);
            // Native category2 callback publishes a new buffer before scanout.
            let a = if bank == 0 { 0x50 } else { 0x58 };
            let base = d.base();
            write(&mut d, &mut h, base + a + 4, 0);
            due(&mut d, &mut h);
            due(&mut d, &mut h);
            assert_eq!(h.frames.len(), 1);
            assert_eq!(h.logs, [format!("display bank{bank} frame1 2x2")]);
            assert_eq!(
                h.frames[0].rgba,
                [
                    255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255
                ]
            );
            assert_eq!(
                (h.frames[0].width, h.frames[0].height, h.frames[0].sequence),
                (2, 2, 1)
            );
            let lines = if bank == 0 {
                [183, 177, 178]
            } else {
                [192, 186, 187]
            };
            assert_eq!(
                h.irqs,
                lines
                    .into_iter()
                    .flat_map(|n| [(n, true), (n, false)])
                    .collect::<Vec<_>>()
            );
            assert_eq!(d.next_deadline(h.now), Some(60_000_000));
            for _ in 0..3 {
                due(&mut d, &mut h);
            }
            assert_eq!(h.frames.len(), 1);
            // A fresh host receives the restored image with the same model
            // sequence, irrespective of the old host's archival files.
            let state = d.save();
            d.restore(&state).unwrap();
            assert_eq!(d.save(), state);
            h.frames.clear();
            for _ in 0..3 {
                due(&mut d, &mut h);
            }
            assert_eq!(h.frames.len(), 1);
            assert_eq!(h.frames[0].sequence, 1);
            assert_eq!(h.logs.len(), 1);
            let base = d.base();
            let mut cx = Cx::new(&mut h, 0, 0, 0);
            d.read(&mut cx, base + 0xc, 4);
            assert!(cx.is_unmodeled());
            let mut cx = Cx::new(&mut h, 0, 0, 0);
            d.read(&mut cx, base + 0x58, 8);
            assert!(cx.is_unmodeled());
        }
    }
    #[test]
    fn packed_picture_uses_dma_crop_and_rejects_unknown_phase() {
        let (mut d, mut h) = setup(1);
        let base = d.base();
        let a = 0x370;
        h.ram[128..136].copy_from_slice(&[16, 235, 128, 128, 16, 235, 128, 128]);
        h.ram[144..152].copy_from_slice(&[235, 16, 128, 128, 235, 16, 128, 128]);
        for (o, v) in [
            (8, 1 << 12),
            (a, 1),
            (a + 4, 128),
            (a + 0x14, 16),
            (a + 0x18, 0x20004),
            (a + 0x20, 0x20002),
            (a + 0x24, 0x10001000),
            (a + 0x28, 2),
        ] {
            write(&mut d, &mut h, base + o, v);
        }
        due(&mut d, &mut h);
        due(&mut d, &mut h);
        assert_eq!(
            h.frames[0].rgba,
            [
                255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255
            ]
        );
        write(&mut d, &mut h, base + a + 0x2c, 1);
        due(&mut d, &mut h);
        due(&mut d, &mut h);
        h.now = d.next_deadline(h.now).unwrap();
        let irqs = h.irqs.len();
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        d.advance(&mut cx);
        assert!(cx.refusal().unwrap().contains("fractional phase"));
        assert_eq!(h.irqs.len(), irqs);
        assert_eq!(h.frames.len(), 1);
    }
    #[test]
    fn converter_publication_binds_a_pair_and_corrupt_headers_still_refuse() {
        let mut d = Display::new(
            0xff9a0000,
            Clocks::new(),
            Config {
                period_ns: Some(30_000_000),
                ..Default::default()
            },
        )
        .unwrap();
        let mut h = OutputHost {
            ram: vec![0; 256],
            ..Default::default()
        };
        let base = d.base();
        let a = 0x58;
        let primary = (1 << 32) + 128;
        let next = (1 << 32) + 160;
        h.ram[128..144].copy_from_slice(&[0, 0, 255, 255].repeat(4));
        h.ram[160..176].copy_from_slice(&[255; 16]);
        let header = osd::encode(primary, 2, 2, &h.ram[128..144]).unwrap();
        h.ram[80..112].copy_from_slice(&header);
        for (o, v) in [
            (8, 1 << 20),
            (a, 1),
            (a + 4, 128),
            (a + 8, 1),
            (a + 12, 80),
            (a + 0x10, 1),
            (a + 0x18, 0x20002),
            (a + 0x20, 0x20002),
            (a + 0x24, 0x10001000),
        ] {
            write(&mut d, &mut h, base + o, v);
        }
        d.scanout(&mut Cx::new(&mut h, 0, 0, 0)).unwrap();
        assert_eq!(h.frames.len(), 1);
        write(&mut d, &mut h, base + a + 4, 160);
        d.scanout(&mut Cx::new(&mut h, 0, 0, 0)).unwrap();
        assert_eq!(h.frames.len(), 1);
        let header = osd::encode(next, 2, 2, &h.ram[160..176]).unwrap();
        h.ram[80..112].copy_from_slice(&header);
        d.scanout(&mut Cx::new(&mut h, 0, 0, 0)).unwrap();
        assert_eq!(h.frames.len(), 2);
        assert_eq!(h.frames[1].rgba, [255; 16]);
        assert_eq!(h.frames[1].sequence, 2);
        h.ram[108] = 1;
        assert!(d.scanout(&mut Cx::new(&mut h, 0, 0, 0)).is_err());
        assert_eq!(h.frames.len(), 2);
    }
    #[test]
    fn late_service_preserves_phase_deadlines_and_image_sequence() {
        let (mut early, mut eh) = setup(1);
        let (mut late, mut lh) = setup(1);
        while early.next_deadline(eh.now).unwrap() <= 140_000_000 {
            due(&mut early, &mut eh);
        }
        lh.now = 140_000_000;
        late.advance(&mut Cx::new(&mut lh, 0, 0, 0));
        assert_eq!(early.save(), late.save());
        assert_eq!(eh.irqs, lh.irqs);
        assert_eq!(eh.frames, lh.frames);
        // Disabling cancels the deadline. Stale tokens do no work.
        let base = late.base();
        write(&mut late, &mut lh, base + 4, 0);
        let state = late.save();
        lh.now += 1_000_000_000;
        late.event(&mut Cx::new(&mut lh, 0, 0, 0), 0);
        assert_eq!(late.save(), state);
        assert_eq!(late.next_deadline(lh.now), None);
    }
}
