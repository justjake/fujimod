//! X-Processor Pro imaging control page: block stop and the register bus.
//!
//! The X-T20 2.11 section-5 driver (0xc345b4..0xc34864) runs one command
//! channel at +0x200: +0x208 configuration (bits 8..12 select the target),
//! +0x20c/+0x210 address and frame count of a transmit in RAM, +0x204 bit 0
//! start/busy (polled by 0xc34824) and eight receive words at +0x21c whose
//! bits 15:8 hold the reply byte (0xc347d8). A transmit is up to eight 8-byte
//! frames, `0x81 hi lo value` (write) and `0x82 hi lo` (read), ended early by
//! a frame whose first byte is 0: the only writer of +0x210 (0xc35254 from
//! 0x16b76fc) always passes 8, and the section-6 table builder (0x17f67f4)
//! fills the block with eight register frames (0x1d..0x24 in REC) or with
//! five start-up frames followed by zero frames. The imaging-side chip
//! is a register file per channel and select: a read returns the last value
//! written there; the start-up writes a register and reads it back until it
//! matches. What the chip does with the values is not modeled. Commands
//! complete at once, so busy reads clear.
//!
//! Six transfer channels follow at `+0x240 + 0x40*n` (0xc34910..0xc34e94):
//! +0x00 reset (written 1, then 0), +0x08 mode, +0x0c..+0x14 28-bit address
//! and length words, +0x18 an optional word, and +0x04 bit 0 a run bit that
//! section 6 sets to start a channel and clears to stop it (0xc34864 and
//! 0xc34e24 refuse a channel already in the requested state). A reset also
//! stops the channel: section 6 0x16b7fbc configures the channels in its mask
//! (0x156a654), resets each (0x156a5f8 via 0xc348d8) and then waits at
//! 0x16b8118 for channel 0's run bit to read clear without clearing it
//! itself. Leaving SETUP for REC takes that path with channel 0 running. The
//! other words read back their last write; no data moves.
//!
//! A sensor readout unit may be declared at one of the channel offsets
//! +0x80/+0xc0/+0x180 (X-T20 2.11: unit 2 at +0xc0, section 5 driver
//! 0xc34fcc/0xc35460, interrupt 98, handler 0xc32520). For a still the
//! firmware programs +0x04..+0x14 (line count +0x08, destination +0x0c,
//! line stride +0x10), the interrupt enable +0x1c bit 0 and starts it with
//! bit 0 of +0x00. The unit finishes after the declared ticks: status +0x20
//! bit 0 sets (write 1 to clear) and, when enabled, the level interrupt is
//! high until the status is cleared. Without a declared sensor frame no
//! sensor data is written. With [`SENSOR_PROFILE`] the unit writes, on
//! completion, the programmed line count of `stride / 2` little-endian u16
//! samples per line at the programmed destination and stride (X-T20 2.11 shot:
//! 4072 lines, stride 0x3020 = 6160 samples, 0x147da100): the virtual scene's
//! first frame over the whole raster, linear sRGB through a declared X-Trans
//! response. sRGB black reads [`BLACK`], white [`WHITE`]; no optical or
//! spectral calibration is claimed. The tile phase at the buffer origin is
//! A caller may supply a host RGB image ([`Control::with_sensor_image`]):
//! the read-out then writes that image, scaled to the whole raster
//! bilinearly, through the same response and tile phase in place of
//! the virtual scene. The image is caller-owned (a local picture), not a
//! catalog input.
//!
//! [`TILE_PHASE`] (line, sample): the 2.11 RAF writer stores 4032 lines of
//! 6160 samples from buffer + 0x723c0, 38 lines down and 128 samples back
//! (checked on an emulated RAF: the scene's edges move by exactly that), so
//! this phase gives the file the standard X-Trans pattern its header names.
//!
//! +0x000 bit 0 is a run bit that reads set: 0xc33ec8 clears it and waits
//! for it to read set again (stopped). Other words read back their last
//! write. With a frame sync ([`crate::sync`]) on the board, the frame
//! interrupt words +0x140/+0x144 belong to it instead.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Register window size in bytes.
pub const WINDOW: u64 = 0x1000;
const CONFIGURATION: u64 = 0x208;
const FRAME: u64 = 0x20c;
const LENGTH: u64 = 0x210;
const START: u64 = 0x204;
const RECEIVE: std::ops::Range<u64> = 0x21c..0x23c;
const BUSY: u32 = 1;
const CHANNELS: std::ops::Range<u64> = 0x240..0x240 + 6 * CHANNEL;
const CHANNEL: u64 = 0x40;
const RUN: u64 = 0x04;
/// Bytes of one register-bus frame.
const FRAME_BYTES: usize = 8;
/// Frames of one transmit (one per receive word).
const MAX_FRAMES: u32 = 8;
const WRITE: u8 = 0x81;
/// Entries an opt-in register-bus log keeps (the newest; older ones drop).
const MAX_LOGGED_FRAMES: usize = 1024;
const READOUT_ENABLE: u64 = 0x1c;
const READOUT_STATUS: u64 = 0x20;
const READOUT_DONE: u32 = 1;
const READOUT_LINES: u64 = 0x08;
const READOUT_DESTINATION: u64 = 0x0c;
const READOUT_STRIDE: u64 = 0x10;

/// Declared sensor response of a readout unit (see the module notes).
pub const SENSOR_PROFILE: &str = "xpp-declared-virtual-scene-linear-srgb-xtrans-u16-v1";
/// Declared sample of sRGB black.
pub const BLACK: u16 = 1024;
/// Declared sample of sRGB white.
pub const WHITE: u16 = 16383;
/// X-Trans tile (line, sample) at the readout buffer origin.
pub const TILE_PHASE: (u64, u64) = (4, 2);
use crate::development::XTRANS;

/// A sensor readout unit: its channel offset, level interrupt and the ticks
/// a readout takes. With a frame period the readout begins at the first frame
/// after its start (frames at multiples of the period, as the frame sync
/// counts them) and the ticks count from that frame.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Readout {
    pub offset: u64,
    pub irq: u32,
    pub ticks: u64,
    #[serde(default)]
    pub frame_period: Option<u64>,
}
const READ: u8 = 0x82;

/// One register-bus frame the guest sent, recorded only by an opt-in log
/// ([`Control::with_write_log`]). `sequence` counts every recorded frame; the
/// log keeps the newest [`MAX_LOGGED_FRAMES`]. `pc` is the guest instruction
/// that started the command channel and `tick` the board time; the device
/// sees no registers, so the caller adds LR with an instruction point at `pc`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameRecord {
    pub sequence: u64,
    pub cpu: u8,
    pub pc: u32,
    pub tick: u64,
    pub select: u8,
    pub write: bool,
    pub register: u16,
    pub value: u8,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    /// (select, register) -> value.
    chip: Vec<((u8, u16), u8)>,
    received: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    readout_due: Option<u64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    readout_status: u32,
}
fn is_zero(n: &u32) -> bool {
    *n == 0
}
pub struct Control {
    base: u64,
    words: BTreeMap<u64, u32>,
    chip: BTreeMap<(u8, u16), u8>,
    received: u8,
    readout: Option<Readout>,
    readout_due: Option<u64>,
    readout_status: u32,
    /// The readout writes the declared sensor frame.
    sensor_frame: bool,
    /// Host RGB image (width, height, rows of red, green and blue bytes)
    /// written instead of the virtual scene.
    sensor_image: Option<(u64, u64, std::sync::Arc<[u8]>)>,
    /// Opt-in register-bus frame log (see [`FrameRecord`]).
    write_log: Option<Vec<FrameRecord>>,
    write_log_sequence: u64,
    integration: Option<Integration>,
}
/// A chip register pair (low and high byte of one select) published as one
/// integration value for another device.
pub struct Integration {
    pub select: u8,
    pub low: u16,
    pub high: u16,
    pub value: std::sync::Arc<std::sync::atomic::AtomicU32>,
}
impl Control {
    pub fn new(base: u64) -> Result<Self, String> {
        if base & 0xfff != 0 {
            return Err("Invalid X-Processor Pro imaging control base".into());
        }
        Ok(Self {
            base,
            words: BTreeMap::new(),
            chip: BTreeMap::new(),
            received: 0,
            readout: None,
            readout_due: None,
            readout_status: 0,
            sensor_frame: false,
            sensor_image: None,
            write_log: None,
            write_log_sequence: 0,
            integration: None,
        })
    }
    /// A sensor readout unit at a channel offset.
    pub fn with_readout(mut self, readout: Readout) -> Result<Self, String> {
        if ![0x80, 0xc0, 0x180].contains(&readout.offset)
            || !(32..512).contains(&readout.irq)
            || readout.ticks == 0
            || readout.frame_period == Some(0)
        {
            return Err("Invalid X-Processor Pro sensor readout unit".into());
        }
        self.readout = Some(readout);
        Ok(self)
    }
    /// The readout writes the declared sensor frame named by `profile`.
    pub fn with_sensor_frame(mut self, profile: &str) -> Result<Self, String> {
        if self.readout.is_none() || profile != SENSOR_PROFILE {
            return Err(format!("unknown X-Processor Pro sensor frame {profile}"));
        }
        self.sensor_frame = true;
        Ok(self)
    }
    /// The declared sensor frame takes its picture from a host RGB image of
    /// `width` x `height` pixels instead of the virtual scene.
    pub fn with_sensor_image(
        mut self,
        width: u64,
        height: u64,
        rgb: Vec<u8>,
    ) -> Result<Self, String> {
        if !self.sensor_frame
            || !(1..=16384).contains(&width)
            || !(1..=16384).contains(&height)
            || width * height * 3 != rgb.len() as u64
        {
            return Err("Invalid X-Processor Pro sensor host image".into());
        }
        self.sensor_image = Some((width, height, rgb.into()));
        Ok(self)
    }
    /// Record every register-bus frame (diagnostic only; off by default and
    /// kept out of the checkpoint state).
    pub fn with_write_log(mut self) -> Self {
        self.write_log = Some(Vec::new());
        self
    }
    /// Publish the chip register pair `integration` names into its value.
    pub fn with_integration(mut self, integration: Integration) -> Result<Self, String> {
        if integration.select > 0x1f || integration.low == integration.high {
            return Err("Invalid X-Processor Pro imaging integration pair".into());
        }
        self.integration = Some(integration);
        self.publish_integration();
        Ok(self)
    }
    fn log(&mut self, cx: &Cx<'_>, select: u8, write: bool, register: u16, value: u8) {
        let Some(log) = self.write_log.as_mut() else {
            return;
        };
        self.write_log_sequence += 1;
        if log.len() == MAX_LOGGED_FRAMES {
            log.remove(0);
        }
        log.push(FrameRecord {
            sequence: self.write_log_sequence,
            cpu: u8::try_from(cx.cpu).unwrap_or(u8::MAX),
            pc: u32::try_from(cx.pc).unwrap_or(u32::MAX),
            tick: cx.now(),
            select,
            write,
            register,
            value,
        });
    }
    /// Lines of the declared sensor frame into the programmed buffer.
    fn write_frame(&self, cx: &mut Cx<'_>, r: Readout) {
        let lines = u64::from(self.word(r.offset + READOUT_LINES));
        let stride = u64::from(self.word(r.offset + READOUT_STRIDE));
        let destination = u64::from(self.word(r.offset + READOUT_DESTINATION));
        if !(6..=8192).contains(&lines) || !(12..=0x8000).contains(&stride) || stride & 1 != 0 {
            return cx.fail("Unsupported sensor readout geometry");
        }
        let width = stride / 2;
        let scene = fujimod_virtual_scene::Scene {
            width,
            height: lines,
            frame: 0,
        };
        let response: Vec<u16> = (0..=255u8)
            .map(|v| {
                let srgb = f64::from(v) / 255.;
                let linear = if srgb <= 0.04045 {
                    srgb / 12.92
                } else {
                    ((srgb + 0.055) / 1.055).powf(2.4)
                };
                BLACK + (linear * f64::from(WHITE - BLACK)).round() as u16
            })
            .collect();
        let mut line = vec![0; stride as usize];
        for y in 0..lines {
            let sites = &XTRANS[((y + TILE_PHASE.0) % 6) as usize];
            for (x, sample) in line.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let color = sites[(x + TILE_PHASE.1 as usize) % 6] as usize;
                let rgb = match &self.sensor_image {
                    Some((w, h, image)) => bilinear(image, *w, *h, x as u64, y, width, lines),
                    None => fujimod_virtual_scene::rgb(scene, x as u64, y),
                };
                sample.copy_from_slice(&response[usize::from(rgb[color])].to_le_bytes());
            }
            if cx.ram_write(destination + y * stride, &line).is_err() {
                return cx.fail("Sensor readout buffer outside RAM");
            }
        }
    }
    fn word(&self, offset: u64) -> u32 {
        self.words.get(&offset).copied().unwrap_or(0)
    }
    fn start(&mut self, cx: &mut Cx<'_>) {
        let select = (self.word(CONFIGURATION) >> 8 & 0x1f) as u8;
        let frames = self.word(LENGTH).clamp(1, MAX_FRAMES) as usize;
        let block = match cx.ram_bytes(self.word(FRAME).into(), frames * FRAME_BYTES) {
            Ok(block) => block,
            Err(e) => return cx.fail(format!("Imaging bus frame read failed: {e}")),
        };
        let key = |frame: &[u8]| (select, u16::from_be_bytes([frame[1], frame[2]]));
        for (n, frame) in block.chunks(FRAME_BYTES).enumerate() {
            match frame[0] {
                0 if n > 0 => break,
                WRITE => {
                    self.chip.insert(key(frame), frame[3]);
                    self.log(cx, select, true, key(frame).1, frame[3]);
                }
                READ => {
                    self.received = self.chip.get(&key(frame)).copied().unwrap_or(0);
                    self.log(cx, select, false, key(frame).1, self.received);
                }
                _ => return cx.unsupported("Unrecognized imaging bus frame"),
            }
        }
        self.publish_integration();
    }
    /// The declared integration pair as one value (0 until both bytes were
    /// written), shared with the statistics engine.
    fn publish_integration(&self) {
        if let Some(i) = &self.integration {
            let byte = |register| self.chip.get(&(i.select, register)).copied();
            let value = match (byte(i.low), byte(i.high)) {
                (Some(low), Some(high)) => u32::from(high) << 8 | u32::from(low),
                _ => 0,
            };
            i.value.store(value, std::sync::atomic::Ordering::Relaxed);
        }
    }
}
/// The host image at raster site (`x`, `y`) of a `width` x `height` raster,
/// interpolated bilinearly between pixel centers (8-bit fixed point).
fn bilinear(image: &[u8], w: u64, h: u64, x: u64, y: u64, width: u64, height: u64) -> [u8; 3] {
    // Pixel-center position in 1/256 units, clamped to the image.
    let at = |site: u64, sites: u64, size: u64| {
        let p = ((2 * site + 1) * size * 256 / (2 * sites)).saturating_sub(128);
        let p = p.min((size - 1) * 256);
        ((p / 256) as usize, (p % 256) as u32)
    };
    let ((x0, fx), (y0, fy)) = (at(x, width, w), at(y, height, h));
    let (x1, y1) = ((x0 + 1).min(w as usize - 1), (y0 + 1).min(h as usize - 1));
    let pixel = |px: usize, py: usize, c: usize| u32::from(image[(py * w as usize + px) * 3 + c]);
    [0, 1, 2].map(|c| {
        let top = pixel(x0, y0, c) * (256 - fx) + pixel(x1, y0, c) * fx;
        let bottom = pixel(x0, y1, c) * (256 - fx) + pixel(x1, y1, c) * fx;
        ((top * (256 - fy) + bottom * fy + 32768) >> 16) as u8
    })
}

impl Device for Control {
    fn name(&self) -> &'static str {
        "xpp-imaging-control"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(self.base);
        if size != 4 || offset & 3 != 0 || offset >= WINDOW {
            cx.unmodeled();
            return 0;
        }
        u64::from(match offset {
            0 => self.word(0) | 1,
            o if self.readout.is_some_and(|r| o == r.offset + READOUT_STATUS) => {
                self.readout_status
            }
            o if RECEIVE.contains(&o) => u32::from(self.received) * 0x101,
            o => self.word(o),
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(self.base);
        let Ok(mut value) = u32::try_from(value) else {
            return cx.unmodeled();
        };
        if size != 4 || offset & 3 != 0 || offset >= WINDOW {
            return cx.unmodeled();
        }
        if let Some(r) = self.readout {
            if offset == r.offset + READOUT_STATUS {
                self.readout_status &= !value;
                if self.readout_status == 0 {
                    cx.set_irq(r.irq, false);
                }
                return;
            }
            if offset == r.offset && value & 1 != 0 && self.readout_due.is_none() {
                let begin = match r.frame_period {
                    Some(period) => (cx.now() / period + 1).checked_mul(period),
                    None => Some(cx.now()),
                };
                self.readout_due = begin.and_then(|at| at.checked_add(r.ticks));
                if self.readout_due.is_none() {
                    return cx.fail("Sensor readout deadline overflow");
                }
            }
        }
        match offset {
            o if RECEIVE.contains(&o) => return cx.unmodeled(),
            START if value & BUSY != 0 => {
                value &= !BUSY;
                self.start(cx);
            }
            o if CHANNELS.contains(&o)
                && (o - CHANNELS.start).is_multiple_of(CHANNEL)
                && value & 1 != 0 =>
            {
                let run = o + RUN;
                match self.word(run) & !1 {
                    0 => self.words.remove(&run),
                    stopped => self.words.insert(run, stopped),
                };
            }
            _ => {}
        }
        if value == 0 {
            self.words.remove(&offset);
        } else {
            self.words.insert(offset, value);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let Some(r) = self.readout else { return };
        if self.readout_due.is_some_and(|at| at <= cx.now()) {
            self.readout_due = None;
            if self.sensor_frame {
                self.write_frame(cx, r);
            }
            self.readout_status |= READOUT_DONE;
            if self.word(r.offset + READOUT_ENABLE) & 1 != 0 {
                cx.set_irq(r.irq, true);
            }
        }
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.readout_due
    }
    fn save(&self) -> Value {
        serde_json::to_value(self.state()).unwrap_or(Value::Null)
    }
    fn diagnostic_state(&self) -> Value {
        let mut state = self.save();
        if let Some(log) = &self.write_log {
            state["write_log"] = serde_json::to_value(log).unwrap_or(Value::Null);
            state["write_log_sequence"] = Value::from(self.write_log_sequence);
        }
        state
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.check(&state)?;
        self.apply(state);
        Ok(())
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(self.name(), &self.state())
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        if !saved.children().is_empty() || !saved.blobs().is_empty() {
            return Err("Imaging control checkpoint has unexpected parts".into());
        }
        self.check(&saved.decode(self.name())?)
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let state = saved.decode(self.name())?;
        self.apply(state);
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.checkpoint()?.state())
    }
}
impl Control {
    fn state(&self) -> State {
        State {
            words: self.words.clone(),
            chip: self.chip.iter().map(|(k, v)| (*k, *v)).collect(),
            received: self.received,
            readout_due: self.readout_due,
            readout_status: self.readout_status,
        }
    }
    fn check(&self, state: &State) -> Result<(), String> {
        if state
            .words
            .iter()
            .any(|(o, v)| o & 3 != 0 || *o >= WINDOW || *v == 0)
            || state.chip.iter().any(|((select, _), _)| *select > 0x1f)
            || self.readout.is_none() && (state.readout_due.is_some() || state.readout_status != 0)
            || state.readout_status & !READOUT_DONE != 0
        {
            return Err("Invalid imaging control checkpoint".into());
        }
        Ok(())
    }
    fn apply(&mut self, state: State) {
        self.words = state.words;
        self.chip = state.chip.into_iter().collect();
        self.received = state.received;
        self.readout_due = state.readout_due;
        self.readout_status = state.readout_status;
        self.publish_integration();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::Host;
    use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};

    const BASE: u64 = 0xfff39000;

    #[test]
    fn register_bus_reads_back_chip_writes_and_completes_at_once() {
        let mut control = Control::new(BASE).unwrap();
        let mut h = TestHost::new();
        let channel = BASE + 0x200;
        h.ram_write(0x1000, &[WRITE, 0x01, 0x23, 0x5a]).unwrap();
        h.ram_write(0x1010, &[READ, 0x01, 0x23]).unwrap();
        write(&mut control, &mut h, channel + 8, 0x1_0300);
        write(&mut control, &mut h, channel + 0xc, 0x1000);
        write(&mut control, &mut h, channel + 0x10, 1);
        write(&mut control, &mut h, channel + 4, 1);
        assert_eq!(read(&mut control, &mut h, channel + 4), 0);
        write(&mut control, &mut h, channel + 0xc, 0x1010);
        write(&mut control, &mut h, channel + 0x10, 1);
        write(&mut control, &mut h, channel + 4, 1);
        assert_eq!(read(&mut control, &mut h, channel + 0x1c), 0x5a5a);
        // Another select has its own registers.
        write(&mut control, &mut h, channel + 8, 0x2_0400);
        write(&mut control, &mut h, channel + 4, 1);
        assert_eq!(read(&mut control, &mut h, channel + 0x38), 0);
        let saved = control.save();
        let mut other = Control::new(BASE).unwrap();
        other.restore(&saved).unwrap();
        write(&mut other, &mut h, channel + 8, 0x1_0300);
        write(&mut other, &mut h, channel + 4, 1);
        assert_eq!(read(&mut other, &mut h, channel + 0x1c), 0x5a5a);
    }

    #[test]
    fn an_opt_in_log_records_register_bus_frames_outside_the_checkpoint() {
        let mut control = Control::new(BASE).unwrap().with_write_log();
        let mut h = TestHost::new();
        let channel = BASE + 0x200;
        h.ram_write(0x1000, &[WRITE, 0x01, 0x23, 0x5a]).unwrap();
        h.ram_write(0x1010, &[READ, 0x01, 0x23]).unwrap();
        assert!(Control::new(BASE).unwrap().save()["write_log"].is_null());
        write(&mut control, &mut h, channel + 8, 0x1_0300);
        write(&mut control, &mut h, channel + 0xc, 0x1000);
        write(&mut control, &mut h, channel + 0x10, 1);
        write(&mut control, &mut h, channel + 4, 1);
        write(&mut control, &mut h, channel + 0xc, 0x1010);
        write(&mut control, &mut h, channel + 0x10, 1);
        write(&mut control, &mut h, channel + 4, 1);
        let saved = control.save();
        assert!(saved["write_log"].is_null());
        let log = control.diagnostic_state()["write_log"].clone();
        let log: Vec<FrameRecord> = serde_json::from_value(log).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(
            (log[0].sequence, log[0].select, log[0].register),
            (1, 3, 0x0123)
        );
        assert!(log[0].write);
        assert_eq!(log[0].value, 0x5a);
        assert_eq!(
            (log[1].sequence, log[1].register, log[1].value),
            (2, 0x0123, 0x5a)
        );
        assert!(!log[1].write);
        assert_eq!(control.diagnostic_state()["write_log_sequence"], 2);
    }

    #[test]
    fn a_transmit_applies_its_frames_until_a_zero_frame_and_publishes_the_pair() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let value = std::sync::Arc::new(AtomicU32::new(7));
        let mut control = Control::new(BASE)
            .unwrap()
            .with_integration(Integration {
                select: 4,
                low: 0x1d,
                high: 0x1e,
                value: value.clone(),
            })
            .unwrap();
        assert_eq!(value.load(Ordering::Relaxed), 0);
        let mut h = TestHost::new();
        let channel = BASE + 0x200;
        let frame = |register: u8, byte: u8| [WRITE, 0, register, byte, 0, 0, 0, 0];
        // A start-up block: two frames, then zero frames (one the bus must skip).
        let mut block = [frame(0x01, 0x21), frame(0x03, 0x00)].concat();
        block.extend([0; 8]);
        block.extend(frame(0x1d, 0x99));
        h.ram_write(0x1000, &block).unwrap();
        write(&mut control, &mut h, channel + 8, 0x4_0400);
        write(&mut control, &mut h, channel + 0xc, 0x1000);
        write(&mut control, &mut h, channel + 0x10, 8);
        write(&mut control, &mut h, channel + 4, 1);
        assert_eq!(control.chip.get(&(4, 0x01)), Some(&0x21));
        assert_eq!(control.chip.get(&(4, 0x03)), Some(&0x00));
        assert_eq!(control.chip.get(&(4, 0x1d)), None);
        assert_eq!(value.load(Ordering::Relaxed), 0);
        // An exposure block: eight frames, the pair 0x1d/0x1e among them.
        let block = [
            frame(0x1d, 0x1e),
            frame(0x1e, 0x06),
            frame(0x1f, 0x1e),
            frame(0x20, 0x06),
            frame(0x21, 0x1e),
            frame(0x22, 0x06),
            frame(0x23, 0x1e),
            frame(0x24, 0x06),
        ]
        .concat();
        h.ram_write(0x2000, &block).unwrap();
        write(&mut control, &mut h, channel + 0xc, 0x2000);
        write(&mut control, &mut h, channel + 4, 1);
        assert_eq!(control.chip.get(&(4, 0x24)), Some(&0x06));
        assert_eq!(value.load(Ordering::Relaxed), 0x061e);
        // A restored copy publishes the pair it restores.
        let saved = control.save();
        value.store(0, Ordering::Relaxed);
        let mut other = Control::new(BASE)
            .unwrap()
            .with_integration(Integration {
                select: 4,
                low: 0x1d,
                high: 0x1e,
                value: value.clone(),
            })
            .unwrap();
        other.restore(&saved).unwrap();
        assert_eq!(value.load(Ordering::Relaxed), 0x061e);
    }

    #[test]
    fn run_bit_reads_set_and_unknown_frames_are_unsupported() {
        let mut control = Control::new(BASE).unwrap();
        let mut h = TestHost::new();
        write(&mut control, &mut h, BASE, 0x10);
        assert_eq!(read(&mut control, &mut h, BASE), 0x11);
        assert_eq!(read(&mut control, &mut h, BASE + 0x50), 0);
        h.ram_write(0x2000, &[0x7f]).unwrap();
        write(&mut control, &mut h, BASE + 0x20c, 0x2000);
        let mut cx = h.cx();
        control.write(&mut cx, BASE + 0x204, 4, 1);
        assert!(cx.is_unmodeled());
    }

    #[test]
    fn transfer_channels_keep_their_run_bit_and_leave_the_bus_alone() {
        let mut control = Control::new(BASE).unwrap();
        let mut h = TestHost::new();
        let channel = BASE + 0x240 + 0x40 * 5;
        // 0xc3493c configures, 0xc34864 starts, 0xc34e24 stops.
        write(&mut control, &mut h, channel + 8, 0x0100_0201);
        write(&mut control, &mut h, channel + 0xc, 0xa7d_6000);
        write(&mut control, &mut h, channel + 4, 1);
        assert_eq!(read(&mut control, &mut h, channel + 4), 1);
        assert_eq!(read(&mut control, &mut h, channel + 0xc), 0xa7d_6000);
        write(&mut control, &mut h, channel + 4, 0);
        assert_eq!(read(&mut control, &mut h, channel + 4), 0);
        assert_eq!(read(&mut control, &mut h, BASE + 0x21c), 0);
    }

    #[test]
    fn a_channel_reset_stops_a_running_channel() {
        let mut control = Control::new(BASE).unwrap();
        let mut h = TestHost::new();
        let channel = BASE + 0x240;
        write(&mut control, &mut h, channel + 8, 0x100);
        write(&mut control, &mut h, channel + 4, 1);
        // 0xc348d8 pulses the reset; 0x16b8118 then waits for the run bit to clear.
        write(&mut control, &mut h, channel, 1);
        assert_eq!(read(&mut control, &mut h, channel + 4), 0);
        write(&mut control, &mut h, channel, 0);
        assert_eq!(read(&mut control, &mut h, channel + 8), 0x100);
        assert_eq!(read(&mut control, &mut h, channel), 0);
        // Other channels keep running.
        write(&mut control, &mut h, channel + 0x44, 1);
        write(&mut control, &mut h, channel, 1);
        assert_eq!(read(&mut control, &mut h, channel + 0x44), 1);
    }

    #[test]
    fn a_started_readout_finishes_once_and_its_status_clears_on_write() {
        let readout = Readout {
            offset: 0xc0,
            irq: 98,
            ticks: 10,
            frame_period: None,
        };
        assert!(
            Control::new(BASE)
                .unwrap()
                .with_readout(Readout {
                    offset: 0x100,
                    ..readout
                })
                .is_err()
        );
        let mut control = Control::new(BASE).unwrap().with_readout(readout).unwrap();
        let mut h = TestHost::new();
        let unit = BASE + 0xc0;
        write(&mut control, &mut h, unit + 0x1c, 1);
        write(&mut control, &mut h, unit, 1);
        assert_eq!(control.next_deadline(0), Some(10));
        advance(&mut control, &mut h, 9);
        assert_eq!(read(&mut control, &mut h, unit + 0x20), 0);
        advance(&mut control, &mut h, 10);
        assert_eq!(read(&mut control, &mut h, unit + 0x20), 1);
        assert_eq!(control.next_deadline(10), None);
        let saved = control.save();
        write(&mut control, &mut h, unit + 0x20, 1);
        assert_eq!(read(&mut control, &mut h, unit + 0x20), 0);
        let mut other = Control::new(BASE).unwrap().with_readout(readout).unwrap();
        other.restore(&saved).unwrap();
        assert_eq!(read(&mut other, &mut h, unit + 0x20), 1);
        assert!(Control::new(BASE).unwrap().restore(&saved).is_err());
    }

    #[test]
    fn a_readout_on_frames_begins_at_the_next_frame() {
        let readout = Readout {
            offset: 0xc0,
            irq: 98,
            ticks: 10,
            frame_period: Some(100),
        };
        assert!(
            Control::new(BASE)
                .unwrap()
                .with_readout(Readout {
                    frame_period: Some(0),
                    ..readout
                })
                .is_err()
        );
        let mut control = Control::new(BASE).unwrap().with_readout(readout).unwrap();
        let mut h = TestHost::new();
        let unit = BASE + 0xc0;
        write(&mut control, &mut h, unit + 0x1c, 1);
        advance(&mut control, &mut h, 230);
        write(&mut control, &mut h, unit, 1);
        // Started at 230: the read-out begins with the frame at 300.
        assert_eq!(control.next_deadline(230), Some(310));
        advance(&mut control, &mut h, 309);
        assert_eq!(read(&mut control, &mut h, unit + 0x20), 0);
        advance(&mut control, &mut h, 310);
        assert_eq!(read(&mut control, &mut h, unit + 0x20), 1);
        // A start on a frame tick waits for the following frame.
        write(&mut control, &mut h, unit + 0x20, 1);
        advance(&mut control, &mut h, 400);
        write(&mut control, &mut h, unit, 1);
        assert_eq!(control.next_deadline(400), Some(510));
    }

    #[test]
    fn a_readout_with_a_sensor_frame_writes_x_trans_samples_of_the_scene() {
        let readout = Readout {
            offset: 0xc0,
            irq: 98,
            ticks: 10,
            frame_period: None,
        };
        assert!(
            Control::new(BASE)
                .unwrap()
                .with_sensor_frame(SENSOR_PROFILE)
                .is_err()
        );
        let control = || Control::new(BASE).unwrap().with_readout(readout).unwrap();
        assert!(control().with_sensor_frame("other").is_err());
        let mut control = control().with_sensor_frame(SENSOR_PROFILE).unwrap();
        let mut h = TestHost::new();
        let unit = BASE + 0xc0;
        // Six lines of eight samples at 0x1000.
        write(&mut control, &mut h, unit + 0x08, 6);
        write(&mut control, &mut h, unit + 0x0c, 0x1000);
        write(&mut control, &mut h, unit + 0x10, 16);
        write(&mut control, &mut h, unit, 1);
        advance(&mut control, &mut h, 9);
        assert_eq!(h.ram[0x1000..0x1002], [0, 0]);
        advance(&mut control, &mut h, 10);
        let sample = |y: usize, x: usize| {
            u16::from_le_bytes([
                h.ram[0x1000 + 16 * y + 2 * x],
                h.ram[0x1001 + 16 * y + 2 * x],
            ])
        };
        // One bar per sample on the first line, X-Trans row 4 from site 2
        // (B G G R G G B G):
        // white, yellow, cyan, green, magenta, red, blue, black.
        let row: Vec<u16> = (0..8).map(|x| sample(0, x)).collect();
        assert_eq!(
            row,
            [WHITE, WHITE, WHITE, BLACK, BLACK, BLACK, WHITE, BLACK]
        );
        assert_eq!(h.ram[0x1000 + 16 * 6], 0);
    }

    #[test]
    fn a_sensor_host_image_replaces_the_scene_at_each_site() {
        let readout = Readout {
            offset: 0xc0,
            irq: 98,
            ticks: 10,
            frame_period: None,
        };
        let control = || Control::new(BASE).unwrap().with_readout(readout).unwrap();
        // Four red pixels, then four blue: one pixel center per sample.
        let image = [[255, 0, 0]; 4]
            .into_iter()
            .chain([[0, 0, 255]; 4])
            .flatten()
            .collect::<Vec<u8>>();
        assert!(control().with_sensor_image(8, 1, image.clone()).is_err());
        let framed = || control().with_sensor_frame(SENSOR_PROFILE).unwrap();
        assert!(framed().with_sensor_image(8, 2, image.clone()).is_err());
        let mut control = framed().with_sensor_image(8, 1, image).unwrap();
        let mut h = TestHost::new();
        let unit = BASE + 0xc0;
        write(&mut control, &mut h, unit + 0x08, 6);
        write(&mut control, &mut h, unit + 0x0c, 0x1000);
        write(&mut control, &mut h, unit + 0x10, 16);
        write(&mut control, &mut h, unit, 1);
        advance(&mut control, &mut h, 10);
        let row: Vec<u16> = (0..8)
            .map(|x| u16::from_le_bytes([h.ram[0x1000 + 2 * x], h.ram[0x1001 + 2 * x]]))
            .collect();
        // Sites B G G R | G G B G: the red half lights its R site, the blue
        // half its B site.
        assert_eq!(
            row,
            [BLACK, BLACK, BLACK, WHITE, BLACK, BLACK, WHITE, BLACK]
        );
    }
}
