//! Eight-byte sensor command packets fetched by programmed DMA.
//! Completion is scheduled; stock code owns its busy polls, cleanup and retries.
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_imaging::exposure::{Exposure, ExposureHandle};
use fujimod_emulator_imaging::xtrans;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Serialize, Deserialize)]
pub struct SensorState {
    pub registers: BTreeMap<u16, u8>,
    pub commands: u64,
    pub writes: u64,
    pub reads: u64,
    #[serde(default)]
    pub image: BTreeMap<u64, u32>,
    #[serde(default)]
    pub frames: [u64; 2],
    #[serde(default)]
    pub last_dma: [(u64, u32, u32); 2],
    /// RAW DMA channels that were enabled at the start of the current frame.
    #[serde(default)]
    pub armed: [bool; 2],
    /// The last picture the still channel ([`StillChannel`]) wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub still_raw: Option<StillRaw>,
    /// Pictures the still channel wrote before [`SensorState::still_raw`], oldest first, at
    /// most [`EARLIER_STILL_RAWS`], none overlapping a later one. A multi-frame drive mode (the
    /// X-T5 4.31 HDR writes three exposures to three buffers) develops each of them after the
    /// last one was written.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub earlier_still_raws: Vec<StillRaw>,
    /// The still channel's destination was programmed and no frame has used it yet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub still_programmed: bool,
    /// Emulator-only option: stream without the register-0 standby gate (see `streaming`).
    #[serde(default)]
    pub no_standby_gate: bool,
    /// Emulator-only option: sensor reads are not gated on register 0x59 (see `set_no_read_gate`).
    #[serde(default)]
    pub no_read_gate: bool,
}
impl Default for SensorState {
    fn default() -> Self {
        // Functional reset assumption: standby until stock programming clears
        // register0 bit0. Stock sleep path330a6e0 sets this bit; wake clears it.
        Self {
            registers: BTreeMap::from([(0, 1)]),
            commands: 0,
            writes: 0,
            reads: 0,
            image: BTreeMap::new(),
            frames: [0; 2],
            last_dma: [(0, 0, 0); 2],
            armed: [false; 2],
            still_raw: None,
            earlier_still_raws: Vec::new(),
            still_programmed: false,
            no_standby_gate: false,
            no_read_gate: false,
        }
    }
}
impl SensorState {
    /// Make `picture` the last still channel picture; the previous one joins the earlier ones,
    /// which drop any picture it overlaps and keep the newest [`EARLIER_STILL_RAWS`].
    pub fn record_still_raw(&mut self, picture: StillRaw) {
        if let Some(previous) = self.still_raw.take() {
            self.earlier_still_raws.push(previous);
        }
        self.earlier_still_raws.retain(|r| !r.overlaps(&picture));
        let excess = self
            .earlier_still_raws
            .len()
            .saturating_sub(EARLIER_STILL_RAWS);
        self.earlier_still_raws.drain(..excess);
        self.still_raw = Some(picture);
    }
}
#[derive(Clone, Copy)]
pub struct ImageRoutes {
    pub dma: Option<[u32; 2]>,
    pub timing: [u32; 2],
    /// Each DMA channel's +0x18 word is its line stride in bytes; otherwise
    /// lines are packed (width x 2 bytes).
    pub line_stride: bool,
    /// An interrupt line pulsed at every frame start, with no enable or status
    /// word in the window (see [`Sensor::timing`]).
    pub frame_start_pulse: Option<u32>,
}
/// A RAW DMA channel that writes the still scene (opt-in, [`Sensor::with_still_channel`]).
///
/// X-T5 4.31 RAW DMA unit 3 is channel 1 of the 0xff948000 block (setters 0x1351560,
/// 0x1352834; unit 2 is channel 0). Its words, relative to the window:
///
/// * +0xc0 bit 0 enable, sampled at each frame start as for every channel.
/// * +0xc4 = start << 16 | samples a line, +0xc8 = start << 16 | lines (0x1351560 writes
///   u16 [+2] / [+6] in bits 15..0 and [+0] / [+4] in bits 30..16, counts at most 0x4000). The
///   horizontal/vertical roles follow the X-T3 channel (+0x44 samples, +0x48 lines); the start
///   fields select the crop of the sensor frame, which a scene does not have, so they are kept
///   but not applied.
/// * +0xcc bits 2..0 / +0xd0: destination address bits 34..32 / 31..0.
/// * +0xd4 bits 2..0 / +0xd8: line pitch in bytes (high / low; the high word must be 0).
/// * +0xdc bits 18..16: format code 0..5 (from the job config, 0x1352834); bits 15..0 a byte of
///   the config, not decoded. The pitch selects the sample layout: from 1.5 up to (not
///   including) 2 bytes a sample it is packed 12-bit lines, as on the live-view channels (two
///   samples in three bytes, [`fujimod_emulator_imaging::xtrans::pack12_line`]), whatever the
///   code; the log line names the code. Otherwise only the codes in `sixteen_bit_codes` are
///   modelled, as 14-bit samples in 16-bit little-endian containers; the others refuse.
///   X-T5 4.31 still capture programs code 5 with pitch 0x3d80 for 7776 samples (7872 x 2
///   bytes, an aligned 16-bit stride); after a MOVIE -> STILL mode switch it programs pitch
///   0x2d90 (11664 = 7776 x 1.5 bytes) for the same 7776 samples: packed 12-bit.
/// * +0xe0 interrupt enable, +0xe4 status (write one to clear), INTID 163. The ISR 0x1350d94
///   handles bit 1 and bit 0 alike (both run the unit's frame callback).
///
/// Arming. The channel stays enabled through live view (+0xc0 bit 0, 0x13514dc), so the
/// enable alone does not mark the still frame. A capture programs the unit through
/// 0x2c69fa4: 0x1352834 (call 0x2c6a068) writes +0xdc, +0xcc/+0xd0 and +0xd4/+0xd8, then
/// 0x13522dc (call 0x2c6a094) claims the unit, clears +0xe4 (0x1350c6c, 0xf) and, with a
/// frame callback, enables the frame interrupts in +0xe0 (0x1350f6c at 0x1352420: bits 0, 2,
/// 3, and bit 1 when +0xf4 bit 0 is set); without one it clears them (0x135238c); 0x13514dc
/// then sets the enable (0x2c6b1a8). The model therefore writes a frame only when all hold at
/// its end: the frame began enabled, +0xd0 was written since the last written frame, and
/// +0xe0 enables bit 0 or bit 1. It then writes the [`StillChannel::scene`] once, line by
/// line as X-Trans samples, site (0, 0) at the destination
/// ([`fujimod_emulator_imaging::xtrans::mosaic_line`]), sets +0xe4 bit 1 (bit 0 when +0xe0
/// enables only bit 0) and raises the line while +0xe0 enables it. Other frames write
/// nothing and set no status; the next +0xd0 write arms the channel again.
/// +0xf0 (line-count interrupt) is not modelled.
///
/// Levels. The samples follow the X-T5 sensor response
/// ([`fujimod_emulator_imaging::xtrans::Response::xt5`]): black 1024, the colour gains of the
/// X-T5 daylight white balance, and the exposure of the sensor's [`ExposureHandle`]
/// ([`Sensor::exposure`]) at the end of the frame, which the picture keeps
/// ([`StillRaw::exposure`]) so each frame of a multi-frame capture develops with its own.
/// With `optical_black`, a 16-bit frame larger than twice the margins holds the scene only
/// in its active area: the first `optical_black.0` samples of every line and the first
/// `optical_black.1` lines are optical black (1024), and the scene is rendered at the size of
/// the rest, site (0, 0) still at the destination.
#[derive(Clone, Copy)]
pub struct StillChannel {
    pub channel: usize,
    /// Bit n set: format code n is 16-bit containers.
    pub sixteen_bit_codes: u8,
    /// The picture the sensor sees.
    pub scene: fn() -> &'static fujimod_emulator_imaging::frame::Scene,
    /// Optical black margins (samples on the left, lines on top) of a 16-bit frame. X-T5
    /// 4.31: still capture writes 7776x5250 and RIFP develops 7752x5184 from sample 24 of line
    /// 66 (input 0xbb10d1e0 in the RAW at 0xbb00f6b0, pitch 0x3d80), so (24, 66).
    pub optical_black: (u32, u32),
}
/// Where the still channel wrote its last picture.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StillRaw {
    pub address: u64,
    pub pitch: u64,
    pub width: u32,
    pub lines: u32,
    /// Packed 12-bit lines (two samples in three bytes) instead of 16-bit containers.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub packed12: bool,
    /// X-Trans tile site (line, sample) of the first sample: (0, 0) for a still channel
    /// picture; a RIFP RAW-to-RAW copy of a crop keeps the crop's site.
    #[serde(default, skip_serializing_if = "is_origin")]
    pub phase: (u8, u8),
    /// Exposure relative to the reference ([`Exposure::relative`]) the samples were taken at.
    #[serde(default = "unit_exposure")]
    pub exposure: f64,
}
fn is_origin(phase: &(u8, u8)) -> bool {
    *phase == (0, 0)
}
fn unit_exposure() -> f64 {
    1.
}
impl Default for StillRaw {
    fn default() -> Self {
        Self {
            address: 0,
            pitch: 0,
            width: 0,
            lines: 0,
            packed12: false,
            phase: (0, 0),
            exposure: 1.,
        }
    }
}
/// Still channel pictures kept besides the last one ([`SensorState::earlier_still_raws`]).
pub const EARLIER_STILL_RAWS: usize = 7;
impl StillRaw {
    /// The bytes from the first line's start to the last line's start plus one pitch.
    fn end(&self) -> u64 {
        self.address + self.pitch * u64::from(self.lines)
    }
    /// Whether a reader with line pitch `pitch` starting at `address` is inside this picture.
    pub fn contains(&self, address: u64, pitch: u64) -> bool {
        self.pitch == pitch && address >= self.address && address < self.end()
    }
    fn overlaps(&self, other: &StillRaw) -> bool {
        self.address < other.end() && other.address < self.end()
    }
}
/// The frame length of one decoded sensor readout (opt-in, [`Sensor::with_frame_timing`]).
///
/// X-T5 4.31: the S1 contrast AF switches the sensor to a fast readout (firmware readout mode
/// 0x113, sensor register 0x01 = 0x07) and plans its scan in frames of that readout: the AF
/// frame period [0x44a37d8] comes from 0x3032cfc, which takes the mode's row of the table at
/// 0x36d9fb8 (row 96: 386 lines of 432 clocks at 72 MHz = 6 us a line, 2316 us) and lengthens
/// it for the exposure (2593 us = 432 lines). The sensor's frame-length register (0x18 low,
/// 0x19 high) counts line pairs less two: 191 for the 386-line base frame, then 214 for the
/// 432-line frame. Frames of that readout therefore last (value + 2) x 2 lines of 6 us. The
/// other readouts' line lengths are not decoded; their frames keep the device's stand-in
/// period.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameTiming {
    /// The value of sensor register 0x01 that selects the decoded readout.
    pub readout: u8,
    /// The length of one line of that readout in ticks (nanoseconds).
    pub line_ticks: u64,
}
#[derive(Clone)]
pub struct Sensor(
    pub Arc<Mutex<SensorState>>,
    Option<ImageRoutes>,
    (u16, u8),
    Option<StillChannel>,
    ExposureHandle,
    Option<FrameTiming>,
);
impl Sensor {
    pub fn new(routes: Option<ImageRoutes>, read_gate: (u16, u8)) -> Self {
        Self(
            Arc::default(),
            routes,
            read_gate,
            None,
            ExposureHandle::default(),
            None,
        )
    }
    /// Let the frame length follow the sensor's registers in one decoded readout (see
    /// [`FrameTiming`]). Set it before the sensor is cloned into its devices.
    pub fn with_frame_timing(mut self, timing: FrameTiming) -> Self {
        self.5 = Some(timing);
        self
    }
    /// The length of the current frame in ticks: in the decoded readout ([`FrameTiming`]) the
    /// programmed frame length, otherwise `stand_in` (the device's configured period).
    pub fn frame_ticks(&self, stand_in: u64) -> u64 {
        let Some(timing) = self.5 else {
            return stand_in;
        };
        let s = self.0.lock().unwrap();
        let r = |a: u16| u64::from(s.registers.get(&a).copied().unwrap_or(0));
        if r(0x01) != u64::from(timing.readout) {
            return stand_in;
        }
        let pairs = r(0x18) | r(0x19) << 8;
        (pairs + 2) * 2 * timing.line_ticks
    }
    /// The exposure the sensor's programming selects ([`Command`] updates it from the gain
    /// registers, see [`exposure_of`]); shared with every clone, so other models (statistics,
    /// live view) can read it.
    pub fn exposure(&self) -> ExposureHandle {
        self.4.clone()
    }
    /// The exposure relative to the reference ([`Exposure::relative`]).
    pub fn relative_exposure(&self) -> f64 {
        self.4.lock().map_or(1., |e| e.relative())
    }
    /// Record a picture another block derived from a still channel picture (a RIFP RAW-to-RAW
    /// copy), so later readers of its address develop it like a still channel picture.
    pub fn record_derived_raw(&self, picture: StillRaw) {
        self.0.lock().unwrap().record_still_raw(picture);
    }
    /// Make one RAW DMA channel the still channel (see [`StillChannel`]). Set it before the
    /// sensor is cloned into its devices.
    pub fn with_still_channel(mut self, still: StillChannel) -> Self {
        self.3 = Some(still);
        self
    }
    /// The last picture the still channel wrote, if any.
    pub fn still_raw(&self) -> Option<StillRaw> {
        self.0.lock().unwrap().still_raw
    }
    /// The latest still channel picture (the last one, then the earlier ones) whose pitch is
    /// `pitch` and whose lines hold `address`; otherwise the last picture, as
    /// [`Sensor::still_raw`].
    pub fn still_raw_at(&self, address: u64, pitch: u64) -> Option<StillRaw> {
        let s = self.0.lock().unwrap();
        s.still_raw
            .iter()
            .chain(s.earlier_still_raws.iter().rev())
            .find(|r| r.contains(address, pitch))
            .copied()
            .or(s.still_raw)
    }
    pub fn image_reg(&self, offset: u64) -> u32 {
        self.0
            .lock()
            .unwrap()
            .image
            .get(&offset)
            .copied()
            .unwrap_or(0)
    }
    pub fn image_write(&self, cx: &mut Cx<'_>, offset: u64, value: u32) {
        let mut s = self.0.lock().unwrap();
        if matches!(offset, 0x64 | 0xe4 | 0x144) {
            let old = s.image.get(&offset).copied().unwrap_or(0);
            s.image.insert(offset, old & !(value & 15));
        } else {
            s.image.insert(offset, value);
        }
        if self
            .3
            .is_some_and(|still| offset == [0x40, 0xc0][still.channel] + 0x10)
        {
            s.still_programmed = true;
        }
        let Some(routes) = self.1 else { return };
        if let Some(dma) = routes.dma {
            for (enable, pending, irq) in [(0x60, 0x64, dma[0]), (0xe0, 0xe4, dma[1])] {
                cx.set_irq(
                    irq,
                    s.image.get(&enable).copied().unwrap_or(0)
                        & s.image.get(&pending).copied().unwrap_or(0)
                        & 15
                        != 0,
                );
            }
        }
        let flags =
            s.image.get(&0x140).copied().unwrap_or(0) & s.image.get(&0x144).copied().unwrap_or(0);
        cx.set_irq(routes.timing[0], flags & 3 != 0);
        cx.set_irq(routes.timing[1], flags & 12 != 0);
    }
    pub fn timing(&self, cx: &mut Cx<'_>, start: bool) {
        let Some(routes) = self.1 else { return };
        let mut s = self.0.lock().unwrap();
        // Global sensor timing remains active even without a RAW DMA buffer.
        // 1604184(event1) ->IRQ166 ->1603d98 ->imgdc32c6bd8/15ae.
        let pending = s.image.get(&0x144).copied().unwrap_or(0) | if start { 3 } else { 12 };
        s.image.insert(0x144, pending);
        let enabled = s.image.get(&0x140).copied().unwrap_or(0);
        cx.set_irq(routes.timing[0], pending & enabled & 3 != 0);
        cx.set_irq(routes.timing[1], pending & enabled & 12 != 0);
        // X-T5 4.31: the third vertical-timing line (INTID 165, logical 0x77, VD
        // callback service id 2 of 0x1530748) has no bit in +0x140/+0x144 (the
        // service ISRs 0x15304a0/0x153050c ack only bits 0..2) and its callbacks
        // (0x2ffafa8, 0x3007764, 0x303c9f0) ack nothing, so it is modelled as an
        // edge at every frame start. Its source is INFERRED from the neighbouring
        // CCDTG lines 164/166, not decoded from a register.
        if start && let Some(irq) = routes.frame_start_pulse {
            cx.set_irq(irq, true);
            cx.set_irq(irq, false);
        }
    }
    /// Frame start: a RAW DMA channel samples its enable bit as a frame
    /// begins and transfers exactly the frames that began enabled. A channel
    /// enabled mid-frame waits for the next frame start, and one disabled
    /// mid-frame still completes the frame in progress.
    /// X-T3 5.11 basis: the still handler (imgdc state 4, on the message
    /// from the frame-start interrupt GIC 102) enables channel 0 during a
    /// frame. State 5 then expects the next frame-start message (MOVDC stop
    /// request, RAW stop request, which clears the enable) before the
    /// channel's completion message ends the still. The sampling point is
    /// inferred from that sequence, not measured.
    pub fn frame_start(&self, channel: usize) {
        if self.1.and_then(|r| r.dma).is_none() {
            return;
        }
        let mut s = self.0.lock().unwrap();
        let base = [0x40, 0xc0][channel];
        s.armed[channel] = s.image.get(&base).copied().unwrap_or(0) & 1 != 0;
    }
    pub fn frame(&self, cx: &mut Cx<'_>, channel: usize) -> Result<(), String> {
        let Some(routes) = self.1 else { return Ok(()) };
        let Some(dma) = routes.dma else { return Ok(()) };
        if let Some(still) = self.3.filter(|still| still.channel == channel) {
            return self.still_frame(cx, still, dma[channel]);
        }
        let mut s = self.0.lock().unwrap();
        let base = [0x40, 0xc0][channel];
        let armed = std::mem::take(&mut s.armed[channel]);
        let reg = |o| s.image.get(&o).copied().unwrap_or(0);
        if !armed {
            return Ok(());
        }
        let address = u64::from(reg(base + 0x10)) | (u64::from(reg(base + 0xc) & 7) << 32);
        let width = reg(base + 4) & 0x7fff;
        let height = reg(base + 8) & 0x7fff;
        // No framebuffer override: shape and destination come from143c828/dafc.
        if address == 0 || width == 0 || height == 0 {
            return Ok(());
        }
        let stride = if routes.line_stride {
            u64::from(reg(base + 0x18))
        } else {
            u64::from(width) * 2
        };
        // A programmed stride of exactly 1.5 bytes a sample is a packed 12-bit line (X-T5 4.31
        // movie live view: unit 3 at 7776 samples with pitch 11664). Writing 16-bit lines there
        // would overrun the guest buffer by a third, so such lines are packed instead.
        // Movie record readout uses the same packing with padded lines (6308 samples, pitch
        // 0x2520 = 9504 >= 9462 bytes), so any stride from 1.5 up to (not including) 2 bytes a
        // sample is a padded packed 12-bit line.
        let packed12 = routes.line_stride
            && width % 2 == 0
            && stride * 2 >= u64::from(width) * 3
            && stride < u64::from(width) * 2;
        let line = if packed12 {
            u64::from(width) * 3 / 2
        } else {
            u64::from(width) * 2
        };
        if stride < line || (!packed12 && stride % 2 != 0) {
            return Err(format!(
                "RAW DMA line stride {stride:#x} does not hold {width} 16-bit or packed 12-bit samples"
            ));
        }
        let size = stride * u64::from(height - 1) + line;
        if size > 128 * 1024 * 1024 {
            return Err("receiver frame exceeds functional DMA limit".into());
        }
        let frame = s.frames[channel];
        let mut bytes = vec![0u8; line as usize];
        for y in 0..height {
            ramp_line(y, frame, packed12, &mut bytes);
            cx.ram_write(address + stride * u64::from(y), &bytes)?;
        }
        s.frames[channel] += 1;
        s.last_dma[channel] = (address, width, height);
        let pending = base + 0x24;
        let value = s.image.get(&pending).copied().unwrap_or(0) | 1;
        s.image.insert(pending, value);
        let enabled = s.image.get(&(base + 0x20)).copied().unwrap_or(0);
        cx.set_irq(dma[channel], value & enabled & 15 != 0);
        Ok(())
    }
    /// The still channel's frame (see [`StillChannel`]).
    fn still_frame(&self, cx: &mut Cx<'_>, still: StillChannel, irq: u32) -> Result<(), String> {
        let mut s = self.0.lock().unwrap();
        let base = [0x40, 0xc0][still.channel];
        if !std::mem::take(&mut s.armed[still.channel]) || !s.still_programmed {
            return Ok(());
        }
        let reg = |o: u64| s.image.get(&(base + o)).copied().unwrap_or(0);
        let done = match reg(0x20) & 3 {
            0 => return Ok(()),
            1 => 1,
            _ => 2,
        };
        let address = u64::from(reg(0x10)) | (u64::from(reg(0x0c) & 7) << 32);
        let (width, lines) = (reg(0x04) & 0x7fff, reg(0x08) & 0x7fff);
        if address == 0 || width == 0 || lines == 0 {
            return Ok(());
        }
        let format = reg(0x1c);
        let code = format >> 16 & 7;
        let pitch = u64::from(reg(0x18));
        // As on the live-view channels: a pitch from 1.5 up to (not including) 2 bytes a sample
        // holds packed 12-bit lines.
        let packed12 =
            reg(0x14) & 7 == 0 && pitch * 2 >= u64::from(width) * 3 && pitch < u64::from(width) * 2;
        if !packed12 && still.sixteen_bit_codes >> code & 1 == 0 {
            return Err(format!(
                "still RAW DMA channel {}: format code {code} (+{:#x} = {format:#x}) is not decoded; \
                 only codes {:#010b} (bit n = code n, 16-bit containers) are modelled. If code {code} is 16-bit, \
                 set XT5_STILL_RAW16={code}",
                still.channel,
                base + 0x1c,
                still.sixteen_bit_codes
            ));
        }
        let samples_bytes = width as usize * 2;
        let line = if packed12 {
            fujimod_emulator_imaging::xtrans::packed12_bytes(width as usize) as u64
        } else {
            samples_bytes as u64
        };
        if reg(0x14) & 7 != 0 || pitch < line || (!packed12 && pitch % 2 != 0) {
            return Err(format!(
                "still RAW DMA channel {}: pitch {:#x}:{pitch:#x} does not hold {width} 16-bit \
                 or packed 12-bit samples (+{:#x} = {:#x}, +{:#x} = {:#x}, format code {code})",
                still.channel,
                reg(0x14),
                base + 4,
                reg(0x04),
                base + 8,
                reg(0x08)
            ));
        }
        if pitch * u64::from(lines - 1) + line > 128 * 1024 * 1024 {
            return Err("still RAW frame exceeds functional DMA limit".into());
        }
        let exposure = self.relative_exposure();
        let lut = xtrans::Response::xt5(exposure).mosaic_lut();
        let (left, top) = match still.optical_black {
            (left, top) if !packed12 && width > 2 * left && lines > 2 * top => {
                (left as usize, top as usize)
            }
            _ => (0, 0),
        };
        let mut samples = vec![0u8; samples_bytes];
        let black = xtrans::BLACK.to_le_bytes().repeat(width as usize);
        for y in 0..top {
            cx.ram_write(address + pitch * y as u64, &black)?;
        }
        samples[..2 * left].copy_from_slice(&black[..2 * left]);
        let mut packed = vec![0u8; if packed12 { line as usize } else { 0 }];
        fujimod_emulator_imaging::frame::render_rgb_lines(
            (still.scene)(),
            width as usize - left,
            lines as usize - top,
            |y, rgb| {
                let y = y + top;
                xtrans::mosaic_line_with(rgb, (y, left), &mut samples[2 * left..], &lut);
                let bytes = if packed12 {
                    xtrans::pack12_line(&samples, &mut packed);
                    &packed
                } else {
                    &samples
                };
                cx.ram_write(address + pitch * y as u64, bytes)
            },
        )?;
        s.still_programmed = false;
        s.frames[still.channel] += 1;
        s.last_dma[still.channel] = (address, width, lines);
        let picture = StillRaw {
            address,
            pitch,
            width,
            lines,
            packed12,
            phase: (0, 0),
            exposure,
        };
        s.record_still_raw(picture);
        let status = base + 0x24;
        let value = s.image.get(&status).copied().unwrap_or(0) | done;
        s.image.insert(status, value);
        let enabled = s.image.get(&(base + 0x20)).copied().unwrap_or(0);
        cx.set_irq(irq, value & enabled & 15 != 0);
        cx.log(&format!(
            "still RAW DMA channel {}: scene {width}x{lines} X-Trans {} at {address:#x} pitch {pitch:#x} \
             (format code {code}), exposure {exposure:.3} x reference, optical black {left} samples / {top} lines",
            still.channel,
            if packed12 { "packed 12-bit" } else { "16-bit" }
        ));
        Ok(())
    }
    fn image_supported(&self, offset: u64) -> bool {
        self.1
            .is_some_and(|r| matches!(offset, 0x4..=0x34 | 0x140 | 0x144) || r.dma.is_some())
    }
    /// X-H2S option: the X100VI-derived register-0 standby bit is not a modeled gate here. The
    /// X-H2S never writes sensor register 0 (observed on the stock boot), so the receiver streams
    /// once the sensor has been programmed (`writes != 0`). Labelled assumption, not a measurement.
    pub fn set_no_standby_gate(&self, on: bool) {
        self.0.lock().unwrap().no_standby_gate = on;
    }
    /// X-H2S option: the X100VI's register-0x59 read gate is not a modeled gate here. The X-H2S
    /// reads sensor registers without having written 0x59 bit 2 (observed: the strict run refuses
    /// the first read). Reads then return the stored register value, zero if never written.
    /// Labelled assumption, not a measurement.
    pub fn set_no_read_gate(&self, on: bool) {
        self.0.lock().unwrap().no_read_gate = on;
    }
    pub fn streaming(&self) -> bool {
        let s = self.0.lock().unwrap();
        s.writes != 0 && (s.no_standby_gate || s.registers.get(&0).copied().unwrap_or(1) & 1 == 0)
    }
}
#[derive(Default, Serialize, Deserialize)]
struct State {
    registers: BTreeMap<u64, u32>,
    response: [u32; 8],
    generation: u64,
    pending: Option<Vec<[u8; 8]>>,
    deadline: Option<u64>,
    /// Trigger channels 0x280 + n * 0x40 (n < 6): the busy-until deadline of
    /// each channel whose start bit (+4) is set.
    #[serde(default)]
    channels: BTreeMap<u64, u64>,
}
/// Trigger channel blocks: X-T5 4.31 programs 0x288 (0x100), 0x28c (a count),
/// 0x290/0x294 (1) and starts the channel with 0x284 = 1, then its REC task
/// polls 0x284 bit 0 (p5 0x2eb7bc0 loop through image 4 0x1356ae0) until it
/// clears before it continues the live-view start; the same for 0x2c0. The
/// blocks have the shape of the 0x200 command block. Only the start/busy bit
/// is modeled: it clears after the command latency. What the channels drive
/// on the sensor (likely trigger/sync lines) is unknown.
const CHANNEL_BASE: u64 = 0x280;
/// Line `y` of frame `frame` of the explicit RAW14 moving ramp (the normal REC14 profile) into
/// `out`: sample `x` is `(31 x + 17 y + 131 frame) mod 2^14`. `out` holds the whole line: two
/// bytes a sample (16-bit little-endian containers) or, with `packed12`, three bytes for each
/// pair of samples (the ramp's top 12 bits, little-endian). Only the low 14 bits of each term
/// matter, so the line offset is reduced once and each sample is computed from its index alone
/// in 32-bit arithmetic, which the compiler vectorizes. [`Sensor::frame`] writes every movie
/// live-view line through here (7776 samples a line, every frame): about 2.5 ms a frame
/// instead of 9-12 ms with per-sample `Vec` pushes.
fn ramp_line(y: u32, frame: u64, packed12: bool, out: &mut [u8]) {
    let offset = (u64::from(y)
        .wrapping_mul(17)
        .wrapping_add(frame.wrapping_mul(131))
        & 0x3fff) as u32;
    let sample = |x: usize| (offset.wrapping_add((x as u32).wrapping_mul(31)) & 0x3fff) as u16;
    if packed12 {
        for (pair_index, pair) in out.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let (a, b) = (sample(2 * pair_index) >> 2, sample(2 * pair_index + 1) >> 2);
            pair[0] = a as u8;
            pair[1] = (a >> 8) as u8 | (b << 4) as u8;
            pair[2] = (b >> 4) as u8;
        }
    } else {
        for (x, container) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let [low, high] = sample(x).to_le_bytes();
            container[0] = low;
            container[1] = high;
        }
    }
}
const CHANNEL_STRIDE: u64 = 0x40;
const CHANNEL_COUNT: u64 = 6;
const CHANNEL_TOKEN: u64 = 1 << 62;
/// Input buffer configuration: two eight-word blocks at 0x440 and 0x460.
/// X-T5 4.31 writes each block in order through one setter (0x1354dd8 for
/// 0x440, 0x135529c for 0x460, both called from 0x2eb89e4) after a range check
/// that ends in "IBUF MAX ERROR."; matching getters (0x1355760, 0x13558ac) read
/// the eight words back with the same field masks. Word 0 holds bit 0 (enable),
/// bit 8 (mode) and a 15-bit value at bit 16; words 1..7 hold two 9-bit values
/// at bits 0 and 16. The words are stored and read back; nothing polls them and
/// what they configure is unknown. Bits outside these fields are refused.
const INPUT_BUFFER: std::ops::RangeInclusive<u64> = 0x440..=0x47c;
fn input_buffer_mask(o: u64) -> u32 {
    if o & 0x1f == 0 {
        0x7fff_0101
    } else {
        0x01ff_01ff
    }
}
/// Input buffer size words at 0x480 and 0x4c0 and their flag words at 0x484 and 0x4c4 (X100VI 1.31;
/// the same eight-word blocks' neighbors). The setters (0x143fa44, 0x143f2ac, 0x143f678) each read
/// the word once, replace bit 0 with a boolean and bits 16 to 26 with a size of at most 0x400
/// (else "IBUF MAX ERROR.") and write it back; the getter 0x143fb58 reads it once. 0x143fbec reads
/// bits 0 and 1 of the flag words; no writer of them is known. Nothing polls any of these words and
/// what they configure is unknown. The power-on values are unverified; they read 0 here.
const INPUT_BUFFER_SIZE_WORDS: [u64; 2] = [0x480, 0x4c0];
const INPUT_BUFFER_FLAG_WORDS: [u64; 2] = [0x484, 0x4c4];
/// Enable word at 0x120 (X100VI 1.31): the setter 0x143e754 writes a boolean (0 or 1, else
/// 0xffffffe2) with one plain write and the getter 0x143e7b4 reads it once and keeps bit 0; no
/// other function touches it and nothing polls it. What it enables is unknown, and the power-on
/// value is unverified; it reads 0 here.
const ENABLE_WORD: u64 = 0x120;
fn plain_word_mask(o: u64) -> Option<u32> {
    if o == ENABLE_WORD {
        Some(0x1)
    } else if INPUT_BUFFER_SIZE_WORDS.contains(&o) {
        Some(0x07ff_0001)
    } else if INPUT_BUFFER_FLAG_WORDS.contains(&o) {
        Some(0x3)
    } else {
        None
    }
}
fn channel(o: u64) -> Option<u64> {
    (CHANNEL_BASE..CHANNEL_BASE + CHANNEL_STRIDE * CHANNEL_COUNT)
        .contains(&o)
        .then(|| (o - CHANNEL_BASE) / CHANNEL_STRIDE * CHANNEL_STRIDE + CHANNEL_BASE)
}
/// Analog gain of a 12-bit gain code: 4095 / (4095 - code), the law of the X-Trans sensor
/// family (fujimod `emulator-parts-sensor` xtrans16). `None` for a code of 4095 or more.
pub fn analog_gain(code: u16) -> Option<f64> {
    (code < 4095).then(|| 4095. / f64::from(4095 - code))
}

/// The two 12-bit gain codes of the X-T5 4.31 sensor: registers 0x38 (bits 7..0) / 0x39
/// (bits 11..8) and 0x3a / 0x3b. Stock 0x3029b58 writes them in that order (codes from
/// 0x302e1dc), then 0xbb, 0x37 and flushes.
pub fn gain_codes(registers: &BTreeMap<u16, u8>) -> [u16; 2] {
    let r = |a: u16| u16::from(registers.get(&a).copied().unwrap_or(0));
    [
        r(0x38) | (r(0x39) & 0xf) << 8,
        r(0x3a) | (r(0x3b) & 0xf) << 8,
    ]
}

/// The exposure the sensor registers select. Gain: the first code of [`gain_codes`] (a code
/// of 0 is unity gain). Shutter: not decoded (candidates are registers 0x29..0x2e and the
/// live AE APEX words; see [`Command`]), so the integration time is the reference's
/// ([`Exposure::REFERENCE`]): the exposure follows the gain alone.
pub fn exposure_of(registers: &BTreeMap<u16, u8>) -> Exposure {
    let [code, _] = gain_codes(registers);
    Exposure {
        seconds: Exposure::REFERENCE.seconds,
        analog_gain: analog_gain(code).unwrap_or(1.),
    }
}

/// Guest address of the live AE APEX words (0x4b73ff0 + 0x540..+0x54c, stored by
/// 0x300df88..0x300dfb4), a shutter candidate that [`Command`] logs.
const AE_APEX: u64 = 0x4b73ff0 + 0x540;
/// Lines of `sensor exposure:` before the log goes quiet.
const EXPOSURE_LOG_LIMIT: u32 = 200;

/// Sensor unit configuration block (XP5). X-H2S 7.30 programs a config block
/// at 0x400..0x42c (writer 0x1678a00); the words are latched: a read returns
/// the last write, and what they drive on the sensor is unknown. The same
/// firmware's table blocks at 0x440..0x47c (set 0x1675eac, reset 0x1676370)
/// and per-unit control words at 0x480 (unit 2) and 0x4c0 (unit 3; bit 0
/// enable, bits 16..26 a count of at most 0x400, read-modify-written by
/// 0x167538c and read back by 0x16754a0) are the input buffer words above.
fn unit_latch(o: u64) -> bool {
    matches!(o, 0x400..=0x42c)
}
pub struct Command {
    base: u64,
    state: State,
    pub sensor: Sensor,
    latency_ticks: u64,
    retain_unmodeled: bool,
    /// End (exclusive) of the window offsets that retain words when `retain_unmodeled` is set.
    retain_limit: u64,
    /// The last `sensor exposure:` line's content and the lines logged so far.
    exposure_log: (String, u32),
}
impl Command {
    pub fn new(base: u64, sensor: Sensor, latency_ticks: u64) -> Result<Self, String> {
        if base & 0xfff != 0 || latency_ticks == 0 {
            return Err("sensor command needs aligned base and positive latency".into());
        }
        Ok(Self {
            base,
            state: State::default(),
            sensor,
            latency_ticks,
            retain_unmodeled: false,
            retain_limit: 0x244,
            exposure_log: (String::new(), 0),
        })
    }
    /// Set the sensor's exposure ([`Sensor::exposure`]) from its registers ([`exposure_of`])
    /// and, when the gain codes, the shutter candidate registers (0x29..0x2e), the frame
    /// timing registers (0x16..0x19) or the live AE APEX words changed, log them as
    /// `sensor exposure: ...` so a run shows which candidate follows the AE.
    fn update_exposure(&mut self, cx: &mut Cx<'_>, registers: &BTreeMap<u16, u8>) {
        let exposure = exposure_of(registers);
        if let Ok(mut handle) = self.sensor.4.lock() {
            *handle = exposure;
        }
        let r = |a: u16| registers.get(&a).copied().unwrap_or(0);
        let hex = |range: std::ops::RangeInclusive<u16>| {
            range
                .map(|a| format!("{:02x}", r(a)))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let apex = cx.ram_bytes(AE_APEX, 12).map_or_else(
            |_| "unreadable".to_string(),
            |b| {
                b.iter()
                    .map(|v| format!("{v:02x}"))
                    .collect::<Vec<_>>()
                    .join("")
            },
        );
        let [first, second] = gain_codes(registers);
        let line = format!(
            "gain codes 0x38/0x39 {first:#05x} 0x3a/0x3b {second:#05x} -> analog gain {:.3}, \
             E/E_ref {:.3} (shutter not decoded; candidates 0x29..0x2e = {}, frame 0x16..0x19 = {}, \
             AE APEX [{AE_APEX:#x}..+0xc] = {apex})",
            exposure.analog_gain,
            exposure.relative(),
            hex(0x29..=0x2e),
            hex(0x16..=0x19),
        );
        if line == self.exposure_log.0 {
            return;
        }
        self.exposure_log.1 = self.exposure_log.1.saturating_add(1);
        if self.exposure_log.1 <= EXPOSURE_LOG_LIMIT {
            cx.log(&format!("sensor exposure: {line}"));
        }
        if self.exposure_log.1 == EXPOSURE_LOG_LIMIT {
            cx.log("sensor exposure: further changes are not logged");
        }
        self.exposure_log.0 = line;
    }
    /// Let words of the window that have no modeled meaning read back their last write
    /// (zero before one) instead of failing the access.
    pub fn with_retained_words(mut self) -> Self {
        self.retain_unmodeled = true;
        self
    }
    /// Like `with_retained_words`, for every unmodeled word of the 4 KiB window. The X-T5 4.31
    /// application (0x1354f00..) writes a table of 9-bit coordinate pairs to +0x440..+0x484 and
    /// uses +0x4c0..+0x4e8; their meaning is unknown, so they only read back their last write.
    pub fn with_retained_window(mut self) -> Self {
        self.retain_unmodeled = true;
        self.retain_limit = 0x1000;
        self
    }
    fn reg(&self, o: u64) -> u32 {
        self.state.registers.get(&o).copied().unwrap_or(0)
    }
    fn cancel(&mut self) {
        self.state.generation = self.state.generation.wrapping_add(1);
        self.state.pending = None;
        self.state.deadline = None;
        self.state.registers.insert(0x204, 0);
    }
    fn submit(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if self.state.pending.is_some() {
            return Err("sensor command submitted while busy".into());
        }
        let format = self.reg(0x208);
        if format & 0x7fffff00 != 0x10400
            || self.reg(0x21c) != 0
            || self.reg(0x214) != 0
            || self.reg(0x218) != 8
            || self.reg(0x20c) > 7
        {
            return Err("unsupported sensor command DMA descriptor".into());
        }
        let source = u64::from(self.reg(0x210)) | (u64::from(self.reg(0x20c)) << 32);
        let count = match format & 255 {
            0 => return Err("zero sensor packet count is unsupported".into()),
            n => n as usize,
        };
        let bytes = cx.ram_bytes(source, count * 8)?;
        let mut packets = Vec::with_capacity(count);
        for bytes in bytes.as_chunks::<8>().0 {
            let packet: [u8; 8] = *bytes;
            if !matches!(packet[0], 0x81 | 0x82) {
                return Err(format!("unsupported sensor opcode {:02x}", packet[0]));
            }
            packets.push(packet);
        }
        let duration = self
            .latency_ticks
            .checked_mul(count as u64)
            .ok_or("sensor command duration overflow")?;
        let at = cx
            .now()
            .checked_add(duration)
            .ok_or("sensor command deadline overflow")?;
        self.state.pending = Some(packets);
        self.state.registers.insert(0x204, 1);
        self.state.generation = self.state.generation.wrapping_add(1);
        self.state.deadline = Some(at);
        cx.schedule(at, self.state.generation);
        Ok(())
    }
}
impl Device for Command {
    fn name(&self) -> &'static str {
        "imaging-sensor"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let o = address.wrapping_sub(self.base);
        self.advance(cx);
        if size != 4 || o & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        match o {
            0 => u64::from(self.reg(0) | 1),
            0x4..=0x34 | 0x40..=0x94 | 0xc0..=0x114 | 0x140 | 0x144 => {
                if !self.sensor.image_supported(o) {
                    cx.unmodeled();
                    return 0;
                }
                u64::from(self.sensor.image_reg(o))
            }
            0x180..=0x1b0 | 0x200 | 0x204 | 0x208 | 0x20c | 0x210 | 0x214 | 0x218 | 0x21c => {
                u64::from(self.reg(o))
            }
            0x224..=0x240 => u64::from(self.state.response[((0x240 - o) / 4) as usize]),
            _ if INPUT_BUFFER.contains(&o) || plain_word_mask(o).is_some() => {
                u64::from(self.reg(o))
            }
            _ if channel(o).is_some() => {
                let block = channel(o).unwrap();
                if o == block + 4 {
                    u64::from(self.state.channels.contains_key(&block))
                } else {
                    u64::from(self.reg(o))
                }
            }
            _ if unit_latch(o) => u64::from(self.reg(o)),
            _ if self.retain_unmodeled && o < self.retain_limit => u64::from(self.reg(o)),
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn read_stable(&self, address: u64, size: u8) -> bool {
        let o = address.wrapping_sub(self.base);
        // Words backed by stored state only: the image registers are read
        // through the sensor and are not covered.
        size == 4 && o & 3 == 0 && matches!(o, 0 | 0x180..=0x1b0 | 0x200..=0x21c | 0x224..=0x240)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let o = address.wrapping_sub(self.base);
        self.advance(cx);
        if size != 4 || o & 3 != 0 {
            cx.unmodeled();
            return;
        }
        let value = value as u32;
        match o {
            0 => {
                self.state.registers.insert(0, value | 1);
                if value & 1 == 0 {
                    self.cancel();
                }
            }
            0x200 => {
                if value > 1 {
                    cx.fail("invalid sensor command reset");
                    return;
                }
                self.cancel();
                self.state.registers.insert(o, 1);
            }
            0x204 => {
                if value == 0 {
                    self.cancel();
                } else if value == 1 {
                    if let Err(e) = self.submit(cx) {
                        cx.fail(e);
                    }
                } else {
                    cx.fail("invalid sensor command start");
                }
            }
            0x4..=0x34 | 0x40..=0x94 | 0xc0..=0x114 | 0x140 | 0x144 => {
                if !self.sensor.image_supported(o) {
                    cx.unmodeled();
                    return;
                }
                self.sensor.image_write(cx, o, value)
            }
            // The words at 0x180..0x1b0 are stored but never read by a submit,
            // so a write while a command is pending cannot change it.
            0x180..=0x1b0 => {
                self.state.registers.insert(o, value);
            }
            0x208 | 0x20c | 0x210 | 0x214 | 0x218 | 0x21c => {
                if self.state.pending.is_some() {
                    cx.fail("reconfiguring active sensor command");
                } else {
                    self.state.registers.insert(o, value);
                }
            }
            _ if INPUT_BUFFER.contains(&o) || plain_word_mask(o).is_some() => {
                let mask = plain_word_mask(o).unwrap_or_else(|| input_buffer_mask(o));
                if value & !mask != 0 {
                    cx.fail(format!(
                        "input buffer word {o:#x} value {value:#x} sets bits outside its fields"
                    ));
                    return;
                }
                self.state.registers.insert(o, value);
            }
            _ if channel(o).is_some() => {
                let block = channel(o).unwrap();
                if o == block + 4 {
                    if value & 1 != 0 {
                        let Some(at) = cx.now().checked_add(self.latency_ticks) else {
                            cx.fail("sensor channel deadline overflow");
                            return;
                        };
                        self.state.channels.insert(block, at);
                        cx.schedule(at, CHANNEL_TOKEN | block);
                    } else {
                        self.state.channels.remove(&block);
                    }
                } else {
                    self.state.registers.insert(o, value);
                }
            }
            _ if unit_latch(o) => {
                self.state.registers.insert(o, value);
            }
            _ if self.retain_unmodeled && o < self.retain_limit => {
                self.state.registers.insert(o, value);
            }
            _ => cx.unmodeled(),
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token == self.state.generation || token & CHANNEL_TOKEN != 0 {
            self.advance(cx);
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state
            .deadline
            .into_iter()
            .chain(self.state.channels.values().copied())
            .min()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let now = cx.now();
        self.state.channels.retain(|_, at| *at > now);
        if !self.state.deadline.is_some_and(|at| at <= cx.now()) {
            return;
        }
        self.state.deadline = None;
        let Some(packets) = self.state.pending.take() else {
            return;
        };
        let sensor_state = self.sensor.0.clone();
        let mut sensor = sensor_state.lock().unwrap();
        let mut wrote = false;
        for packet in packets {
            let address = u16::from_be_bytes([packet[1], packet[2]]);
            sensor.commands += 1;
            if packet[0] == 0x81 {
                sensor.registers.insert(address, packet[3]);
                sensor.writes += 1;
                wrote = true;
            } else {
                let (gate, mask) = self.sensor.2;
                if !sensor.no_read_gate
                    && sensor.registers.get(&gate).copied().unwrap_or(0) & mask == 0
                {
                    cx.fail(format!(
                        "sensor read while register {gate:#x} gate is closed"
                    ));
                    return;
                }
                for (i, response) in self.state.response.iter_mut().enumerate() {
                    *response = u32::from(
                        sensor
                            .registers
                            .get(&address.wrapping_add(i as u16))
                            .copied()
                            .unwrap_or(0),
                    ) << 16;
                }
                sensor.reads += 1;
            }
        }
        self.state.registers.insert(0x204, 0);
        if wrote {
            let registers = sensor.registers.clone();
            drop(sensor);
            self.update_exposure(cx, &registers);
        }
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        let sensor = self.sensor.0.lock().unwrap();
        Checkpoint::encode(self.name(), &(&self.state, &*sensor))
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let (state, _): (State, SensorState) = saved.decode(self.name())?;
        if state.pending.is_some() != state.deadline.is_some() {
            return Err("sensor pending/deadline disagree".into());
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (command, sensor): (State, SensorState) = saved.decode(self.name())?;
        self.state = command;
        *self.sensor.4.lock().unwrap() = exposure_of(&sensor.registers);
        *self.sensor.0.lock().unwrap() = sensor;
        Ok(())
    }
    fn save(&self) -> Value {
        serde_json::json!({"command": self.state, "sensor": *self.sensor.0.lock().unwrap(), "command_latency_ticks": self.latency_ticks})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value["command"].clone())
            .map_err(|e| format!("sensor command checkpoint: {e}"))?;
        let sensor = serde_json::from_value(value["sensor"].clone())
            .map_err(|e| format!("sensor checkpoint: {e}"))?;
        let latency = value["command_latency_ticks"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or("missing sensor command latency")?;
        if state.pending.is_some() != state.deadline.is_some() {
            return Err("sensor pending/deadline disagree".into());
        }
        self.state = state;
        let sensor: SensorState = sensor;
        *self.sensor.4.lock().unwrap() = exposure_of(&sensor.registers);
        *self.sensor.0.lock().unwrap() = sensor;
        self.latency_ticks = latency;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;

    /// Three exposures in three buffers (the X-T5 4.31 HDR): a reader finds the picture its
    /// input is in; an unknown input or another pitch gets the last picture. A new picture
    /// over an earlier buffer replaces it, and only the newest earlier pictures are kept.
    #[test]
    fn still_raw_at_finds_earlier_pictures() {
        let picture = |address| StillRaw {
            address,
            pitch: 0x3d80,
            width: 7776,
            lines: 5250,
            packed12: false,
            ..Default::default()
        };
        let sensor = Sensor::new(None, (0, 0));
        for address in [0xbb00f6b0, 0x1_fb00f690, 0xb600f6a0] {
            sensor.0.lock().unwrap().record_still_raw(picture(address));
        }
        assert_eq!(
            sensor.still_raw_at(0xbb10d1e0, 0x3d80),
            Some(picture(0xbb00f6b0))
        );
        assert_eq!(
            sensor.still_raw_at(0x1_fb10d1c0, 0x3d80),
            Some(picture(0x1_fb00f690))
        );
        assert_eq!(
            sensor.still_raw_at(0xb610d1d0, 0x3d80),
            Some(picture(0xb600f6a0))
        );
        assert_eq!(
            sensor.still_raw_at(0x1_c34f5880, 0x3d60),
            Some(picture(0xb600f6a0))
        );
        assert_eq!(
            sensor.still_raw_at(0xbb10d1e0, 0x3d60),
            Some(picture(0xb600f6a0))
        );
        // A picture 0x1000 bytes into the first buffer replaces it.
        sensor
            .0
            .lock()
            .unwrap()
            .record_still_raw(picture(0xbb0106b0));
        assert_eq!(
            sensor.0.lock().unwrap().earlier_still_raws,
            vec![picture(0x1_fb00f690), picture(0xb600f6a0)]
        );
        for n in 0..10 {
            sensor
                .0
                .lock()
                .unwrap()
                .record_still_raw(picture(0x2_0000_0000 + n * 0x1000_0000));
        }
        let state = sensor.0.lock().unwrap();
        assert_eq!(state.earlier_still_raws.len(), EARLIER_STILL_RAWS);
        assert_eq!(
            state.earlier_still_raws[EARLIER_STILL_RAWS - 1],
            picture(0x2_8000_0000)
        );
    }
    /// Gain register writes set the shared exposure (gain 4095 / (4095 - code), first code);
    /// a snapshot restore recomputes it from the registers.
    #[test]
    fn gain_writes_set_the_sensor_exposure() {
        let sensor = Sensor::new(None, (0x59, 4));
        let mut command = Command::new(0xff948000, sensor.clone(), 10).unwrap();
        // Code 0x800 on both pairs: 4095 / 2047 = 2.0005.
        let packets: Vec<u8> = [(0x38u16, 0x00u8), (0x39, 0x08), (0x3a, 0x00), (0x3b, 0x08)]
            .iter()
            .flat_map(|&(r, v)| [0x81, (r >> 8) as u8, r as u8, v, 0, 0, 0, 0])
            .collect();
        let mut host = RecordingHost {
            ram: packets.clone(),
            ram_base: 0x1000,
            ..Default::default()
        };
        assert_eq!(sensor.relative_exposure(), 1.);
        for (offset, value) in [
            (0x208, 0x10404),
            (0x20c, 0),
            (0x210, 0x1000),
            (0x218, 8),
            (0x204, 1),
        ] {
            command.write(
                &mut Cx::new(&mut host, 0, 0, 0),
                0xff948000 + offset,
                4,
                value,
            );
        }
        host.now = 1000;
        command.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(
            gain_codes(&sensor.0.lock().unwrap().registers),
            [0x800, 0x800]
        );
        assert!((sensor.relative_exposure() - 4095. / 2047.).abs() < 1e-9);
        assert_eq!(analog_gain(0), Some(1.));
        assert_eq!(analog_gain(4095), None);
        let saved = command.checkpoint().unwrap();
        *sensor.exposure().lock().unwrap() = Exposure::default();
        sensor.0.lock().unwrap().registers.insert(0x39, 0);
        command.restore_checkpoint(&saved).unwrap();
        assert!((sensor.relative_exposure() - 4095. / 2047.).abs() < 1e-9);
    }
    /// A 16-bit still frame holds optical black (1024) in its margins and the scene, at the
    /// active size, from the margin on; the exposure at the frame end scales the samples and
    /// stays with the picture.
    #[test]
    fn the_still_channel_writes_optical_black_and_latches_the_exposure() {
        let sensor = Sensor::new(
            Some(ImageRoutes {
                dma: Some([162, 163]),
                timing: [166, 164],
                line_stride: true,
                frame_start_pulse: None,
            }),
            (0x59, 4),
        )
        .with_still_channel(StillChannel {
            channel: 1,
            sixteen_bit_codes: 1 << 5,
            scene: bars,
            optical_black: (6, 6),
        });
        let (width, lines, pitch) = (24usize, 18usize, 0x40usize);
        let mut host = RecordingHost {
            ram: vec![0x55; pitch * lines],
            ram_base: 0x2000_0000,
            ..Default::default()
        };
        let capture = |host: &mut RecordingHost| {
            for (offset, value) in [
                (0xdc, 5 << 16),
                (0xc4, width as u32),
                (0xc8, lines as u32),
                (0xcc, 0),
                (0xd0, 0x2000_0000),
                (0xd4, 0),
                (0xd8, pitch as u32),
                (0xe0, 2),
                (0xc0, 1),
            ] {
                sensor.image_write(&mut Cx::new(host, 0, 0, 0), offset, value);
            }
            sensor.frame_start(1);
            sensor.frame(&mut Cx::new(host, 0, 0, 0), 1).unwrap();
        };
        capture(&mut host);
        let sample = |host: &RecordingHost, y: usize, x: usize| {
            u16::from_le_bytes([host.ram[y * pitch + 2 * x], host.ram[y * pitch + 2 * x + 1]])
        };
        for y in 0..lines {
            for x in 0..width {
                let v = sample(&host, y, x);
                assert!(v <= xtrans::WHITE, "({x}, {y}) {v}");
                if y < 6 || x < 6 {
                    assert_eq!(v, xtrans::BLACK, "optical black ({x}, {y})");
                }
            }
        }
        // The active area starts with the scene's first pixel (white bar, sRGB 235) at site
        // (6, 6) = (0, 0): red.
        let lut = xtrans::Response::xt5(1.).mosaic_lut();
        assert_eq!(sample(&host, 6, 6), lut.0[0][235]);
        assert_eq!(sample(&host, 7, 6), lut.0[1][235]);
        assert_eq!(sensor.still_raw().unwrap().exposure, 1.);
        // Twice the exposure: twice the signal above black.
        *sensor.exposure().lock().unwrap() = Exposure {
            seconds: 1. / 125.,
            analog_gain: 2.,
        };
        let before = sample(&host, 7, 6) - xtrans::BLACK;
        capture(&mut host);
        let after = sample(&host, 7, 6) - xtrans::BLACK;
        assert!(after.abs_diff(2 * before) <= 1, "{before} -> {after}");
        assert_eq!(sensor.still_raw().unwrap().exposure, 2.);
        assert_eq!(sample(&host, 0, 0), xtrans::BLACK);
    }
    #[test]
    fn timing_input_causes_ack_without_granting_raw_dma_support() {
        let sensor = Sensor::new(
            Some(ImageRoutes {
                dma: None,
                timing: [100, 102],
                line_stride: false,
                frame_start_pulse: None,
            }),
            (0x51, 8),
        );
        let mut command = Command::new(0xfff6c000, sensor.clone(), 17).unwrap();
        let mut host = RecordingHost::default();
        command.write(&mut Cx::new(&mut host, 3, 0xabad, 0), 0xfff6c140, 4, 15);
        sensor.timing(&mut Cx::new(&mut host, 0, 0, 0), true);
        assert!(host.irqs.iter().rev().find(|(i, _)| *i == 100).unwrap().1);
        assert_eq!(
            command.read(&mut Cx::new(&mut host, 0, 0, 0), 0xfff6c144, 4),
            3
        );
        command.write(&mut Cx::new(&mut host, 0, 0, 0), 0xfff6c144, 4, 3);
        assert!(!host.irqs.iter().rev().find(|(i, _)| *i == 100).unwrap().1);
        sensor.timing(&mut Cx::new(&mut host, 3, 0x1234, 0), false);
        assert!(host.irqs.iter().rev().find(|(i, _)| *i == 102).unwrap().1);
        assert_eq!(
            command.read(&mut Cx::new(&mut host, 0, 0, 0), 0xfff6c144, 4),
            12
        );
        let saved = command.save();
        {
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            command.write(&mut cx, 0xfff6c040, 4, 1);
            assert!(cx.is_unmodeled());
        }
        assert_eq!(command.save(), saved);
        assert!(host.ram.is_empty());
    }
    #[test]
    fn frame_start_pulse_is_an_edge_at_every_frame_start_without_enable_or_status() {
        let sensor = Sensor::new(
            Some(ImageRoutes {
                dma: None,
                timing: [166, 164],
                line_stride: false,
                frame_start_pulse: Some(165),
            }),
            (0x59, 4),
        );
        let mut host = RecordingHost::default();
        for _ in 0..2 {
            sensor.timing(&mut Cx::new(&mut host, 0, 0, 0), true);
            sensor.timing(&mut Cx::new(&mut host, 0, 0, 0), false);
        }
        let pulses: Vec<bool> = host
            .irqs
            .iter()
            .filter(|(line, _)| *line == 165)
            .map(|(_, level)| *level)
            .collect();
        // +0x140 is zero: the CCDTG lines stay low, the VD pulse still fires once per start.
        assert_eq!(pulses, [true, false, true, false]);
        assert!(
            host.irqs
                .iter()
                .filter(|(l, _)| *l != 165)
                .all(|(_, level)| !level)
        );
        assert_eq!(sensor.image_reg(0x144), 15);
    }
    #[test]
    fn raw_dma_transfers_exactly_the_frames_that_start_enabled() {
        let sensor = Sensor::new(
            Some(ImageRoutes {
                dma: Some([97, 98]),
                timing: [102, 100],
                line_stride: false,
                frame_start_pulse: None,
            }),
            (0x59, 4),
        );
        let mut host = RecordingHost {
            ram: vec![0x55; 0x10],
            ram_base: 0x1000,
            ..Default::default()
        };
        // A frame is under way when the channel is programmed and enabled.
        sensor.frame_start(0);
        for (offset, value) in [(0x40, 1), (0x44, 2), (0x48, 2), (0x50, 0x1000), (0x60, 1)] {
            sensor.image_write(&mut Cx::new(&mut host, 0, 0, 0), offset, value);
        }
        sensor.frame(&mut Cx::new(&mut host, 0, 0, 0), 0).unwrap();
        assert_eq!(host.ram, vec![0x55; 0x10]);
        assert_eq!(sensor.image_reg(0x64), 0);
        assert!(!host.irqs.iter().any(|(i, on)| *i == 97 && *on));
        // The next frame starts with the channel enabled; disabling it during
        // that frame stops the transfers after it, not the frame itself.
        sensor.frame_start(0);
        sensor.image_write(&mut Cx::new(&mut host, 0, 0, 0), 0x40, 0);
        sensor.frame(&mut Cx::new(&mut host, 0, 0, 0), 0).unwrap();
        assert_eq!(&host.ram[..4], &[0, 0, 31, 0]);
        assert_eq!(sensor.image_reg(0x64), 1);
        assert!(host.irqs.iter().rev().find(|(i, _)| *i == 97).unwrap().1);
        sensor.image_write(&mut Cx::new(&mut host, 0, 0, 0), 0x64, 1);
        host.ram.fill(0x55);
        sensor.frame_start(0);
        sensor.frame(&mut Cx::new(&mut host, 0, 0, 0), 0).unwrap();
        assert_eq!(host.ram, vec![0x55; 0x10]);
        assert_eq!(sensor.image_reg(0x64), 0);
    }
    /// The incremental ramp writes the same bytes as the per-sample formula it replaced.
    #[test]
    fn ramp_lines_match_the_per_sample_formula() {
        fn reference(width: u32, y: u32, frame: u64, packed12: bool) -> Vec<u8> {
            let sample =
                |x: u32| ((u64::from(x) * 31 + u64::from(y) * 17 + frame * 131) & 0x3fff) as u16;
            let mut bytes = Vec::new();
            if packed12 {
                for x in (0..width).step_by(2) {
                    let (a, b) = (sample(x) >> 2, sample(x + 1) >> 2);
                    bytes.extend_from_slice(&[
                        a as u8,
                        (a >> 8) as u8 | (b << 4) as u8,
                        (b >> 4) as u8,
                    ]);
                }
            } else {
                for x in 0..width {
                    bytes.extend_from_slice(&sample(x).to_le_bytes());
                }
            }
            bytes
        }
        for (width, y, frame) in [
            (2, 0, 0),
            (7776, 0, 0),
            (7776, 4371, 1799),
            (6308, 17, 123_456_789),
            (16386, 1, 3),
        ] {
            for packed12 in [false, true] {
                let expected = reference(width, y, frame, packed12);
                let mut line = vec![0xa5; expected.len()];
                ramp_line(y, frame, packed12, &mut line);
                assert_eq!(
                    line, expected,
                    "width {width} y {y} frame {frame} packed {packed12}"
                );
            }
        }
    }
    fn bars() -> &'static fujimod_emulator_imaging::frame::Scene {
        static BARS: fujimod_emulator_imaging::frame::Scene =
            fujimod_emulator_imaging::frame::Scene::Bars;
        &BARS
    }
    #[test]
    fn the_still_channel_writes_the_scene_once_per_programming_with_frame_interrupts() {
        let sensor = Sensor::new(
            Some(ImageRoutes {
                dma: Some([162, 163]),
                timing: [166, 164],
                line_stride: false,
                frame_start_pulse: None,
            }),
            (0x59, 4),
        )
        .with_still_channel(StillChannel {
            channel: 1,
            sixteen_bit_codes: 1 << 5,
            scene: bars,
            optical_black: (0, 0),
        });
        let mut host = RecordingHost {
            ram: vec![0x55; 0x200],
            ram_base: 0x1_0000_0000,
            ..Default::default()
        };
        let write = |host: &mut RecordingHost, offset, value| {
            sensor.image_write(&mut Cx::new(host, 0, 0, 0), offset, value)
        };
        let frame = |host: &mut RecordingHost| {
            sensor.frame_start(1);
            sensor.frame(&mut Cx::new(host, 0, 0, 0), 1)
        };
        // Live view: the channel is programmed and enabled, but its frame interrupts are off.
        // 16 samples x 8 lines at pitch 0x30, format code 5 (+0xc4 / +0xc8 carry start fields).
        for (offset, value) in [
            (0xdc, 5 << 16 | 0x10),
            (0xc4, 5 << 16 | 16),
            (0xc8, 7 << 16 | 8),
            (0xcc, 1),
            (0xd0, 0),
            (0xd8, 0x30),
            (0xc0, 1),
        ] {
            write(&mut host, offset, value);
        }
        for _ in 0..3 {
            frame(&mut host).unwrap();
        }
        assert_eq!(host.ram, vec![0x55; 0x200]);
        assert_eq!(sensor.image_reg(0xe4), 0);
        assert!(!host.irqs.rev_find(163));
        // The capture: 0x13522dc enables the frame interrupts (here bits 0..3).
        write(&mut host, 0xe0, 0xf);
        frame(&mut host).unwrap();
        // Bars: the first two pixels are white (235), so every site of them takes its sample
        // with the X-T5 response at the reference exposure (red and blue on line 0, green on
        // line 1).
        let lut = xtrans::Response::xt5(1.).mosaic_lut();
        let (mut white, mut white1) = ([0u8; 4], [0u8; 4]);
        xtrans::mosaic_line_with(&[235; 6], (0, 0), &mut white, &lut);
        xtrans::mosaic_line_with(&[235; 6], (1, 0), &mut white1, &lut);
        assert_ne!(white[..2], white[2..]);
        assert_eq!(white1[..2], white1[2..]);
        assert_eq!(&host.ram[..4], &white);
        assert_eq!(&host.ram[0x30..0x34], &white1);
        assert_eq!(&host.ram[0x20..0x30], &[0x55; 0x10]);
        assert_eq!(sensor.image_reg(0xe4), 2);
        assert!(host.irqs.rev_find(163));
        assert_eq!(
            sensor.still_raw(),
            Some(StillRaw {
                address: 0x1_0000_0000,
                pitch: 0x30,
                width: 16,
                lines: 8,
                packed12: false,
                ..Default::default()
            })
        );
        // The ISR clears bit 1 by writing 2. Later frames write nothing until +0xd0 is
        // programmed again.
        write(&mut host, 0xe4, 2);
        assert_eq!(sensor.image_reg(0xe4), 0);
        assert!(!host.irqs.rev_find(163));
        host.ram.fill(0x55);
        frame(&mut host).unwrap();
        assert_eq!(host.ram, vec![0x55; 0x200]);
        assert_eq!(sensor.image_reg(0xe4), 0);
        // With only bit 0 enabled, the done status is bit 0.
        write(&mut host, 0xe0, 1);
        write(&mut host, 0xd0, 0);
        frame(&mut host).unwrap();
        assert_eq!(&host.ram[..4], &white);
        assert_eq!(sensor.image_reg(0xe4), 1);
        // A pitch of 1.5 bytes a sample (0x18 for 16 samples) is packed 12-bit lines, whatever
        // the code: the top 12 bits of the 16-bit samples, two in three bytes, nothing past the
        // line.
        host.ram.fill(0x55);
        write(&mut host, 0xdc, 1 << 16);
        write(&mut host, 0xd8, 0x18);
        write(&mut host, 0xd0, 0);
        frame(&mut host).unwrap();
        let (mut packed, mut packed1) = ([0u8; 3], [0u8; 3]);
        fujimod_emulator_imaging::xtrans::pack12_line(&white, &mut packed);
        fujimod_emulator_imaging::xtrans::pack12_line(&white1, &mut packed1);
        assert_eq!(&host.ram[..3], &packed);
        assert_eq!(&host.ram[0x18..0x1b], &packed1);
        assert_eq!(&host.ram[0xc0..], &[0x55; 0x140]);
        assert_eq!(
            sensor.still_raw(),
            Some(StillRaw {
                address: 0x1_0000_0000,
                pitch: 0x18,
                width: 16,
                lines: 8,
                packed12: true,
                ..Default::default()
            })
        );
        // A pitch below 1.5 bytes a sample refuses.
        write(&mut host, 0xdc, 5 << 16);
        write(&mut host, 0xd8, 0x16);
        write(&mut host, 0xd0, 0);
        assert!(frame(&mut host).unwrap_err().contains("does not hold 16"));
        // An undecoded format code with a 16-bit pitch refuses.
        write(&mut host, 0xdc, 1 << 16);
        write(&mut host, 0xd8, 0x30);
        write(&mut host, 0xd0, 0);
        let error = frame(&mut host).unwrap_err();
        assert!(error.contains("format code 1"), "{error}");
    }
    trait RevFind {
        fn rev_find(&self, irq: u32) -> bool;
    }
    impl RevFind for Vec<(u32, bool)> {
        fn rev_find(&self, irq: u32) -> bool {
            self.iter()
                .rev()
                .find(|(i, _)| *i == irq)
                .is_some_and(|(_, on)| *on)
        }
    }
    #[test]
    fn raw_dma_line_stride_places_each_line_and_refuses_short_strides() {
        let sensor = Sensor::new(
            Some(ImageRoutes {
                dma: Some([97, 98]),
                timing: [102, 100],
                line_stride: true,
                frame_start_pulse: None,
            }),
            (0x59, 4),
        );
        let mut host = RecordingHost {
            ram: vec![0x55; 0x40],
            ram_base: 0x1000,
            ..Default::default()
        };
        for (offset, value) in [
            (0x40, 1),
            (0x44, 4),
            (0x48, 2),
            (0x50, 0x1000),
            (0x58, 12),
            (0x60, 1),
        ] {
            sensor.image_write(&mut Cx::new(&mut host, 0, 0, 0), offset, value);
        }
        sensor.frame_start(0);
        sensor.frame(&mut Cx::new(&mut host, 0, 0, 0), 0).unwrap();
        assert_eq!(&host.ram[..8], &[0, 0, 31, 0, 62, 0, 93, 0]);
        assert_eq!(&host.ram[8..12], &[0x55; 4]);
        assert_eq!(&host.ram[12..14], &17u16.to_le_bytes());
        assert_eq!(&host.ram[20..22], &[0x55; 2]);
        assert!(host.irqs.iter().rev().find(|(i, _)| *i == 97).unwrap().1);
        // 1.5 bytes a sample: packed 12-bit, two samples in three bytes, nothing past the pitch.
        host.ram.fill(0x55);
        sensor.image_write(&mut Cx::new(&mut host, 0, 0, 0), 0x58, 6);
        sensor.frame_start(0);
        sensor.frame(&mut Cx::new(&mut host, 0, 0, 0), 0).unwrap();
        // Frame 1: samples 131, 162, 193, 224 -> 12-bit 32, 40, 48, 56.
        assert_eq!(&host.ram[..6], &[32, 0x80, 2, 48, 0x80, 3]);
        assert_eq!(&host.ram[12..14], &[0x55; 2]);
        sensor.image_write(&mut Cx::new(&mut host, 0, 0, 0), 0x58, 4);
        sensor.frame_start(0);
        assert!(sensor.frame(&mut Cx::new(&mut host, 0, 0, 0), 0).is_err());
    }
    #[test]
    fn packet_lists_read_gate_cancel_and_exact_deadline_are_program_independent() {
        fn experiment(cpu: usize, pc: u64, late: bool) -> Value {
            let sensor = Sensor::new(None, (0x51, 8));
            let mut command = Command::new(0xfff6c000, sensor.clone(), 17).unwrap();
            let mut host = RecordingHost {
                ram_base: 0x1000,
                ram: vec![
                    0x81, 0, 0x51, 8, 0, 0, 0, 0, 0x81, 0, 0, 4, 0, 0, 0, 0, 0x81, 1, 0, 0xa5, 0,
                    0, 0, 0,
                ],
                ..Default::default()
            };
            for (offset, value) in [(0x208, 0x10403), (0x210, 0x1000), (0x218, 8), (0x204, 1)] {
                let mut cx = Cx::new(&mut host, cpu, pc, 0);
                command.write(&mut cx, 0xfff6c000 + offset, 4, value);
                assert!(cx.refusal().is_none());
            }
            let saved = command.save();
            assert_eq!(command.next_deadline(0), Some(51));
            host.now = 50;
            command.event(&mut Cx::new(&mut host, cpu, pc, 0), 1);
            assert_eq!(command.save(), saved);
            assert!(!sensor.streaming());
            // The transfer owns the submitted DMA packet bytes.
            host.ram.fill(0xff);
            host.now = if late { 1000 } else { 51 };
            command.event(&mut Cx::new(&mut host, cpu, pc, 0), 1);
            assert!(sensor.streaming());
            assert_eq!(sensor.0.lock().unwrap().commands, 3);
            host.ram[..8].copy_from_slice(&[0x82, 1, 0, 0, 0, 0, 0, 0]);
            for (offset, value) in [(0x208, 0x10401), (0x204, 1)] {
                command.write(
                    &mut Cx::new(&mut host, cpu, pc, 0),
                    0xfff6c000 + offset,
                    4,
                    value,
                );
            }
            host.now += 17;
            command.advance(&mut Cx::new(&mut host, cpu, pc, 0));
            assert_eq!(
                command.read(&mut Cx::new(&mut host, cpu, pc, 0), 0xfff6c240, 4),
                0xa50000
            );
            // Cancel a pending write; its old event cannot apply it.
            host.ram[..8].copy_from_slice(&[0x81, 1, 0, 0x99, 0, 0, 0, 0]);
            command.write(&mut Cx::new(&mut host, cpu, pc, 0), 0xfff6c204, 4, 1);
            command.write(&mut Cx::new(&mut host, cpu, pc, 0), 0xfff6c204, 4, 0);
            let saved = command.save();
            host.now += 17;
            command.event(&mut Cx::new(&mut host, cpu, pc, 0), 3);
            assert_eq!(command.save(), saved);
            assert_eq!(command.next_deadline(host.now), None);
            saved
        }
        let expected = experiment(0, 0, false);
        assert_eq!(experiment(3, 0xabad, true), expected);
    }
    #[test]
    fn no_read_gate_option_lets_a_read_packet_run_with_register_0x59_unwritten() {
        for (option, closed) in [(false, true), (true, false)] {
            let sensor = Sensor::new(None, (0x59, 4));
            sensor.set_no_read_gate(option);
            let mut command = Command::new(0xfff6c000, sensor, 17_000).unwrap();
            let mut host = RecordingHost {
                ram_base: 0x1000,
                ram: vec![0x82, 0, 0x10, 0, 0, 0, 0, 0],
                ..Default::default()
            };
            for (offset, value) in [(0x208, 0x10401), (0x210, 0x1000), (0x218, 8), (0x204, 1)] {
                command.write(
                    &mut Cx::new(&mut host, 0, 0, 0),
                    0xfff6c000 + offset,
                    4,
                    value,
                );
            }
            host.now = 17_000;
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            command.advance(&mut cx);
            assert_eq!(cx.refusal().is_some(), closed, "option {option}");
        }
    }
    #[test]
    fn command_window_words_the_submit_never_reads_accept_writes_while_busy() {
        // X-T3 5.11: one core sets the one-bit field at +0x188 while another
        // polls the busy bit at +0x204, as an emulated SD card boot did.
        let sensor = Sensor::new(None, (0x59, 4));
        let mut command = Command::new(0xfff6c000, sensor, 100_000).unwrap();
        let mut host = RecordingHost {
            ram_base: 0x1000,
            ram: vec![0x81, 0, 0x59, 4, 0, 0, 0, 0],
            ..Default::default()
        };
        let mut cx = Cx::new(&mut host, 2, 0, 0);
        for (o, v) in [(0x208, 0x10401), (0x210, 0x1000), (0x218, 8), (0x204, 1)] {
            command.write(&mut cx, 0xfff6c000 + o, 4, v);
        }
        command.write(&mut cx, 0xfff6c188, 4, 1);
        assert!(cx.refusal().is_none());
        assert_eq!(command.read(&mut cx, 0xfff6c204, 4), 1);
        command.write(&mut cx, 0xfff6c210, 4, 0x2000);
        assert!(cx.refusal().is_some());
    }
    #[test]
    fn dma_command_waits_for_guest_time_and_restores_pending_transport() {
        let sensor = Sensor::new(None, (0x59, 4));
        let mut command = Command::new(0xff948000, sensor.clone(), 100_000).unwrap();
        let mut host = RecordingHost {
            ram_base: 0x1000,
            ram: vec![0x81, 0, 0x59, 4, 0, 0, 0, 0],
            ..Default::default()
        };
        {
            let mut cx = Cx::new(&mut host, 3, 0xabad, 0);
            for (o, v) in [(0x208, 0x10401), (0x210, 0x1000), (0x218, 8), (0x204, 1)] {
                command.write(&mut cx, 0xff948000 + o, 4, v);
            }
            assert!(cx.refusal().is_none());
        }
        let saved = command.save();
        let token = saved["command"]["generation"].as_u64().unwrap();
        host.now = 99_999;
        command.event(&mut Cx::new(&mut host, 0, 0, 0), token);
        assert_eq!(command.save(), saved);
        assert_eq!(sensor.0.lock().unwrap().writes, 0);
        command.restore(&saved).unwrap();
        host.now = 100_000;
        command.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(sensor.0.lock().unwrap().registers[&0x59], 4);
        assert_eq!(command.next_deadline(host.now), None);
        host.ram[0] = 0x82;
        command.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff948204, 4, 1);
        host.now = 200_000;
        command.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(
            command.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff948240, 4),
            4 << 16
        );
        assert_eq!(sensor.0.lock().unwrap().reads, 1);
    }
    #[test]
    fn asynchronous_single_command_start_bit_clears_on_virtual_time() {
        // X-T5 4.31: 0x2eb7678 sets format bit 31 for an asynchronous list,
        // programs the descriptor words through 0x1352510 (+0x21c first),
        // starts with +0x204 = 1 (0x1356750) and polls +0x204 bit 0
        // (0x1356788). The poll only ends when guest time passes the latency.
        let sensor = Sensor::new(None, (0x59, 4));
        let mut command = Command::new(0xff948000, sensor.clone(), 1000).unwrap();
        let mut host = RecordingHost {
            ram_base: 0x1000,
            ram: vec![0x81, 0x12, 0x34, 0x5a, 0, 0, 0, 0],
            ..Default::default()
        };
        let mut cx = Cx::new(&mut host, 1, 0, 0);
        for (o, v) in [
            (0x208, 0x8001_0401),
            (0x21c, 0),
            (0x20c, 0),
            (0x210, 0x1000),
            (0x214, 0),
            (0x218, 8),
            (0x204, 1),
        ] {
            command.write(&mut cx, 0xff948000 + o, 4, v);
        }
        assert!(cx.refusal().is_none());
        for _ in 0..3 {
            assert_eq!(command.read(&mut cx, 0xff948204, 4) & 1, 1);
        }
        assert_eq!(command.next_deadline(0), Some(1000));
        host.now = 999;
        assert_eq!(
            command.read(&mut Cx::new(&mut host, 1, 0, 0), 0xff948204, 4),
            1
        );
        host.now = 1000;
        assert_eq!(
            command.read(&mut Cx::new(&mut host, 1, 0, 0), 0xff948204, 4),
            0
        );
        assert_eq!(sensor.0.lock().unwrap().registers[&0x1234], 0x5a);
        // +0x220 sits below the eight response words; 0x13568a4 reads it, but
        // nothing establishes its meaning, so it stays unmodeled.
        let mut cx = Cx::new(&mut host, 1, 0, 0);
        command.read(&mut cx, 0xff948220, 4);
        assert!(cx.is_unmodeled());
    }
    #[test]
    fn trigger_channel_start_bit_clears_after_the_command_latency() {
        let sensor = Sensor::new(None, (0x51, 8));
        let mut command = Command::new(0xfff6c000, sensor, 17).unwrap();
        let mut host = RecordingHost::default();
        let base = 0xfff6c000 + 0x2c0;
        command.write(&mut Cx::new(&mut host, 1, 0, 0), base + 8, 4, 0x100);
        command.write(&mut Cx::new(&mut host, 1, 0, 0), base + 4, 4, 1);
        assert_eq!(
            command.read(&mut Cx::new(&mut host, 1, 0, 0), base + 4, 4),
            1
        );
        assert_eq!(
            command.read(&mut Cx::new(&mut host, 1, 0, 0), base + 8, 4),
            0x100
        );
        let &(_, at, token) = host.events.last().unwrap();
        assert_eq!(at, 17);
        host.now = 16;
        command.event(&mut Cx::new(&mut host, 0, 0, 0), token);
        assert_eq!(
            command.read(&mut Cx::new(&mut host, 1, 0, 0), base + 4, 4),
            1
        );
        host.now = 17;
        command.event(&mut Cx::new(&mut host, 0, 0, 0), token);
        assert_eq!(
            command.read(&mut Cx::new(&mut host, 1, 0, 0), base + 4, 4),
            0
        );
        // Offsets past the six channel blocks stay unmodeled.
        let mut cx = Cx::new(&mut host, 1, 0, 0);
        command.read(&mut cx, 0xfff6c000 + 0x430, 4);
        assert!(cx.is_unmodeled());
    }
    #[test]
    fn input_buffer_words_store_their_fields_and_refuse_other_bits() {
        // X-T5 4.31: 0x1354dd8 writes 0x440 = 0x100101 (enable, mode 1, value
        // 0x10) then the seven 9-bit pairs; 0x1355760 reads them back.
        let sensor = Sensor::new(None, (0x59, 4));
        let mut command = Command::new(0xff948000, sensor, 1000).unwrap();
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 1, 0, 0);
        for (o, v) in [
            (0x440, 0x0010_0101),
            (0x444, 0x01ff_0001),
            (0x45c, 0x0100_0000),
            (0x460, 0x7fff_0001),
        ] {
            command.write(&mut cx, 0xff948000 + o, 4, v);
            assert!(cx.refusal().is_none());
            assert_eq!(command.read(&mut cx, 0xff948000 + o, 4), v);
        }
        assert!(cx.refusal().is_none());
        assert_eq!(command.next_deadline(0), None);
        for (o, v) in [(0x440, 0x8000_0000), (0x440, 2), (0x444, 0x200)] {
            let mut cx = Cx::new(&mut host, 1, 0, 0);
            command.write(&mut cx, 0xff948000 + o, 4, v);
            assert!(cx.refusal().is_some());
        }
        // The words around the two blocks stay unmodeled.
        for o in [0x43c, 0x488, 0x4c8, 0x4fc] {
            let mut cx = Cx::new(&mut host, 1, 0, 0);
            command.read(&mut cx, 0xff948000 + o, 4);
            assert!(cx.is_unmodeled());
        }
    }
    #[test]
    fn input_buffer_size_and_flag_words_read_modify_write() {
        // X100VI 1.31: 0x143fa44 reads 0x480 or 0x4c0 and writes it back with bit 0 and bits 16 to
        // 26 replaced; 0x143fbec reads bits 0 and 1 of 0x484 and 0x4c4.
        let sensor = Sensor::new(None, (0x59, 4));
        let mut command = Command::new(0xff948000, sensor, 1000).unwrap();
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 1, 0, 0);
        for o in [0x480, 0x484, 0x4c0, 0x4c4] {
            assert_eq!(command.read(&mut cx, 0xff948000 + o, 4), 0);
        }
        for (o, v) in [
            (0x480, 0x0400_0001),
            (0x4c0, 0x0040_0000),
            (0x484, 3),
            (0x4c4, 2),
        ] {
            command.write(&mut cx, 0xff948000 + o, 4, v);
            assert_eq!(command.read(&mut cx, 0xff948000 + o, 4), v);
        }
        assert!(cx.refusal().is_none());
        assert_eq!(command.next_deadline(0), None);
        for (o, v) in [(0x480, 0x0800_0000), (0x4c0, 2), (0x484, 4)] {
            let mut cx = Cx::new(&mut host, 1, 0, 0);
            command.write(&mut cx, 0xff948000 + o, 4, v);
            assert!(cx.refusal().is_some());
        }
    }
    #[test]
    fn enable_word_at_0x120_is_a_plain_boolean_word() {
        // X100VI 1.31: 0x143e754 writes 0 or 1 once; 0x143e7b4 reads it once and keeps bit 0.
        let sensor = Sensor::new(None, (0x59, 4));
        let mut command = Command::new(0xff948000, sensor, 1000).unwrap();
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 1, 0, 0);
        assert_eq!(command.read(&mut cx, 0xff948120, 4), 0);
        for v in [1, 0] {
            command.write(&mut cx, 0xff948120, 4, v);
            assert_eq!(command.read(&mut cx, 0xff948120, 4), v);
        }
        assert!(cx.refusal().is_none());
        assert_eq!(command.next_deadline(0), None);
        let mut cx = Cx::new(&mut host, 1, 0, 0);
        command.write(&mut cx, 0xff948120, 4, 2);
        assert!(cx.refusal().is_some());
        for o in [0xff94811c, 0xff948124] {
            let mut cx = Cx::new(&mut host, 1, 0, 0);
            command.write(&mut cx, o, 4, 0);
            assert!(cx.refusal().is_some());
        }
    }
    #[test]
    fn unit_configuration_words_latch() {
        let sensor = Sensor::new(None, (0x51, 8));
        let mut command = Command::new(0xff948000, sensor, 17).unwrap();
        let mut host = RecordingHost::default();
        for (o, v) in [
            (0x400, 0x11),
            (0x42c, 0x22),
            (0x440, 0x0033_0001),
            (0x47c, 0x0044_0044),
            (0x480, 0x0400_0001),
            (0x4c0, 0x0010_0001),
        ] {
            let mut cx = Cx::new(&mut host, 1, 0, 0);
            command.write(&mut cx, 0xff948000 + o, 4, v);
            assert!(cx.refusal().is_none() && !cx.is_unmodeled());
            assert_eq!(command.read(&mut cx, 0xff948000 + o, 4), v);
        }
        for o in [0x430, 0x4c8] {
            let mut cx = Cx::new(&mut host, 1, 0, 0);
            command.read(&mut cx, 0xff948000 + o, 4);
            assert!(cx.is_unmodeled(), "{o:#x} read");
            let mut cx = Cx::new(&mut host, 1, 0, 0);
            command.write(&mut cx, 0xff948000 + o, 4, 1);
            assert!(cx.is_unmodeled(), "{o:#x} write");
        }
    }
}
