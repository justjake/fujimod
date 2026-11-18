//! XP5 MOVDC packed422 output DMA, MOVRSZ output, the IBUF input write DMA and reset-only work
//! gates. MOVDC registers: size+18, address+c0/c4, stride+d0, cause+e0/W1C+e4,
//! mask+e8. Static evidence: 144be74/144cacc/144d118; no software ring access.
//!
//! IBUF (X-T5 4.31 0xff945000, driver 0x138a8dc..0x138ba9c, DMA mask 0x40000 of 0x2c6af44 /
//! 0x2c6ce5c). Static evidence:
//! * +0x00 bit 0: ready. Close (0x138b604) and open (0x138b9e0) clear it through 0x138a970(0) and
//!   then poll it (0x138a8dc) until it reads 1, with no timeout: the reset handshake of the other
//!   work gates.
//! * +0x40 bit 0: start. 0x138a970(1) sets it read-modify-write after the destination was
//!   written (+0x4c bits 2..0 / +0x50 address 34..32 / 31..0); bit 31 is a mode bit (0x138b6a8).
//!   Geometry +0x44 samples, +0x48 lines, +0x54/+0x58 pitch, +0x5c format, +0x60, +0x70 are kept
//!   as written. Every start is one transfer: X-T5 4.31 movie live view re-arms it once a frame
//!   (saclac 0x2e7a2e8 sets the RAW job count [0x4a0fff0] = 1, then 0x2c6ce5c).
//! * +0x68 bit 0: completion interrupt enable (0x138a94c). +0x6c: write 1 clears the completion
//!   (0x138a908; the ISR 0x138a928 does this before it calls the registered callback, it reads no
//!   status). Logical interrupt 0x7a = INTID 169 (table 0x18ea918).
//! * saclac needs that completion: callback 0x2e7fb34 sets [0x4a101f0] bit 1, which 0x2e7877c
//!   turns into bit 1 of [0x4a101e8]; without it the movie job never calls MOVDC back
//!   (0x3056164) and the movie live view stays black.
//! The model completes a start after `completion_us`, clears +0x40 bit 0, latches the completion
//! and raises INTID 169 while +0x68 enables it. It writes no pixels: the RAW destination is not
//! read by anything the emulator models yet.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
enum Kind {
    Reset,
    Movdc,
    Resize,
    Ibuf,
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
    /// Bring-up option: unmodeled aligned words read back their last write (kept in `state.regs`).
    retain_words: bool,
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
            retain_words: false,
        })
    }
    pub fn movdc(base: u64, period_us: u64, luma: u8) -> Result<Self, String> {
        Self::source(base, Kind::Movdc, period_us, Some(luma))
    }
    pub fn resize(base: u64, latency_us: u64) -> Result<Self, String> {
        Self::source(base, Kind::Resize, latency_us, None)
    }
    pub fn ibuf(base: u64, completion_us: u64) -> Result<Self, String> {
        Self::source(base, Kind::Ibuf, completion_us, None)
    }
    fn source(base: u64, kind: Kind, interval_us: u64, luma: Option<u8>) -> Result<Self, String> {
        if (kind == Kind::Movdc && !matches!(base, 0xff970000 | 0xff971000 | 0xff972000))
            || (kind == Kind::Resize && base != 0xff982000)
            || (kind == Kind::Ibuf && base != 0xff945000)
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
            retain_words: false,
        })
    }
    /// Let every aligned word of the window that has no modeled meaning read back its last
    /// write (0 before any write) instead of stopping as unmodeled. A bring-up option: the
    /// words have no side effects and are saved in the exact checkpoint with the other registers.
    pub fn with_retained_words(mut self) -> Self {
        self.retain_words = true;
        self
    }
    fn retained(&self, size: u8, o: u64) -> bool {
        self.retain_words && size == 4 && o & 3 == 0 && o < 0x1000 && !self.known(o)
    }
    fn irq(&self, cx: &mut Cx<'_>) {
        let line = match self.kind {
            Kind::Reset => return,
            Kind::Movdc => 171 + ((self.base - 0xff970000) / 0x1000) as u32,
            Kind::Resize => 200,
            Kind::Ibuf => 169,
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
        if self.kind == Kind::Ibuf {
            self.state.regs.insert(0x40, self.state.r(0x40) & !1);
            self.state.frames += 1;
            self.state.pending |= 1;
            self.irq(cx);
            return Ok(());
        }
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
            let stride = self.state.r(0xd0);
            // XT5_SCENE (a host input outside the checkpoint) replaces the flat source.
            if !fujimod_emulator_imaging::frame::paint_scene(cx, address, width, height, stride, self.state.frames)? {
                fujimod_emulator_imaging::frame::paint(cx, address, width, height, stride, luma)?;
            }
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
            // +24: input-buffer size (height<<16 | width, width/height
            // bound-checked by the writer before it stores them); only the
            // first gate is written with it. +10: read-modify-write control
            // word whose bit 16 the first gate's setter sets or clears. +14/+18: window
            // size words (height<<16 | 15-bit width) chosen by the setter's selector.
            // Every other word the first gate's setters, getters and read-modify-write
            // helpers address below +0x800 is a plain read-back word (PLAIN_RESET_WORDS).
            // The second gate (0xff973000) latches only the words its setters
            // write with a freshly computed value (+8 mode, +c/+10 size, +38 and
            // +40 selectors). +30 is polled by its stop routine, +3c arms its
            // interrupt, and +14..+2c/+34 are only read by a debug snapshot;
            // none of those is modeled.
            Kind::Reset => {
                matches!(o, 0 | 4)
                    || (self.base == 0xff950000 && PLAIN_RESET_WORDS.iter().any(|r| r.contains(&o)))
                    || (self.base == 0xff973000 && SECOND_GATE_PLAIN_WORDS.contains(&o))
            }
            Kind::Ibuf => o <= 0x74,
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
/// Word offsets of the second reset gate that only latch a written value.
/// Power-on values are unverified (read as zero).
const SECOND_GATE_PLAIN_WORDS: &[u64] = &[0x08, 0x0c, 0x10, 0x38, 0x40];
/// Word offsets (4-byte aligned) of the first reset gate that only latch a value.
const PLAIN_RESET_WORDS: &[std::ops::RangeInclusive<u64>] = &[
    0x08..=0x58,
    0x98..=0x98,
    0xdc..=0xdc,
    0xfc..=0xfc,
    0x13c..=0x13c,
    0x17c..=0x17c,
    0x1bc..=0x1bc,
    0x1fc..=0x1fc,
    0x23c..=0x23c,
    0x27c..=0x27c,
    0x2bc..=0x2bc,
    0x2fc..=0x308,
    0x328..=0x328,
    0x368..=0x368,
    0x3a8..=0x3a8,
    0x3e8..=0x3e8,
    0x428..=0x428,
    0x468..=0x468,
    0x4a8..=0x4a8,
    0x4e8..=0x4e8,
    0x528..=0x57c,
    0x5f0..=0x7ac,
];
impl Device for Work {
    fn name(&self) -> &'static str {
        match self.kind {
            Kind::Reset => "imaging-reset",
            Kind::Movdc => "movdc",
            Kind::Resize => "imaging-resize",
            Kind::Ibuf => "imaging-ibuf",
        }
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.advance(cx);
        let o = address.wrapping_sub(self.base);
        if self.retained(size, o) {
            return u64::from(self.state.r(o));
        }
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
            (Kind::Resize, 0x60) | (Kind::Movdc, 0xe8) | (Kind::Ibuf, 0x68) => {
                u64::from(self.state.irq_enable)
            }
            (Kind::Ibuf, 0x6c) => u64::from(self.state.pending),
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
        if self.retained(size, o) {
            self.state.regs.insert(o, v);
            return;
        }
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
                    if self.kind != Kind::Ibuf {
                        self.state.regs.insert(4, 0);
                    }
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
                if !matches!(self.kind, Kind::Reset | Kind::Ibuf) {
                    if v & 1 != 0 {
                        if let Err(e) = self.arm(cx) {
                            cx.fail(e);
                        }
                    } else {
                        self.cancel_frame();
                    }
                }
            }
            (Kind::Movdc, 0xe4) | (Kind::Resize, 0x5c) | (Kind::Ibuf, 0x6c) => {
                self.state.pending &= !(v & if self.kind == Kind::Movdc { 7 } else { 1 });
                self.irq(cx);
            }
            (Kind::Ibuf, 0x40) => {
                self.state.regs.insert(0x40, v);
                if v & 1 != 0 && self.state.frame_deadline.is_none() {
                    // The X-T5 movie job (0x138b538) can set +0x40 bit 0 while the +0x00 reset
                    // handshake is still running; the completion then follows the end of reset.
                    let from = self.state.reset_deadline.unwrap_or(0).max(cx.now());
                    let Some(at) = from.checked_add(self.interval_ns) else {
                        cx.fail("IBUF completion deadline overflow");
                        return;
                    };
                    self.state.frame_generation = self.state.frame_generation.wrapping_add(1);
                    self.state.frame_deadline = Some(at);
                    cx.schedule(at, self.state.frame_generation | (1 << 63));
                }
            }
            (Kind::Movdc, 0xe8) | (Kind::Resize, 0x60) | (Kind::Ibuf, 0x68) => {
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
            } else if self.kind != Kind::Ibuf {
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
    fujimod_emulator_devices::checkpoint_fields!(state: State);
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
    #[test]
    fn ibuf_start_completes_once_with_a_level_interrupt_cleared_by_6c() {
        const IBUF: u64 = 0xff945000;
        assert!(Work::ibuf(BASE, 8000).is_err());
        let mut work = Work::ibuf(IBUF, 8000).unwrap();
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        // Open (0x138b9e0): clear +0x6c, reset handshake on +0x00, destination, start.
        work.write(&mut cx, IBUF + 0x6c, 4, 1);
        work.write(&mut cx, IBUF + 0x04, 4, 0x1234);
        work.write(&mut cx, IBUF + 0x00, 4, 0);
        assert_eq!(work.read(&mut cx, IBUF, 4), 0);
        assert!(cx.refusal().is_none());
        host.now = 100_000;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        assert_eq!(work.read(&mut cx, IBUF, 4), 1);
        assert_eq!(work.read(&mut cx, IBUF + 0x04, 4), 0x1234);
        work.write(&mut cx, IBUF + 0x50, 4, 0x8e83_a350);
        work.write(&mut cx, IBUF + 0x68, 4, 1);
        let v = work.read(&mut cx, IBUF + 0x40, 4) as u32;
        work.write(&mut cx, IBUF + 0x40, 4, u64::from(v | 1));
        assert!(cx.refusal().is_none());
        assert_eq!(work.next_deadline(host.now), Some(8_100_000));
        assert!(!host.irqs.iter().any(|(i, on)| *i == 169 && *on));
        host.now = 8_100_000;
        work.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(host.irqs.last(), Some(&(169, true)));
        assert_eq!(work.next_deadline(host.now), None);
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        assert_eq!(work.read(&mut cx, IBUF + 0x40, 4), 0);
        // ISR 0x138a928: write 1 to +0x6c, then the next frame re-arms the transfer.
        work.write(&mut cx, IBUF + 0x6c, 4, 1);
        assert_eq!(host.irqs.last(), Some(&(169, false)));
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        work.write(&mut cx, IBUF + 0x40, 4, 1);
        // Without +0x68 the completion latches but the line stays low.
        work.write(&mut cx, IBUF + 0x68, 4, 0);
        assert_eq!(work.next_deadline(host.now), Some(16_100_000));
        assert_eq!(work.save()["state"]["frames"], 1);
        host.now = 16_100_000;
        work.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(work.save()["state"]["pending"], 1);
        assert_eq!(host.irqs.last(), Some(&(169, false)));
    }
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
mod reset_tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    const BASE: u64 = 0xff950000;
    #[test]
    fn reset_gate_reports_ready_after_its_deadline_and_keeps_the_enable_word() {
        let mut work = Work::reset(BASE).unwrap();
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        assert_eq!(work.read(&mut cx, BASE, 4), 1);
        work.write(&mut cx, BASE, 4, 0);
        assert_eq!(work.read(&mut cx, BASE, 4), 0);
        assert!(cx.refusal().is_none());
        host.now = 100_000;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        assert_eq!(work.read(&mut cx, BASE, 4), 1);
        // Read-modify-write of +4 bit 0 in both directions.
        let word = work.read(&mut cx, BASE + 4, 4);
        work.write(&mut cx, BASE + 4, 4, word | 1);
        assert_eq!(work.read(&mut cx, BASE + 4, 4), 1);
        let word = work.read(&mut cx, BASE + 4, 4);
        work.write(&mut cx, BASE + 4, 4, word & !1);
        assert_eq!(work.read(&mut cx, BASE + 4, 4), 0);
        assert!(cx.refusal().is_none());
        assert!(host.irqs.is_empty());
    }
    #[test]
    fn reset_gate_control_word_at_8_is_a_plain_read_modify_write_word() {
        // The firmware's setters read +8, change one bit and write it back, and
        // nothing polls it afterward. The power-on value is unverified.
        let mut work = Work::reset(BASE).unwrap();
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        assert_eq!(work.read(&mut cx, BASE + 8, 4), 0);
        for bit in [0, 4, 8, 12, 16, 17, 20, 21, 24] {
            let word = work.read(&mut cx, BASE + 8, 4);
            work.write(&mut cx, BASE + 8, 4, word | (1 << bit));
            assert_eq!(work.read(&mut cx, BASE + 8, 4), word | (1 << bit));
        }
        assert!(cx.refusal().is_none());
    }
    #[test]
    fn reset_gate_refuses_the_rest_of_the_block() {
        let mut work = Work::reset(BASE).unwrap();
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        work.read(&mut cx, BASE + 0x5c, 4);
        assert!(cx.refusal().is_some());
    }
}

#[cfg(test)]
mod retain_tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    const BASE: u64 = 0xff950000;
    #[test]
    fn reset_gate_retains_unmodeled_words_only_when_enabled() {
        let mut host = RecordingHost::default();
        let mut strict = Work::reset(BASE).unwrap();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        strict.write(&mut cx, BASE + 0x5c, 4, 5);
        assert!(cx.is_unmodeled());

        let mut work = Work::reset(BASE).unwrap().with_retained_words();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        assert_eq!(work.read(&mut cx, BASE + 0x10, 4), 0);
        // Read-modify-write bit set/clear, as X-T5 4.31 does on +0x08..+0x10.
        work.write(&mut cx, BASE + 0x10, 4, 0x8000_0001);
        let v = work.read(&mut cx, BASE + 0x10, 4);
        work.write(&mut cx, BASE + 0x10, 4, v & !1);
        work.write(&mut cx, BASE + 0x7ac, 4, 0x1234_5678);
        assert_eq!(work.read(&mut cx, BASE + 0x10, 4), 0x8000_0000);
        assert_eq!(work.read(&mut cx, BASE + 0x7ac, 4), 0x1234_5678);
        // Modeled registers keep their behaviour: +0 reports ready, not the written word.
        work.write(&mut cx, BASE, 4, 3);
        assert_eq!(work.read(&mut cx, BASE, 4), 1);
        // Narrow accesses stay unmodeled.
        assert!(!cx.is_unmodeled());
        work.read(&mut cx, BASE + 0x10, 2);
        assert!(cx.is_unmodeled());

        let checkpoint = work.checkpoint().unwrap();
        let mut restored = Work::reset(BASE).unwrap().with_retained_words();
        restored.restore_checkpoint(&checkpoint).unwrap();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        assert_eq!(restored.read(&mut cx, BASE + 0x7ac, 4), 0x1234_5678);
        assert_eq!(restored.checkpoint().unwrap(), checkpoint);
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

#[cfg(test)]
mod reset_size_tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    #[test]
    fn first_reset_gate_keeps_the_programmed_input_buffer_size() {
        let mut work = Work::reset(0xff950000).unwrap();
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 1, 0xdead, 0);
        work.write(&mut cx, 0xff950024, 4, 0x1680f24);
        assert!(cx.refusal().is_none());
        assert_eq!(work.read(&mut cx, 0xff950024, 4), 0x1680f24);
        work.write(&mut cx, 0xff950010, 4, 0x10000);
        assert_eq!(work.read(&mut cx, 0xff950010, 4), 0x10000);
        for (o, v) in [(0xff950014, 0x00500080), (0xff950018, 0x00200040)] {
            work.write(&mut cx, o, 4, v);
            assert_eq!(work.read(&mut cx, o, 4), v);
        }
        let mut other = Work::reset(0xff973000).unwrap();
        other.write(&mut cx, 0xff973024, 4, 1);
        assert!(cx.refusal().is_some());
    }

    #[test]
    fn first_reset_gate_keeps_the_remaining_configuration_words() {
        let mut work = Work::reset(0xff950000).unwrap();
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 1, 0xdead, 0);
        let offsets = [0x0c, 0x1c, 0x20, 0x28, 0x2c, 0x38, 0x3c, 0x40, 0x44, 0x48, 0x4c, 0x50, 0x54, 0x58, 0x98, 0x2bc, 0x328, 0x57c, 0x5f0, 0x7a8, 0x7ac];
        for (i, o) in offsets.into_iter().enumerate() {
            let v = 0x420012 + i as u64;
            work.write(&mut cx, 0xff950000 + o, 4, v);
            assert_eq!(work.read(&mut cx, 0xff950000 + o, 4), v);
        }
        assert!(cx.refusal().is_none());
        work.write(&mut cx, 0xff95005c, 4, 1);
        assert!(cx.refusal().is_some());
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 1, 0xdead, 0);
        let mut other = Work::reset(0xff973000).unwrap();
        other.write(&mut cx, 0xff97301c, 4, 1);
        assert!(cx.refusal().is_some());
    }

    #[test]
    fn second_reset_gate_keeps_the_plain_words_and_refuses_the_polled_and_snapshot_words() {
        let mut work = Work::reset(0xff973000).unwrap();
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 1, 0xdead, 0);
        for (i, o) in [0x08, 0x0c, 0x10, 0x38, 0x40].into_iter().enumerate() {
            assert_eq!(work.read(&mut cx, 0xff973000 + o, 4), 0);
            let v = 3 + i as u64;
            work.write(&mut cx, 0xff973000 + o, 4, v);
            assert_eq!(work.read(&mut cx, 0xff973000 + o, 4), v);
        }
        assert!(cx.refusal().is_none());
        for o in [0x14u64, 0x30, 0x3c, 0x44] {
            let mut host = RecordingHost::default();
            let mut cx = Cx::new(&mut host, 1, 0xdead, 0);
            work.write(&mut cx, 0xff973000 + o, 4, 1);
            assert!(cx.refusal().is_some());
        }
    }

    #[test]
    fn first_reset_gate_keeps_the_run_of_words_between_0x638_and_0x674() {
        let mut work = Work::reset(0xff950000).unwrap();
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 1, 0xdead, 0);
        for (i, o) in (0x638..=0x674u64).step_by(4).enumerate() {
            let v = 0x087408ae + i as u64;
            work.write(&mut cx, 0xff950000 + o, 4, v);
            assert_eq!(work.read(&mut cx, 0xff950000 + o, 4), v);
        }
        assert!(cx.refusal().is_none());
        work.write(&mut cx, 0xff950001 + 0x63c, 1, 1);
        assert!(cx.refusal().is_some());
    }
}
