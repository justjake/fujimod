//! XP5 MOVDC output DMA, MOVRSZ output, the IBUF input write DMA and reset-only work
//! gates. MOVDC registers: size+18, address+c0/c4, stride+d0, cause+e0/W1C+e4,
//! mask+e8. Static evidence: 144be74/144cacc/144d118; no software ring access.
//!
//! MOVDC output format (X-T5 4.31 driver; channel base `(n + 0xff970) << 12`):
//! * +08 (setter 0x13609d8, called per channel from 0x2c6cd10/0x2c6cd3c/0x2c6cd68): bits 1..0
//!   the output format, bit 8 and bit 12 two flags. The caller maps the saclac per-channel
//!   format words [0x4ba5c68 + 0x54/0x64/0x74] to it (0x2e9a9f0..0x2e9ab68). Live view writes
//!   0x100 (format 0, packed 4:2:2, one plane); the FHD movie channel 2 writes 0x101 (format 1)
//!   with a second address. Format 1 is modeled as 8-bit two-plane 4:2:0 with interleaved Cb,Cr
//!   (the H.264 encoder's input format); the half-height chroma is inferred from that consumer,
//!   not from a MOVDC-side proof. Formats 2 and 3 are refused.
//! * Address setter 0x1361804: plane 0 address bits 34..32 -> +c0, bits 31..0 -> +c4; plane 1
//!   address -> +c8/+cc; one stride -> +d0 for both planes. It requires both addresses
//!   8-byte aligned and below 0x7fffffff0, and the stride a multiple of 4.
//!
//! MOVRSZ (X-T5 4.31 0xff982000, driver 0x2c4419c..0x2c44e64, logical IRQ 0x6e = INTID 200).
//! The parameter block of set 0x2c44264 (static evidence) programs:
//! * +08 bit 24 and +44 (0x2c445b0): horizontal seven-tap filter, +44 = w0 << 24 | w1 << 16 |
//!   w2 << 8 | w3, taps w0 w1 w2 w3 w2 w1 w0, required sum 256.
//! * +08 bit 20 and +40 (0x2c44670): vertical five-tap filter, +40 = v0 << 16 | v1 << 8 | v2
//!   (7 bits each), taps v0 v1 v2 v1 v0, required sum 128.
//! * +08 bit 16 and +3c bits 9..0 (0x2c44744), +64 (0x2c447d0): not decoded, refused when used.
//! * +08 bits 5..4 output class (0..2), bits 1..0 input class (0..2), bit 8 a flag
//!   (0x2c4481c). The pixel count must be a multiple of 16 for output class 0, of 32 for class 1.
//!   Source +10/+14 (bits 34..32 / 31..0), input stride +18, size +1c = lines << 16 | pixels
//!   (counts), destination +30/+34, output stride +38, +48 bits 9..0 = 0x200.
//! * Start 0x2c44ba4: +4c/+50 bit 16, then +04 bit 0. Status +58, clear +5c (ISR 0x2c44164),
//!   enable +60. Stop 0x2c4439c polls +54 until (v & v >> 16) bit 0.
//!
//! Output class 0 with input class 0 is the half-size packed 4:2:2 of live view (horizontal
//! filter only). Output class 1 is half-size 8-bit luma: the HDR alignment pyramid
//! (recfPrProcess 0x2d3d414, steps 0x59..0x5c, setup 0x2d31d48). Its first level reads the
//! packed 4:2:2 frame (input class 0, both filters; X-T5 4.31 HDR: +08 0x1100010, 7744x5152,
//! strides 0x3c80); the next levels read the previous 8-bit level (input class 1, call at
//! 0x2d32174) inside one buffer. The pyramid table at 0x2d31f94 halves each level (bytes / 2
//! rounded up to 32, lines / 2 rounded up to 2), which is what makes class 1 a half-size,
//! one-byte-per-pixel output [inferred]. The model takes output pixel (x, y) from input (2x, 2y)
//! after the enabled filters (horizontal first, rounded to 8 bits, then vertical), edges
//! clamped.
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
use fujimod_emulator_imaging::frame::{self, YuvLayout};
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
    /// Bring-up option: unmodeled aligned words read back their last write (kept in `state.regs`).
    retain_words: bool,
    /// MOVDC: the sensor exposure that scales the host scene ([`Work::with_exposure`]).
    exposure: Option<fujimod_emulator_imaging::exposure::ExposureHandle>,
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
            exposure: None,
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
            retain_words: false,
            exposure: None,
        })
    }
    /// Let every aligned word of the window that has no modeled meaning read back its last
    /// write (0 before any write) instead of stopping as unmodeled. A bring-up option: the
    /// words have no side effects and are saved in the exact checkpoint with the other registers.
    pub fn with_retained_words(mut self) -> Self {
        self.retain_words = true;
        self
    }
    /// MOVDC: scale the host scene's linear levels by the sensor exposure relative to the
    /// reference ([`frame::paint_scene_exposed`]), read at each frame, so live view follows the
    /// firmware's gain programming.
    pub fn with_exposure(mut self, exposure: fujimod_emulator_imaging::exposure::ExposureHandle) -> Self {
        self.exposure = Some(exposure);
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
            // Output format +08 bits 1..0 (setter 0x13609d8): 0 is packed 4:2:2 (live view),
            // 1 is 8-bit two-plane 4:2:0 with the Cb,Cr plane at +c8/+cc (the FHD movie
            // encoder feed). Other formats are refused.
            let format = self.state.r(8) & 3;
            if self.state.r(0xf0) != 0xb
                || self.state.r(0xd8) != 0x10030001
                || self.state.r(0x18) & !0x07ff07ff != 0
                || self.state.r(0xc0) > 7
                || format > 1
                || (format == 1 && self.state.r(0xc8) > 7)
            {
                return Err(format!(
                    "unsupported MOVDC output mode/size/address: +08 {:#x} +18 {:#x} +c0 {:#x} +c8 {:#x} +d8 {:#x} +f0 {:#x}",
                    self.state.r(8),
                    self.state.r(0x18),
                    self.state.r(0xc0),
                    self.state.r(0xc8),
                    self.state.r(0xd8),
                    self.state.r(0xf0)
                ));
            }
            let size = self.state.r(0x18);
            let width = (size & 0x7ff) + 1;
            let height = ((size >> 16) & 0x7ff) + 1;
            let address = (u64::from(self.state.r(0xc0)) << 32) | u64::from(self.state.r(0xc4));
            if address == 0 || address & 7 != 0 {
                return Err("MOVDC output DMA requires a nonzero 8-byte-aligned address".into());
            }
            // One stride serves both planes: the address setter 0x1361804 writes only +d0.
            let stride = self.state.r(0xd0);
            let layout = if format == 0 {
                YuvLayout::Packed422
            } else {
                let chroma = (u64::from(self.state.r(0xc8)) << 32) | u64::from(self.state.r(0xcc));
                if chroma == 0 || chroma & 7 != 0 {
                    return Err(format!(
                        "MOVDC two-plane output requires a nonzero 8-byte-aligned chroma address, not {chroma:#x}"
                    ));
                }
                YuvLayout::TwoPlane420 { chroma, chroma_stride: stride }
            };
            // XT5_SCENE (a host input outside the checkpoint) replaces the flat source.
            let exposure = self
                .exposure
                .as_ref()
                .map_or(1., |e| e.lock().map_or(1., |e| e.relative()));
            if !frame::paint_scene_exposed(
                cx,
                address,
                width,
                height,
                stride,
                layout,
                self.state.frames,
                exposure,
            )? {
                frame::paint_layout(cx, address, width, height, stride, layout, luma)?;
            }
        } else if self.state.r(8) & 0x30 == 0x10 {
            // Output class 1 (+08 bits 5..4): half-size 8-bit luma (module notes).
            self.luma_half(cx)?;
            for offset in [0x4c, 0x50] {
                self.state
                    .regs
                    .insert(offset, self.state.r(offset) & !0x10000);
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
            let output = frame::half_422(&input, width as usize, height as usize, weights)?;
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
    /// MOVRSZ output class 1: half-size 8-bit luma from packed 4:2:2 or 8-bit luma input, with
    /// the optional seven-tap horizontal (+08 bit 24, +44) and five-tap vertical (+08 bit 20,
    /// +40) filters (module notes).
    fn luma_half(&self, cx: &mut Cx<'_>) -> Result<(), String> {
        let r = |o| self.state.r(o);
        let control = r(8);
        let (width, height) = ((r(0x1c) & 0xffff) as usize, (r(0x1c) >> 16) as usize);
        let (input_stride, output_stride) = (r(0x18) as usize, r(0x38) as usize);
        let source = (u64::from(r(0x10) & 7) << 32) | u64::from(r(0x14));
        let address = (u64::from(r(0x30) & 7) << 32) | u64::from(r(0x34));
        // Input class (+08 bits 1..0): 0 packed 4:2:2, 1 8-bit luma.
        let bytes = if control & 3 == 0 { 2 } else { 1 };
        let h = r(0x44).to_be_bytes();
        let v = [(r(0x40) >> 16) & 0x7f, (r(0x40) >> 8) & 0x7f, r(0x40) & 0x7f];
        let horizontal = (control & 0x0100_0000 != 0).then_some([h[0], h[1], h[2], h[3], h[2], h[1], h[0]]);
        let vertical = (control & 0x0010_0000 != 0).then_some([v[0], v[1], v[2], v[1], v[0]]);
        if control & !0x0110_0031 != 0
            || horizontal.is_some_and(|t| t.iter().map(|&w| u32::from(w)).sum::<u32>() != 256)
            || vertical.is_some_and(|t| t.iter().sum::<u32>() != 128)
            || !(0x20..=0xfff0).contains(&width)
            || width % 32 != 0
            || !(2..=0xfffa).contains(&height)
            || height % 2 != 0
            || input_stride < width * bytes
            || output_stride < width / 2
            || input_stride % 8 != 0
            || output_stride % 8 != 0
            || r(0x10) > 7
            || r(0x30) > 7
            || source == 0
            || address == 0
        {
            return Err(format!(
                "unsupported MOVRSZ mode/geometry: control={control:#x} size={:#x} input_stride={input_stride:#x} \
                 output_stride={output_stride:#x} source={source:#x} destination={address:#x} +40={:#x} +44={:#x}",
                r(0x1c),
                r(0x40),
                r(0x44)
            ));
        }
        cx.log(&format!(
            "MOVRSZ: half-size 8-bit luma {}x{height} -> {}x{} from {} at {source:#x} (stride {input_stride:#x}) \
             to {address:#x} (stride {output_stride:#x}), horizontal filter {horizontal:?}, vertical filter {vertical:?}",
            width,
            width / 2,
            height / 2,
            if bytes == 2 { "packed 4:2:2" } else { "8-bit luma" },
        ));
        let mut line = vec![0; width * bytes];
        let mut rows = Vec::with_capacity(height * width / 2);
        for y in 0..height {
            cx.ram_read(source + (y * input_stride) as u64, &mut line)?;
            let luma = |x: usize| if bytes == 2 { line[x / 2 * 4 + x % 2] } else { line[x] };
            rows.extend((0..width / 2).map(|x| match horizontal {
                None => luma(2 * x),
                Some(taps) => {
                    let sum: u32 = taps
                        .iter()
                        .enumerate()
                        .map(|(t, &w)| u32::from(w) * u32::from(luma((2 * x + t).saturating_sub(3).min(width - 1))))
                        .sum();
                    ((sum + 128) >> 8) as u8
                }
            }));
        }
        let half = width / 2;
        let mut output = vec![0; half];
        for y in 0..height / 2 {
            for (x, pixel) in output.iter_mut().enumerate() {
                *pixel = match vertical {
                    None => rows[2 * y * half + x],
                    Some(taps) => {
                        let sum: u32 = taps
                            .iter()
                            .enumerate()
                            .map(|(t, &w)| w * u32::from(rows[(2 * y + t).saturating_sub(2).min(height - 1) * half + x]))
                            .sum();
                        ((sum + 64) >> 7) as u8
                    }
                };
            }
            cx.ram_write(address + (y * output_stride) as u64, &output)?;
        }
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
    /// Format 1 (+08 bits 1..0, the FHD movie channel) writes a Y plane at +c0/+c4 and a
    /// half-height Cb,Cr plane at +c8/+cc, both with the +d0 stride. Without XT5_SCENE the
    /// flat source gives Y = luma and Cb = Cr = 128.
    #[test]
    fn movdc_two_plane_output_writes_luma_and_half_height_chroma_planes() {
        if fujimod_emulator_imaging::frame::host_scene().is_some() {
            return;
        }
        let mut work = Work::movdc(BASE, 1000, 100).unwrap();
        let mut host = RecordingHost {
            ram_base: 0x1_0000_0100,
            ram: vec![0x55; 0x200],
            ..Default::default()
        };
        setup(&mut work, &mut host, 0xb);
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        // 8x3, stride 12: chroma at 0x1_0000_0200 gets ceil(3 / 2) = 2 lines.
        for (o, v) in [(8, 0x101), (0x18, (2 << 16) | 7), (0xc8, 1), (0xcc, 0x200), (0xd0, 12)] {
            work.write(&mut cx, BASE + o, 4, v);
        }
        host.now = 1_000_000;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        work.advance(&mut cx);
        assert!(cx.refusal().is_none());
        for y in 0..3 {
            assert_eq!(&host.ram[16 + y * 12..16 + y * 12 + 8], &[100; 8]);
            assert_eq!(&host.ram[16 + y * 12 + 8..16 + y * 12 + 12], &[0x55; 4]);
        }
        assert!(host.ram[16 + 36..0x100].iter().all(|v| *v == 0x55));
        for y in 0..2 {
            assert_eq!(&host.ram[0x100 + y * 12..0x100 + y * 12 + 8], &[128; 8]);
            assert_eq!(&host.ram[0x100 + y * 12 + 8..0x100 + y * 12 + 12], &[0x55; 4]);
        }
        assert!(host.ram[0x100 + 24..].iter().all(|v| *v == 0x55));
        assert_eq!(work.save()["state"]["frames"], 1);
        assert!(host.irqs.contains(&(171, true)));
    }
    /// Formats 2 and 3, and format 1 without a chroma address, are refused without pixels.
    #[test]
    fn movdc_refuses_unmodeled_formats_and_a_missing_chroma_plane() {
        for (control, high, chroma, expected) in [
            (0x102, 1, 0x200, "+08 0x102"),
            (0x103, 1, 0x200, "+08 0x103"),
            (0x101, 0, 0, "chroma address"),
            (0x101, 1, 0x204, "chroma address"),
            (0x101, 8, 0x200, "+c8 0x8"),
        ] {
            let mut work = Work::movdc(BASE, 1000, 100).unwrap();
            let mut host = RecordingHost {
                ram_base: 0x1_0000_0100,
                ram: vec![0x55; 0x200],
                ..Default::default()
            };
            setup(&mut work, &mut host, 0xb);
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            for (o, v) in [(8, control), (0xc8, high), (0xcc, chroma)] {
                work.write(&mut cx, BASE + o, 4, v);
            }
            host.now = 1_000_000;
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            work.advance(&mut cx);
            let refusal = cx.refusal().unwrap();
            assert!(refusal.contains(expected), "{refusal}");
            assert!(host.ram.iter().all(|v| *v == 0x55));
            assert_eq!(work.save()["state"]["frames"], 0);
        }
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

    /// Output class 1 (+08 bits 5..4 = 1): half-size 8-bit luma, as the X-T5 4.31 HDR
    /// alignment pyramid programs it, from packed 4:2:2 and from 8-bit luma, with and without
    /// the filters, at a source and destination above 4 GiB.
    #[test]
    fn luma_half_resize_takes_every_other_luma_sample_through_the_filters() {
        const RAM: u64 = 0x1_0000_0000;
        let mut host = RecordingHost {
            ram_base: RAM,
            ram: vec![0x80; 0x4000],
            ..Default::default()
        };
        let luma = |x: usize, y: usize| (x + 8 * y) as u8;
        for y in 0..4 {
            for x in 0..32 {
                host.ram[y * 64 + x / 2 * 4 + x % 2] = luma(x, y);
                host.ram[0x1000 + y * 32 + x] = luma(x, y);
            }
        }
        let mut work = Work::resize(BASE, 1000).unwrap();
        let run = |work: &mut Work, host: &mut RecordingHost, words: &[(u64, u64)]| {
            host.ram[0x2000..0x2040].fill(0xee);
            let mut cx = Cx::new(host, 0, 0, 0);
            work.write(&mut cx, BASE + 0x5c, 4, 1);
            for &(o, v) in [
                (0x10, 1),
                (0x14, 0),
                (0x18, 64),
                (0x1c, (4 << 16) | 32),
                (0x30, 1),
                (0x34, 0x2000),
                (0x38, 24),
                (0x40, 0),
                (0x44, 0),
            ]
            .iter()
            .chain(words)
            {
                work.write(&mut cx, BASE + o, 4, v);
            }
            work.write(&mut cx, BASE + 4, 4, 1);
            host.now += 1_000_000;
            let mut cx = Cx::new(host, 0, 0, 0);
            work.advance(&mut cx);
            cx.refusal().map(str::to_owned)
        };
        let rows = |host: &RecordingHost| [0, 1].map(|y| host.ram[0x2000 + y * 24..][..16].to_vec());
        // No filter: input (2x, 2y); the destination stride gap is left alone.
        assert_eq!(run(&mut work, &mut host, &[(8, 0x10)]), None);
        assert_eq!(rows(&host), [0, 1].map(|y| (0..16).map(|x| luma(2 * x, 2 * y)).collect::<Vec<_>>()));
        assert_eq!(host.ram[0x2000 + 16], 0xee);
        // 8-bit luma input (class 1, stride 32): the same picture.
        assert_eq!(run(&mut work, &mut host, &[(8, 0x11), (0x14, 0x1000), (0x18, 32)]), None);
        assert_eq!(rows(&host), [0, 1].map(|y| (0..16).map(|x| luma(2 * x, 2 * y)).collect::<Vec<_>>()));
        // Both filters (+44 taps 0 0 64 128 64 0 0, +40 taps 0 32 64 32 0) on the ramp: exact
        // inside, the clamped top edge pulls line 0 up by 2.
        assert_eq!(run(&mut work, &mut host, &[(8, 0x1100010), (0x44, 0x40 << 8 | 0x80), (0x40, 32 << 8 | 64)]), None);
        assert_eq!(rows(&host), [2, 16].map(|base| (0..16).map(|x| (2 * x + base) as u8).collect::<Vec<_>>()));
        // A filter that is not normalized, the undecoded +3c function (bit 16) and a pixel count
        // that is not a multiple of 32 refuse.
        for words in [
            vec![(8, 0x1000010), (0x44, 0x80)],
            vec![(8, 0x10010)],
            vec![(8, 0x10), (0x1c, (4 << 16) | 48)],
        ] {
            let refusal = run(&mut work, &mut host, &words).expect("a refusal");
            assert!(refusal.contains("unsupported MOVRSZ mode"), "{refusal}");
            work = Work::resize(BASE, 1000).unwrap();
        }
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
