//! XP5 MOVDC packed422 output DMA, MOVRSZ output and reset-only work gates.
//! MOVDC registers: size+18, address+c0/c4, stride+d0, cause+e0/W1C+e4,
//! mask+e8. Static evidence: 144be74/144cacc/144d118; no software ring access.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
enum Kind {
    Reset,
    Movdc,
    Resize,
}
#[derive(Default, Serialize, Deserialize)]
struct State {
    regs: BTreeMap<u64, u32>,
    pending: u32,
    irq_enable: u32,
    reset_generation: u64,
    frame_generation: u64,
    reset_deadline: Option<u64>,
    frame_deadline: Option<u64>,
    frames: u64,
}
impl State {
    fn r(&self, o: u64) -> u32 {
        self.regs.get(&o).copied().unwrap_or(0)
    }
}
pub struct Work {
    base: u64,
    kind: Kind,
    state: State,
    interval_ns: u64,
    source_luma: Option<u8>,
}
impl Work {
    pub fn reset(base: u64) -> Result<Self, String> {
        if !matches!(base, 0xff950000 | 0xff973000) {
            return Err("invalid imaging reset gate".into());
        }
        Ok(Self {
            base,
            kind: Kind::Reset,
            state: State::default(),
            interval_ns: 0,
            source_luma: None,
        })
    }
    pub fn movdc(base: u64, period_us: u64, luma: u8) -> Result<Self, String> {
        Self::source(base, Kind::Movdc, period_us, Some(luma))
    }
    pub fn resize(base: u64, latency_us: u64) -> Result<Self, String> {
        Self::source(base, Kind::Resize, latency_us, None)
    }
    fn source(base: u64, kind: Kind, interval_us: u64, luma: Option<u8>) -> Result<Self, String> {
        if (kind == Kind::Movdc && !matches!(base, 0xff970000 | 0xff971000 | 0xff972000))
            || (kind == Kind::Resize && base != 0xff982000)
            || luma.is_some_and(|n| !(16..=235).contains(&n))
        {
            return Err("invalid imaging work block/source".into());
        }
        let interval_ns = interval_us
            .checked_mul(1000)
            .filter(|v| *v > 0)
            .ok_or("imaging work needs a positive bounded interval")?;
        Ok(Self {
            base,
            kind,
            state: State::default(),
            interval_ns,
            source_luma: luma,
        })
    }
    fn irq(&self, cx: &mut Cx<'_>) {
        let line = match self.kind {
            Kind::Reset => return,
            Kind::Movdc => 171 + ((self.base - 0xff970000) / 0x1000) as u32,
            Kind::Resize => 200,
        };
        cx.set_irq(line, self.state.pending & self.state.irq_enable != 0);
    }
    fn ready(&self) -> bool {
        self.state.reset_deadline.is_none()
    }
    fn cancel_frame(&mut self) {
        self.state.frame_deadline = None;
        self.state.frame_generation = self.state.frame_generation.wrapping_add(1);
    }
    fn arm(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if self.state.frame_deadline.is_none() && self.state.r(4) & 1 != 0 {
            if !self.ready() {
                return Err("imaging work started while reset is active".into());
            }
            let at = cx
                .now()
                .checked_add(self.interval_ns)
                .ok_or("imaging frame deadline overflow")?;
            self.state.frame_generation = self.state.frame_generation.wrapping_add(1);
            self.state.frame_deadline = Some(at);
            cx.schedule(at, self.state.frame_generation | (1 << 63));
        }
        Ok(())
    }
    fn produce(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if self.kind == Kind::Movdc {
            let luma = self
                .source_luma
                .ok_or("MOVDC has no declared frame source")?;
            // Only the observed packed422 mode is implemented. The second
            // programmed address is retained as opaque register data.
            if self.state.r(0xf0) != 0xb
                || self.state.r(0xd8) != 0x10030001
                || self.state.r(0x18) & !0x07ff07ff != 0
                || self.state.r(0xc0) > 7
            {
                return Err("unsupported MOVDC output mode/size/address".into());
            }
            let size = self.state.r(0x18);
            let width = (size & 0x7ff) + 1;
            let height = ((size >> 16) & 0x7ff) + 1;
            let address = (u64::from(self.state.r(0xc0)) << 32) | u64::from(self.state.r(0xc4));
            if address == 0 || address & 7 != 0 {
                return Err("MOVDC output DMA requires a nonzero 8-byte-aligned address".into());
            }
            fujimod_emulator_imaging::frame::paint(cx, address, width, height, self.state.r(0xd0), luma)?;
        } else {
            // Half-size packed422: consume the actual programmed input. The
            // supported horizontal filter uses the normalized seven-tap
            // coefficient contract checked by stock2f0af74.
            let width = self.state.r(0x1c) & 0xffff;
            let height = self.state.r(0x1c) >> 16;
            let input_stride = self.state.r(0x18);
            let output_stride = self.state.r(0x38);
            let source = (u64::from(self.state.r(0x10) & 7) << 32) | u64::from(self.state.r(0x14));
            let address = (u64::from(self.state.r(0x30) & 7) << 32) | u64::from(self.state.r(0x34));
            let control = self.state.r(8);
            let weights = self.state.r(0x44).to_be_bytes();
            let normalized = 2
                * (u32::from(weights[0]) + u32::from(weights[1]) + u32::from(weights[2]))
                + u32::from(weights[3])
                == 256;
            if control & !0x01000000 != 0
                || (control & 0x01000000 != 0 && !normalized)
                || !(4..=4096).contains(&width)
                || width & 3 != 0
                || !(2..=4096).contains(&height)
                || height & 1 != 0
                || input_stride < width * 2
                || output_stride < width
                || input_stride & 7 != 0
                || output_stride & 7 != 0
                || self.state.r(0x10) > 7
                || self.state.r(0x30) > 7
                || source == 0
                || address == 0
            {
                return Err(format!("unsupported MOVRSZ mode/geometry: control={control:#x} size={:#x} input_stride={input_stride:#x} output_stride={output_stride:#x} source={source:#x} destination={address:#x}",self.state.r(0x1c)));
            }
            let length = width as usize * height as usize * 2;
            let mut input = vec![0; length];
            for y in 0..height as usize {
                cx.ram_read(
                    source + y as u64 * u64::from(input_stride),
                    &mut input[y * width as usize * 2..(y + 1) * width as usize * 2],
                )?;
            }
            let weights = (control & 0x01000000 != 0).then_some(weights);
            let output = fujimod_emulator_imaging::frame::half_422(&input, width as usize, height as usize, weights)?;
            for y in 0..height as usize / 2 {
                cx.ram_write(
                    address + y as u64 * u64::from(output_stride),
                    &output[y * width as usize..(y + 1) * width as usize],
                )?;
            }
            for offset in [0x4c, 0x50] {
                self.state
                    .regs
                    .insert(offset, self.state.r(offset) & !0x10000);
            }
        }
        self.state.frames += 1;
        self.state.pending |= 1;
        self.irq(cx);
        Ok(())
    }
    fn known(&self, o: u64) -> bool {
        if o & 3 != 0 {
            return false;
        }
        match self.kind {
            Kind::Reset => matches!(o, 0 | 4),
            Kind::Resize => matches!(
                o,
                0 | 4
                    | 8
                    | 0x10
                    | 0x14
                    | 0x18
                    | 0x1c
                    | 0x30
                    | 0x34
                    | 0x38
                    | 0x3c
                    | 0x40
                    | 0x44
                    | 0x48
                    | 0x4c
                    | 0x50
                    | 0x54
                    | 0x58
                    | 0x5c
                    | 0x60
                    | 0x64
            ),
            Kind::Movdc => {
                o <= 0xb8
                    || matches!(
                        o,
                        0xc0 | 0xc4
                            | 0xc8
                            | 0xcc
                            | 0xd0
                            | 0xd4
                            | 0xd8
                            | 0xdc
                            | 0xe0
                            | 0xe4
                            | 0xe8
                            | 0xf0
                            | 0x100
                            | 0x104
                    )
            }
        }
    }
}
impl Device for Work {
    fn name(&self) -> &'static str {
        match self.kind {
            Kind::Reset => "imaging-reset",
            Kind::Movdc => "movdc",
            Kind::Resize => "imaging-resize",
        }
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.advance(cx);
        let o = address.wrapping_sub(self.base);
        if size != 4 || !self.known(o) {
            cx.unmodeled();
            return 0;
        }
        match (self.kind, o) {
            (_, 0) => u64::from(self.ready()),
            (Kind::Resize, 0x54) => {
                if self.ready() && self.state.frame_deadline.is_none() {
                    0x10001
                } else {
                    0
                }
            }
            (Kind::Resize, 0x58) | (Kind::Movdc, 0xe0) => u64::from(self.state.pending),
            (Kind::Resize, 0x60) | (Kind::Movdc, 0xe8) => u64::from(self.state.irq_enable),
            (Kind::Movdc, 0xdc) => {
                if self.state.r(0xd4) & (1 << 28) == 0 {
                    0x100
                } else {
                    0
                }
            }
            _ => u64::from(self.state.r(o)),
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.advance(cx);
        let o = address.wrapping_sub(self.base);
        let v = value as u32;
        if size != 4 || !self.known(o) {
            cx.unmodeled();
            return;
        }
        match (self.kind, o) {
            (_, 0) => {
                self.state.regs.insert(0, v);
                if v & 1 == 0 {
                    self.cancel_frame();
                    self.state.pending = 0;
                    self.state.regs.insert(4, 0);
                    self.state.reset_generation = self.state.reset_generation.wrapping_add(1);
                    let Some(at) = cx.now().checked_add(100_000) else {
                        cx.fail("imaging reset deadline overflow");
                        return;
                    };
                    self.state.reset_deadline = Some(at);
                    cx.schedule(at, self.state.reset_generation);
                    self.irq(cx);
                } else {
                    self.state.reset_deadline = None;
                }
            }
            (_, 4) => {
                self.state.regs.insert(4, v);
                if self.kind != Kind::Reset {
                    if v & 1 != 0 {
                        if let Err(e) = self.arm(cx) {
                            cx.fail(e);
                        }
                    } else {
                        self.cancel_frame();
                    }
                }
            }
            (Kind::Movdc, 0xe4) | (Kind::Resize, 0x5c) => {
                self.state.pending &= !(v & if self.kind == Kind::Movdc { 7 } else { 1 });
                self.irq(cx);
            }
            (Kind::Movdc, 0xe8) | (Kind::Resize, 0x60) => {
                self.state.irq_enable = v & if self.kind == Kind::Movdc { 7 } else { 1 };
                self.irq(cx);
            }
            _ => {
                self.state.regs.insert(o, v);
            }
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if (token & (1 << 63) == 0 && token == self.state.reset_generation)
            || (token & (1 << 63) != 0 && token & !(1 << 63) == self.state.frame_generation)
        {
            self.advance(cx);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.state.reset_deadline.is_some_and(|at| at <= cx.now()) {
            self.state.reset_deadline = None;
        }
        while let Some(at) = self.state.frame_deadline.filter(|at| *at <= cx.now()) {
            self.state.frame_deadline = None;
            if let Err(e) = self.produce(cx) {
                cx.fail(e);
                return;
            }
            if self.kind == Kind::Movdc && self.state.r(4) & 1 != 0 {
                let Some(next) = at.checked_add(self.interval_ns) else {
                    cx.fail("MOVDC frame deadline overflow");
                    return;
                };
                self.state.frame_deadline = Some(next);
                cx.schedule(next, self.state.frame_generation | (1 << 63));
            } else {
                self.state.regs.insert(4, 0);
            }
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state
            .reset_deadline
            .into_iter()
            .chain(self.state.frame_deadline)
            .min()
    }
    fn save(&self) -> Value {
        json!({"state":self.state})
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        self.state = serde_json::from_value(v["state"].clone())
            .map_err(|e| format!("imaging work checkpoint: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    const BASE: u64 = 0xff970000;
    fn setup(work: &mut Work, host: &mut RecordingHost, mode: u64) {
        let mut cx = Cx::new(host, 3, 0xdead, 0);
        for (o, v) in [
            (0x18, (3 << 16) | 7),
            (0xc0, 1),
            (0xc4, 0x110),
            (0xd0, 24),
            (0xd8, 0x10030001),
            (0xf0, mode),
            (0xe8, 1),
            (4, 1),
        ] {
            work.write(&mut cx, BASE + o, 4, v);
        }
        assert!(cx.refusal().is_none());
    }
    #[test]
    fn movdc_uses_only_programmed_output_geometry_deadline_and_level_irq() {
        let mut work = Work::movdc(BASE, 1000, 128).unwrap();
        let mut host = RecordingHost {
            ram_base: 0x1_0000_0100,
            ram: vec![0x55; 160],
            ..Default::default()
        };
        setup(&mut work, &mut host, 0xb);
        let saved = work.save();
        let token = saved["state"]["frame_generation"].as_u64().unwrap() | (1 << 63);
        assert_eq!(work.next_deadline(0), Some(1_000_000));
        host.now = 999_999;
        work.event(&mut Cx::new(&mut host, 0, 0, 0), token);
        assert_eq!(work.save(), saved);
        assert!(host.ram.iter().all(|v| *v == 0x55));
        work.restore(&saved).unwrap();
        host.now = 1_000_000;
        work.advance(&mut Cx::new(&mut host, 0, 0, 0));
        for y in 0..4 {
            assert_eq!(&host.ram[16 + y * 24..16 + y * 24 + 16], &[128; 16]);
            assert_eq!(&host.ram[16 + y * 24 + 16..16 + y * 24 + 24], &[0x55; 8]);
        }
        assert!(host.irqs.contains(&(171, true)));
        assert_eq!(work.save()["state"]["frames"], 1);
        assert_eq!(work.next_deadline(host.now), Some(2_000_000));
        work.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0xe4, 4, 1);
        assert_eq!(host.irqs.last(), Some(&(171, false)));
        work.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 4, 4, 0);
        host.now = 9_000_000;
        work.event(&mut Cx::new(&mut host, 0, 0, 0), token);
        assert_eq!(work.next_deadline(host.now), None);
        assert_eq!(work.save()["state"]["frames"], 1);
    }
    #[test]
    fn unsupported_output_does_not_receive_pixels_or_completion() {
        let mut work = Work::movdc(BASE, 1000, 128).unwrap();
        let mut host = RecordingHost {
            ram_base: 0x1_0000_0100,
            ram: vec![0x55; 160],
            ..Default::default()
        };
        setup(&mut work, &mut host, 0);
        host.now = 1_000_000;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        work.advance(&mut cx);
        assert!(cx.refusal().unwrap().contains("unsupported MOVDC"));
        assert!(host.ram.iter().all(|v| *v == 0x55));
        assert!(!host.irqs.contains(&(171, true)));
        assert_eq!(work.save()["state"]["frames"], 0);
    }
}

#[cfg(test)]
mod resize_tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    const BASE: u64 = 0xff982000;
    #[test]
    fn half_resize_reads_and_filters_actual_dma_and_refuses_unknown_modes() {
        let mut work = Work::resize(BASE, 1000).unwrap();
        let mut host = RecordingHost {
            ram_base: 0x1_0000_1000,
            ram: vec![0x55; 0x2000],
            ..Default::default()
        };
        for row in 0..8 {
            host.ram[row * 40..row * 40 + 32].fill(128);
        }
        {
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            for (o, v) in [
                (8, 0x1000000),
                (0x44, 0x4080),
                (0x10, 1),
                (0x14, 0x1000),
                (0x18, 40),
                (0x1c, (8 << 16) | 16),
                (0x30, 1),
                (0x34, 0x2000),
                (0x38, 24),
                (0x60, 1),
                (4, 1),
            ] {
                work.write(&mut cx, BASE + o, 4, v);
            }
            assert!(cx.refusal().is_none());
        }
        host.now = 1_000_000;
        work.advance(&mut Cx::new(&mut host, 0, 0, 0));
        for row in 0..4 {
            assert_eq!(
                &host.ram[0x1000 + row * 24..0x1000 + row * 24 + 16],
                &[128; 16]
            );
        }
        assert_eq!(work.save()["state"]["frames"], 1);
        assert!(host.irqs.contains(&(200, true)));
        work.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x5c, 4, 1);
        host.ram[4 * 40 + 7] = 64;
        host.ram[0x1000..0x1100].fill(0x55);
        work.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 4, 4, 1);
        host.now = 2_000_000;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        work.advance(&mut cx);
        assert!(cx.refusal().is_none());
        assert_eq!(host.ram[0x1000 + 2 * 24 + 7], 112);
        assert_eq!(work.save()["state"]["frames"], 2);
        work.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x5c, 4, 1);
        host.ram[0x1000..0x1100].fill(0x55);
        work.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 8, 4, 0x1000010);
        work.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 4, 4, 1);
        host.now = 3_000_000;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        work.advance(&mut cx);
        assert!(cx.refusal().unwrap().contains("unsupported MOVRSZ mode"));
        assert!(host.ram[0x1000..0x1100].iter().all(|v| *v == 0x55));
        assert_eq!(work.save()["state"]["frames"], 2);
        assert_eq!(host.irqs.last(), Some(&(200, false)));
    }
}
