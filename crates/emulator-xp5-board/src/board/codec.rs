//! The XP5 video codec IP (kind `xp5-codec`): the H.264 core at 0xffd15000
//! with its shared stream/image interface at 0xffd15e00..0xffd15ff4, the HEVC
//! core at 0xffd14000, and the codec address-high words 0xff90e710..0xff90e71c.
//!
//! The register contract is the X-T5 4.31 driver's (helpers 0x13cbee0..0x13cf474,
//! open 0x13cfe84, configure 0x13d0220/0x13cf638, frame start 0x13d197c, ISR
//! 0x13d1da0, completion task 0x13d200c). Register names below are the driver's
//! own strings. Only the H.264 encode path is modelled: the HEVC core and the
//! decode path keep plain register words.
//!
//! A start (`H264_CMD` bit 15) encodes one picture with the workspace software
//! encoder ([`Direct`]) after `run_ticks`:
//!
//! - The picture is the two-plane 8-bit 4:2:0 input at the last image-bank
//!   addresses written with `IMG_WIDX` 0 (Y) and 1 (C), with the pitch in
//!   `IMG_LINE_OFS` and address bits 34..32 from `ExIADDR1` (0xff90e718). The
//!   chroma plane is interleaved Cb, Cr ([`CB_FIRST`]); the order is inferred.
//! - With `XT5_SCENE` set and `XT5_MOVIE_PATH` not `real`, the host scene at
//!   the programmed size replaces the guest picture (a functional shortcut, as
//!   for the JPEG encoder: the movie develop chain writes no pixels). The scene
//!   is a host input outside the run definition and the checkpoint.
//! - The access unit (SPS, PPS and an IDR slice on each IDR; one non-IDR slice
//!   otherwise) is written with a 4-byte start code before each NAL into the
//!   stream banks `BADR_STR0`/`BADR_STR1` of `STR_BUF_SIZE` bytes each. The
//!   banks are a ping-pong pair: a NAL that reaches the end of one bank
//!   continues at the start of the other, and `0x15e64` bit 12 reports the
//!   bank of the next byte. The driver rewrites each start code with the NAL's
//!   big-endian length.
//! - One NALU table entry `{type, size including the start code, cumulative
//!   stream offset, 0}` per NAL goes to `BADR_NALU` + `0x15ebc`, followed by a
//!   `0x3f` terminator; `0x15ebc` counts the bytes of real entries until the
//!   driver writes `0x15ea0` = 1.
//! - `0x15120` (pending) and `0x15124` (raw) both report `0x40001` (bit 18 NALU
//!   table ready, bits 1..0 = 1 one picture coded) and the interrupt is driven
//!   while `0x15120 & H264_INT_EN` is non-zero. A write to `0x15120` clears the
//!   written bits in both words (the ISR writes back the value of `0x15124`).
//! - A stop (`H264_CMD` bit 14) ends the recording: the encoder is dropped and
//!   bit 19 (stop done) is reported after `run_ticks`.
//!
//! The encoder holds reference pictures and rate-control history that cannot be
//! serialized, so an exact checkpoint is refused while a recording is open (from
//! the first coded picture until the stop, a new open or a new configuration).
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::graph::{DeviceGraph, DeviceWindow, Mapping};
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_image_codecs::h264::{Direct, DirectConfig};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The two codec register pages.
pub const BASE: u64 = 0xffd1_4000;
pub const SIZE: u64 = 0x2000;
/// ExIADDR0, ExOADDR0, ExIADDR1 and ExOADDR1: address bits 34..32.
pub const ADDRESS_HIGH: u64 = 0xff90_e710;
const ADDRESS_HIGH_SIZE: u64 = 0x10;
/// Register numbers are the driver's offsets from this address.
const ORIGIN: u64 = 0xffd0_0000;

/// The order of the interleaved chroma plane: Cb first (NV12) when true. The
/// contract infers NV12 without a traced producer; flip this to read NV21.
pub const CB_FIRST: bool = true;

const HEVC_STATUS: u64 = 0x1_4120;
const HEVC_RAW_STATUS: u64 = 0x1_4124;
const H264_CMD: u64 = 0x1_5100;
const CMD_START: u32 = 1 << 15;
const CMD_STOP: u32 = 1 << 14;
const H264_STATUS: u64 = 0x1_5120;
const H264_RAW_STATUS: u64 = 0x1_5124;
const H264_INT_EN: u64 = 0x1_5128;
const H264_CORESEL: u64 = 0x1_5200;
const PAR_WIDTH: u64 = 0x1_5300;
const PAR_HEIGHT: u64 = 0x1_5304;
const PAR_FPS: u64 = 0x1_530c;
const PAR_GOP: u64 = 0x1_5310;
const PAR_LEVEL: u64 = 0x1_5334;
const PAR_BITRATE: u64 = 0x1_5554;
const BADR_STR0: u64 = 0x1_5e40;
const BADR_STR1: u64 = 0x1_5e44;
const STR_BUF_SIZE: u64 = 0x1_5e60;
/// Read: bit 12 is the current stream bank. Written by CHGBUF with
/// `(n + 1) << 4` after the driver refills bank `n`.
const STR_BANK: u64 = 0x1_5e64;
const STR_BUF_MODE: u64 = 0x1_5e68;
const BADR_NALU: u64 = 0x1_5e90;
const NALU_RESET: u64 = 0x1_5ea0;
const NALU_BUF_SIZE: u64 = 0x1_5eb0;
const NALU_END: u64 = 0x1_5ebc;
const IMG_WIDX: u64 = 0x1_5ee4;
const IMG_WADR: u64 = 0x1_5ee8;
const IMG_LINE_OFS: u64 = 0x1_5f10;
const PERI_STATUS: u64 = 0x1_5fd0;

/// Status bits of `0x15120`/`0x15124`.
const ONE_PICTURE: u32 = 1;
const NALU_READY: u32 = 1 << 18;
const STOP_DONE: u32 = 1 << 19;
const STATUS_BITS: u32 = ONE_PICTURE | NALU_READY | STOP_DONE;
const NALU_TERMINATOR: u32 = 0x3f;
const ENTRY: u32 = 16;
/// The driver caches at most this many bytes of an SPS and of a PPS (avcC
/// builder 0x2dbca5c).
const SPS_LIMIT: usize = 64;
const PPS_LIMIT: usize = 32;

const RECORDING: &str = "xp5-codec: the H.264 encoder holds reference pictures and rate-control \
     state that cannot be checkpointed while a movie is recording; stop the recording first";

/// A packed 4:2:2 (Y0, Y1, Cb, Cr) host picture of the given size.
pub type SceneSource = fn(usize, usize) -> Option<Vec<u8>>;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    /// Plain register words by driver offset; zero words are absent.
    words: BTreeMap<u64, u32>,
    /// ExIADDR0, ExOADDR0, ExIADDR1, ExOADDR1 (bits 2..0).
    high: [u32; 4],
    /// Image-bank addresses by `IMG_WIDX` value.
    images: BTreeMap<u32, u32>,
    /// `0x15120`: pending status, write one to clear.
    pending: u32,
    /// `0x15124`: raw status.
    raw: u32,
    /// `0x15ebc`: bytes of NALU table entries since the last reset.
    nalu_bytes: u32,
    /// Bytes written to the stream since the last configuration.
    stream_position: u64,
    /// The stream bank of the next byte.
    bank: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    due: Option<u64>,
    /// A start is waiting for `due`.
    encode: bool,
    /// A stop is waiting for `due`.
    stop: bool,
    pictures: u64,
}

pub struct Codec {
    irq: u32,
    ticks: u64,
    /// Host picture source; not part of the device state.
    scene: Option<SceneSource>,
    /// The open recording's encoder; refuses checkpoints while present.
    encoder: Option<Direct>,
    state: State,
}

impl Codec {
    pub fn new(irq: u32, ticks: u64) -> Result<Self, String> {
        if !(32..1020).contains(&irq) || ticks == 0 {
            return Err("invalid XP5 codec interrupt or run_ticks".into());
        }
        Ok(Self { irq, ticks, scene: None, encoder: None, state: State::default() })
    }
    /// Encodes the picture `scene` gives instead of the guest input when it
    /// gives one of the programmed size.
    pub fn with_scene(mut self, scene: SceneSource) -> Self {
        self.scene = Some(scene);
        self
    }
    fn word(&self, register: u64) -> u32 {
        self.state.words.get(&register).copied().unwrap_or(0)
    }
    fn set_word(&mut self, register: u64, value: u32) {
        if value == 0 {
            self.state.words.remove(&register);
        } else {
            self.state.words.insert(register, value);
        }
    }
    fn update_irq(&self, cx: &mut Cx<'_>) {
        cx.set_irq(self.irq, self.state.pending & self.word(H264_INT_EN) != 0);
    }
    /// A new open or configuration starts a new stream and a new sequence.
    fn restart_stream(&mut self) {
        self.encoder = None;
        self.state.stream_position = 0;
        self.state.bank = 0;
    }
    fn high(&self, index: usize) -> u64 {
        u64::from(self.state.high[index] & 7) << 32
    }
    fn settings(&self) -> DirectConfig {
        DirectConfig {
            width: (self.word(PAR_WIDTH) & 0x1fff) as usize,
            height: (self.word(PAR_HEIGHT) & 0xfff) as usize,
            frames_per_second: self.word(PAR_FPS) & 0x7ff,
            bitrate: (self.word(PAR_BITRATE) & 0x7_ffff) << 10,
            gop: (self.word(PAR_GOP) & 0x3ff).max(1),
            level_idc: (self.word(PAR_LEVEL) & 0xff) as u8,
        }
    }

    /// The input picture as tight I420.
    fn picture(&self, cx: &mut Cx<'_>, width: usize, height: usize) -> Result<Vec<u8>, String> {
        if let Some(packed) = self
            .scene
            .and_then(|scene| scene(width, height))
            .filter(|p| p.len() == width * height * 2)
        {
            return Ok(packed_to_i420(&packed, width, height));
        }
        let y = self.state.images.get(&0).copied().ok_or("no luma address (IMG_WIDX 0)")?;
        let c = self.state.images.get(&1).copied().ok_or("no chroma address (IMG_WIDX 1)")?;
        let pitch = self.word(IMG_LINE_OFS) as usize;
        if pitch < width {
            return Err(format!("IMG_LINE_OFS {pitch} is less than the width {width}"));
        }
        let high = self.high(2);
        let luma = cx.ram_bytes(high | u64::from(y), pitch * (height - 1) + width)?;
        let chroma = cx.ram_bytes(high | u64::from(c), pitch * (height / 2 - 1) + width)?;
        Ok(semi_planar_to_i420(&luma, &chroma, pitch, width, height))
    }

    /// Code one picture and publish it in the stream banks and NALU table.
    fn encode(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let settings = self.settings();
        if self.word(STR_BUF_MODE) != 0 {
            return Err("STR_BUF_MODE 1 (no NALU table) is not modelled".into());
        }
        if self.encoder.as_ref().is_some_and(|e| e.config() != settings) {
            cx.log("xp5-codec: H.264 parameters changed within a recording; new sequence");
            self.encoder = None;
        }
        if self.encoder.is_none() {
            let mut encoder = Direct::new(settings)?;
            encoder.request_keyframe();
            cx.log(&format!("xp5-codec: H.264 sequence {settings:?}"));
            self.encoder = Some(encoder);
        }
        let picture = self.picture(cx, settings.width, settings.height)?;
        let unit = self.encoder.as_mut().expect("encoder").encode_i420(&picture)?;
        let nals = nal_units(&unit)?;

        let size = u64::from(self.word(STR_BUF_SIZE));
        if !size.is_power_of_two() || !(0x1000..=0x1000_0000).contains(&size) {
            return Err(format!("STR_BUF_SIZE {size:#x} is not a power of two in 4 KiB..256 MiB"));
        }
        let table = self.high(3) | u64::from(self.word(BADR_NALU));
        let table_size = u64::from(self.word(NALU_BUF_SIZE));
        let first = u64::from(self.state.nalu_bytes);
        if first + u64::from(ENTRY) * (nals.len() as u64 + 1) > table_size {
            return Err(format!("NALU table of {table_size:#x} bytes is full"));
        }
        for (index, nal) in nals.iter().enumerate() {
            let kind = u32::from(nal[0] & 31);
            let start = self.state.stream_position;
            if matches!(kind, 7 | 8) && start % size + nal.len() as u64 + 4 >= size {
                cx.log(&format!(
                    "xp5-codec: parameter set type {kind} wraps across stream banks; the driver does not join it"
                ));
            }
            self.write_stream(cx, &[0, 0, 0, 1], size)?;
            self.write_stream(cx, nal, size)?;
            let entry = [kind, nal.len() as u32 + 4, start as u32, 0];
            cx.ram_write(table + first + u64::from(ENTRY) * index as u64, &words(&entry))?;
        }
        let end = first + u64::from(ENTRY) * nals.len() as u64;
        cx.ram_write(table + end, &words(&[NALU_TERMINATOR, 0, 0, 0]))?;
        self.state.nalu_bytes = (end as u32) & 0x7ff_ffff;
        self.state.pictures += 1;
        Ok(())
    }

    /// Write bytes at the stream position, continuing in the other bank at
    /// the end of a bank.
    fn write_stream(&mut self, cx: &mut Cx<'_>, mut bytes: &[u8], size: u64) -> Result<(), String> {
        while !bytes.is_empty() {
            let position = self.state.stream_position % size;
            let bank = if self.state.bank == 0 { BADR_STR0 } else { BADR_STR1 };
            let base = self.high(3) | u64::from(self.word(bank));
            let count = bytes.len().min((size - position) as usize);
            cx.ram_write(base + position, &bytes[..count])?;
            bytes = &bytes[count..];
            self.state.stream_position += count as u64;
            if self.state.stream_position.is_multiple_of(size) {
                self.state.bank ^= 1;
            }
        }
        Ok(())
    }

    fn register_read(&self, register: u64) -> u32 {
        match register {
            H264_STATUS => self.state.pending,
            H264_RAW_STATUS => self.state.raw,
            HEVC_STATUS | HEVC_RAW_STATUS | PERI_STATUS => 0,
            STR_BANK => self.state.bank << 12,
            NALU_END => self.state.nalu_bytes,
            NALU_RESET => 0,
            H264_CMD => self.word(H264_CMD) & !(CMD_START | CMD_STOP),
            r => self.word(r),
        }
    }

    fn register_write(&mut self, cx: &mut Cx<'_>, register: u64, value: u32) {
        match register {
            H264_STATUS => {
                self.state.pending &= !value;
                self.state.raw &= !value;
                self.update_irq(cx);
            }
            // The ISR writes back what it read; nothing is ever pending here.
            HEVC_STATUS | PERI_STATUS | STR_BANK => {}
            NALU_RESET => {
                if value & 1 != 0 {
                    self.state.nalu_bytes = 0;
                }
            }
            H264_INT_EN => {
                self.set_word(register, value);
                self.update_irq(cx);
            }
            H264_CORESEL | STR_BUF_SIZE => {
                self.set_word(register, value);
                self.restart_stream();
            }
            IMG_WADR => {
                let index = self.word(IMG_WIDX) & 0xff;
                self.state.images.insert(index, value);
                self.set_word(register, value);
            }
            H264_CMD => {
                if value & (CMD_START | CMD_STOP) != 0 {
                    if value & CMD_START != 0 {
                        if self.state.encode {
                            return cx.fail("xp5-codec: H.264 start while a picture is in flight");
                        }
                        self.state.encode = true;
                    }
                    if value & CMD_STOP != 0 {
                        self.state.stop = true;
                    }
                    if self.state.due.is_none() {
                        match cx.now().checked_add(self.ticks) {
                            Some(at) => self.state.due = Some(at),
                            None => return cx.fail("xp5-codec deadline overflow"),
                        }
                    }
                }
                self.set_word(register, value & !(CMD_START | CMD_STOP));
            }
            r => self.set_word(r, value),
        }
    }

    fn check(&self, state: &State) -> Result<(), String> {
        let in_window = |r: &u64| (BASE - ORIGIN..BASE - ORIGIN + SIZE).contains(r) && r & 3 == 0;
        if state.words.iter().any(|(r, v)| !in_window(r) || *v == 0)
            || state.high.iter().any(|h| *h > 7)
            || state.images.keys().any(|k| *k > 0xff)
            || state.bank > 1
            || state.nalu_bytes & !0x7ff_fff0 != 0
            || (state.pending | state.raw) & !STATUS_BITS != 0
            || (state.encode || state.stop) != state.due.is_some()
        {
            return Err("invalid XP5 codec checkpoint".into());
        }
        Ok(())
    }
}

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// The NAL payloads of an Annex B access unit (types 7, 8, 1 and 5 only; SEI,
/// delimiters and filler are dropped). The driver caches at most 64 bytes of
/// an SPS and 32 of a PPS and hangs on any other cached type.
fn nal_units(stream: &[u8]) -> Result<Vec<&[u8]>, String> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= stream.len() {
        if stream[i..i + 3] == [0, 0, 1] {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut nals = Vec::new();
    for (n, &start) in starts.iter().enumerate() {
        let mut end = starts.get(n + 1).map_or(stream.len(), |next| next - 3);
        while end > start && stream[end - 1] == 0 {
            end -= 1;
        }
        let nal = &stream[start..end];
        match nal.first().map(|b| b & 31) {
            Some(7) if nal.len() <= SPS_LIMIT => nals.push(nal),
            Some(8) if nal.len() <= PPS_LIMIT => nals.push(nal),
            Some(1 | 5) => nals.push(nal),
            Some(6 | 9 | 12) => {}
            _ => return Err("encoder produced an unsupported NAL or parameter-set length".into()),
        }
    }
    if nals.iter().filter(|n| matches!(n[0] & 31, 1 | 5)).count() != 1 {
        return Err("encoder must produce exactly one coded slice per picture".into());
    }
    Ok(nals)
}

/// Two-plane 4:2:0 (Y, then interleaved chroma with one pitch) to tight I420.
fn semi_planar_to_i420(luma: &[u8], chroma: &[u8], pitch: usize, width: usize, height: usize) -> Vec<u8> {
    let y = width * height;
    let mut out = vec![0; y * 3 / 2];
    for row in 0..height {
        out[row * width..][..width].copy_from_slice(&luma[row * pitch..][..width]);
    }
    let (cb, cr) = if CB_FIRST { (0, 1) } else { (1, 0) };
    let half = width / 2;
    for row in 0..height / 2 {
        let line = &chroma[row * pitch..][..width];
        for col in 0..half {
            out[y + row * half + col] = line[col * 2 + cb];
            out[y + y / 4 + row * half + col] = line[col * 2 + cr];
        }
    }
    out
}

/// Packed Y0, Y1, Cb, Cr 4:2:2 to tight I420, averaging chroma row pairs.
fn packed_to_i420(packed: &[u8], width: usize, height: usize) -> Vec<u8> {
    let y = width * height;
    let half = width / 2;
    let mut out = vec![0; y * 3 / 2];
    for row in 0..height {
        for col in 0..half {
            let p = (row * half + col) * 4;
            out[row * width + col * 2] = packed[p];
            out[row * width + col * 2 + 1] = packed[p + 1];
            if row % 2 == 0 {
                let q = p + width * 2;
                let c = row / 2 * half + col;
                out[y + c] = (u16::from(packed[p + 2]) + u16::from(packed[q + 2])).div_ceil(2) as u8;
                out[y + y / 4 + c] =
                    (u16::from(packed[p + 3]) + u16::from(packed[q + 3])).div_ceil(2) as u8;
            }
        }
    }
    out
}

impl Device for Codec {
    fn name(&self) -> &'static str {
        "xp5-codec"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if size != 4 || address & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        if (ADDRESS_HIGH..ADDRESS_HIGH + ADDRESS_HIGH_SIZE).contains(&address) {
            return u64::from(self.state.high[((address - ADDRESS_HIGH) / 4) as usize]);
        }
        u64::from(self.register_read(address - ORIGIN))
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Ok(value) = u32::try_from(value) else {
            return cx.unmodeled();
        };
        if size != 4 || address & 3 != 0 {
            return cx.unmodeled();
        }
        if (ADDRESS_HIGH..ADDRESS_HIGH + ADDRESS_HIGH_SIZE).contains(&address) {
            if value > 7 {
                return cx.fail(format!("xp5-codec: address-high word {address:#x} = {value:#x} exceeds 3 bits"));
            }
            self.state.high[((address - ADDRESS_HIGH) / 4) as usize] = value;
            return;
        }
        self.register_write(cx, address - ORIGIN, value);
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.state.due.is_none_or(|at| at > cx.now()) {
            return;
        }
        self.state.due = None;
        if std::mem::take(&mut self.state.encode) {
            if let Err(error) = self.encode(cx) {
                return cx.fail(format!("xp5-codec: H.264 encode failed: {error}"));
            }
            self.state.pending |= NALU_READY | ONE_PICTURE;
            self.state.raw |= NALU_READY | ONE_PICTURE;
        }
        if std::mem::take(&mut self.state.stop) {
            if self.encoder.take().is_some() {
                cx.log(&format!("xp5-codec: recording stopped after {} pictures", self.state.pictures));
            }
            self.state.pending |= STOP_DONE;
            self.state.raw |= STOP_DONE;
        }
        self.update_irq(cx);
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.due
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn diagnostic_state(&self) -> Value {
        let mut state = self.save();
        if let Value::Object(map) = &mut state {
            map.insert("recording".into(), Value::Bool(self.encoder.is_some()));
        }
        state
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.check(&state)?;
        self.state = state;
        self.encoder = None;
        Ok(())
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        if self.encoder.is_some() {
            return Err(RECORDING.into());
        }
        Checkpoint::encode(self.name(), &self.state)
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        if !saved.children().is_empty() || !saved.blobs().is_empty() {
            return Err(format!("{} snapshot has unexpected parts", self.name()));
        }
        let state: State = saved.decode(self.name())?;
        self.check(&state)
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        self.state = saved.decode(self.name())?;
        self.encoder = None;
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.checkpoint()?.state())
    }
}

/// The `XT5_SCENE` scene at the programmed size, unless `XT5_MOVIE_PATH` is
/// `real` (read once per process).
pub fn host_movie_422(width: usize, height: usize) -> Option<Vec<u8>> {
    use std::sync::OnceLock;
    static REAL: OnceLock<bool> = OnceLock::new();
    let real = *REAL.get_or_init(|| {
        let real = std::env::var("XT5_MOVIE_PATH").is_ok_and(|v| v == "real");
        if real {
            eprintln!("XT5_MOVIE_PATH: real (the H.264 encoder reads the guest Y/C planes)");
        }
        real
    });
    if real {
        return None;
    }
    fujimod_emulator_imaging::frame::render_422(
        fujimod_emulator_imaging::frame::host_scene()?,
        width,
        height,
    )
}

/// Kind `xp5-codec`: the window 0xffd14000..0xffd16000 with options `irq`
/// (the GIC INTID) and `run_ticks`; the model also owns 0xff90e710..0xff90e71f.
pub fn register(bus: &mut DeviceGraph, window: &DeviceWindow) -> Result<(), String> {
    if window.base != BASE
        || window.size != SIZE
        || window.options.keys().any(|key| !matches!(key.as_str(), "irq" | "run_ticks"))
    {
        return Err("invalid XP5 codec window or options".into());
    }
    let number = |key: &str| {
        window
            .options
            .get(key)
            .and_then(toml::Value::as_integer)
            .and_then(|n| u64::try_from(n).ok())
            .ok_or_else(|| format!("XP5 codec requires {key}"))
    };
    let irq = u32::try_from(number("irq")?).map_err(|e| e.to_string())?;
    let codec = Codec::new(irq, number("run_ticks")?)?.with_scene(host_movie_422);
    bus.register_windows(
        vec![
            Mapping { base: BASE, size: SIZE },
            Mapping { base: ADDRESS_HIGH, size: ADDRESS_HIGH_SIZE },
        ],
        Box::new(codec),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;

    const RAM: u64 = 0x8000_0000;
    const Y_AT: u64 = RAM + 0x1000;
    const C_AT: u64 = RAM + 0x4000;
    const STR0_AT: u64 = RAM + 0x10_0000;
    const STR1_AT: u64 = RAM + 0x20_0000;
    const NALU_AT: u64 = RAM + 0x30_0000;
    const BANK: u32 = 0x1000;
    const PITCH: usize = 80;
    const W: usize = 64;
    const H: usize = 48;
    const IRQ: u32 = 210;

    fn host() -> RecordingHost {
        RecordingHost { ram: vec![0; 4 << 20], ram_base: RAM, ..RecordingHost::default() }
    }
    fn write(c: &mut Codec, h: &mut RecordingHost, address: u64, value: u32) {
        let mut cx = Cx::new(h, 0, 0, 0);
        c.write(&mut cx, address, 4, value.into());
        assert_eq!(cx.refusal(), None, "{address:#x}");
    }
    fn read(c: &mut Codec, h: &mut RecordingHost, address: u64) -> u32 {
        let mut cx = Cx::new(h, 0, 0, 0);
        let value = c.read(&mut cx, address, 4) as u32;
        assert_eq!(cx.refusal(), None, "{address:#x}");
        value
    }
    fn reg(c: &mut Codec, h: &mut RecordingHost, register: u64, value: u32) {
        write(c, h, ORIGIN + register, value);
    }
    fn get(c: &mut Codec, h: &mut RecordingHost, register: u64) -> u32 {
        read(c, h, ORIGIN + register)
    }

    /// The driver's open and configure sequences (0x13cfe84, 0x13cf638,
    /// 0x13d0290..0x13d04c4) for a small picture, with all-intra GOP 2.
    fn open_and_configure(c: &mut Codec, h: &mut RecordingHost) {
        reg(c, h, 0x1_4088, 1);
        reg(c, h, 0x1_4080, 1);
        reg(c, h, 0x1_4008, 1);
        reg(c, h, H264_CORESEL, 1);
        reg(c, h, H264_INT_EN, 0x010c_0000);
        reg(c, h, 0x1_5fd8, 0xffff);
        for address in [0xff90_e710, 0xff90_e714, 0xff90_e718, 0xff90_e71c] {
            write(c, h, address, 0);
        }
        reg(c, h, 0x1_5ef4, 1);
        reg(c, h, 0x1_5ee0, 4);
        for set in 0..2 {
            for index in 0..8u32 {
                reg(c, h, IMG_WIDX, set << 5 | index);
                reg(c, h, IMG_WADR, 0x9000_0000 + index * 0x1000);
            }
        }
        reg(c, h, IMG_LINE_OFS, PITCH as u32);
        reg(c, h, 0x1_5e00, 0x9100_0000);
        reg(c, h, 0x1_5e04, 0x9200_0000);
        reg(c, h, 0x1_5f14, 0x1_0000);
        reg(c, h, BADR_STR0, STR0_AT as u32);
        reg(c, h, BADR_STR1, STR1_AT as u32);
        reg(c, h, STR_BUF_SIZE, BANK);
        reg(c, h, STR_BUF_MODE, 0);
        reg(c, h, BADR_NALU, NALU_AT as u32);
        reg(c, h, NALU_BUF_SIZE, 0x10_0000);
        for (register, value) in [
            (PAR_WIDTH, W as u32),
            (PAR_HEIGHT, H as u32),
            (PAR_FPS, 30),
            (PAR_GOP, 2),
            (0x1_5314, 2),
            (0x1_5320, 20_000_000 >> 14),
            (PAR_LEVEL, 40),
            (0x1_5358, 1),
            (0x1_5368, 0x2000 | 5 << 10 | 1 << 8 | 1),
            (0x1_536c, 0x0101),
            (0x1_5374, 3),
            (0x1_5378, 1001),
            (0x1_5380, 60000),
            (0x1_5394, 0x1ff),
            (PAR_BITRATE, 20_000_000 >> 10),
            (0x1_555c, 20_000_000 >> 10),
            (0x1_5680, 0x544d_3234),
            (0x1_5684, 0x3533_3320),
        ] {
            reg(c, h, register, value);
        }
    }

    /// A moving gradient frame in NV12 at Y_AT/C_AT.
    fn frame(h: &mut RecordingHost, n: usize) {
        for row in 0..H {
            let line: Vec<u8> = (0..PITCH).map(|x| (16 + (x * 3 + row + n * 4) % 200) as u8).collect();
            let at = (Y_AT - RAM) as usize + row * PITCH;
            h.ram[at..at + PITCH].copy_from_slice(&line);
        }
        for row in 0..H / 2 {
            let line: Vec<u8> = (0..PITCH).map(|x| if x % 2 == 0 { 90 } else { 170 }).collect();
            let at = (C_AT - RAM) as usize + row * PITCH;
            h.ram[at..at + PITCH].copy_from_slice(&line);
        }
    }

    /// ENC_S 0x13d197c, then the deadline, then the ISR and the completion
    /// task. Returns the NALU entries {type, size, offset} read from the table
    /// and checks the terminator.
    fn encode_one(c: &mut Codec, h: &mut RecordingHost) -> Vec<[u32; 3]> {
        reg(c, h, IMG_LINE_OFS, PITCH as u32);
        reg(c, h, IMG_WIDX, 0);
        reg(c, h, IMG_WADR, Y_AT as u32);
        reg(c, h, IMG_WIDX, 1);
        reg(c, h, IMG_WADR, C_AT as u32);
        reg(c, h, 0x1_5ef4, 1);
        let bank = get(c, h, STR_BANK) >> 12 & 1;
        assert_eq!(bank, c.state.bank);
        write(c, h, 0xff90_e718, 0);
        reg(c, h, H264_CMD, CMD_START);
        assert_eq!(get(c, h, H264_STATUS), 0);
        h.now = c.next_deadline(h.now).expect("a completion deadline");
        let mut cx = Cx::new(h, 0, 0, 0);
        c.advance(&mut cx);
        assert_eq!(cx.refusal(), None);
        assert_eq!(h.irqs.last(), Some(&(IRQ, true)));
        // ISR 0x13d1da0.
        assert_eq!(get(c, h, 0x1_5fd0), 0);
        let status = get(c, h, H264_STATUS);
        let raw = get(c, h, H264_RAW_STATUS);
        assert_eq!((status, raw), (0x40001, 0x40001));
        assert_eq!((get(c, h, HEVC_STATUS), get(c, h, HEVC_RAW_STATUS)), (0, 0));
        reg(c, h, 0x1_5fd0, 0);
        reg(c, h, H264_STATUS, raw);
        reg(c, h, HEVC_STATUS, 0);
        assert_eq!(h.irqs.last(), Some(&(IRQ, false)));
        assert_eq!(get(c, h, H264_STATUS), 0);
        // Task 0x13d200c: parse the table, then reset it.
        let end = get(c, h, NALU_END) & !0xf;
        let table = h.bytes(NALU_AT, end as usize + 16).unwrap();
        let word = |i: usize| u32::from_le_bytes(table[i * 4..i * 4 + 4].try_into().unwrap());
        let entries: Vec<[u32; 3]> =
            (0..end as usize / 16).map(|e| [word(e * 4), word(e * 4 + 1), word(e * 4 + 2)]).collect();
        assert_eq!(word(end as usize / 4), NALU_TERMINATOR);
        reg(c, h, NALU_RESET, 1);
        assert_eq!(get(c, h, NALU_END), 0);
        entries
    }

    /// The bytes of one table entry, joined across the stream banks.
    fn nal_bytes(h: &RecordingHost, [_, size, offset]: [u32; 3]) -> Vec<u8> {
        let position = u64::from(offset) % u64::from(BANK);
        let bank = (u64::from(offset) / u64::from(BANK)) % 2;
        let base = |b: u64| if b == 0 { STR0_AT } else { STR1_AT };
        let first = (u64::from(size)).min(u64::from(BANK) - position);
        let mut bytes = h.bytes(base(bank) + position, first as usize).unwrap();
        bytes.extend(h.bytes(base(bank ^ 1), (u64::from(size) - first) as usize).unwrap());
        bytes
    }

    #[test]
    fn two_pictures_follow_the_driver_contract_and_decode() {
        let mut c = Codec::new(IRQ, 1_000_000).unwrap();
        let mut h = host();
        open_and_configure(&mut c, &mut h);
        let mut decoder = fujimod_emulator_image_codecs::h264::decode::Decoder::new(&[]).unwrap();
        let mut types = Vec::new();
        let mut expected_offset = 0;
        for n in 0..3 {
            frame(&mut h, n);
            let entries = encode_one(&mut c, &mut h);
            let mut unit = Vec::new();
            for entry in &entries {
                assert_eq!(entry[2], expected_offset, "cumulative offset");
                expected_offset += entry[1];
                let bytes = nal_bytes(&h, *entry);
                assert_eq!(bytes[..4], [0, 0, 0, 1], "4-byte start code");
                assert_eq!(u32::from(bytes[4] & 31), entry[0], "NAL type");
                match entry[0] {
                    7 => assert!(bytes.len() - 4 <= SPS_LIMIT),
                    8 => assert!(bytes.len() - 4 <= PPS_LIMIT),
                    _ => {}
                }
                unit.extend(bytes);
            }
            types.push(entries.iter().map(|e| e[0]).collect::<Vec<_>>());
            let picture = decoder.decode(&unit).unwrap();
            assert_eq!((picture.sequence.width, picture.sequence.height), (W, H));
            // The decoded chroma follows the NV12 Cb, Cr order.
            let luma = W * H;
            let (cb, cr) = (picture.i420[luma + 10], picture.i420[luma + luma / 4 + 10]);
            assert!(cb.abs_diff(90) < 12 && cr.abs_diff(170) < 12, "{cb} {cr}");
        }
        // GOP 2: IDR with parameter sets, P, then IDR with parameter sets again.
        assert_eq!(types, [vec![7, 8, 5], vec![1], vec![7, 8, 5]]);
        assert_eq!(get(&mut c, &mut h, STR_BANK) >> 12, c.state.bank);
        // A recording refuses an exact checkpoint.
        assert_eq!(c.checkpoint().unwrap_err(), RECORDING);
        // Stop: bit 19 and the interrupt, then the encoder is gone.
        reg(&mut c, &mut h, H264_CMD, CMD_STOP);
        h.now = c.next_deadline(h.now).unwrap();
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        c.advance(&mut cx);
        assert_eq!(cx.refusal(), None);
        assert_eq!(h.irqs.last(), Some(&(IRQ, true)));
        assert_eq!(get(&mut c, &mut h, H264_STATUS), STOP_DONE);
        reg(&mut c, &mut h, H264_STATUS, STOP_DONE);
        assert_eq!(h.irqs.last(), Some(&(IRQ, false)));
        // Idle: the checkpoint round-trips.
        let saved = c.checkpoint().unwrap();
        let mut restored = Codec::new(IRQ, 1_000_000).unwrap();
        restored.restore_checkpoint(&saved).unwrap();
        assert_eq!(restored.checkpoint().unwrap().state(), saved.state());
        assert_eq!(get(&mut restored, &mut h, BADR_NALU), NALU_AT as u32);
    }

    #[test]
    fn a_nal_that_reaches_the_end_of_a_bank_continues_in_the_other() {
        let mut c = Codec::new(IRQ, 10).unwrap();
        let mut h = host();
        open_and_configure(&mut c, &mut h);
        let mut decoder = fujimod_emulator_image_codecs::h264::decode::Decoder::new(&[]).unwrap();
        let mut wrapped = false;
        for n in 0..200 {
            if wrapped && c.state.bank == 0 {
                break;
            }
            frame(&mut h, n);
            let before = c.state.bank;
            let mut unit = Vec::new();
            for entry in encode_one(&mut c, &mut h) {
                let position = entry[2] % BANK;
                if position + entry[1] >= BANK {
                    wrapped = true;
                }
                let bytes = nal_bytes(&h, entry);
                assert_eq!(bytes[..4], [0, 0, 0, 1]);
                unit.extend(bytes);
            }
            // Every picture, split or not, decodes from the joined pieces.
            decoder.decode(&unit).unwrap();
            if c.state.bank != before {
                assert_eq!(get(&mut c, &mut h, STR_BANK), c.state.bank << 12);
            }
        }
        assert!(wrapped, "200 pictures of 64x48 fill a 4 KiB bank");
    }

    #[test]
    fn the_scene_replaces_the_guest_picture() {
        fn grey(width: usize, height: usize) -> Option<Vec<u8>> {
            Some([200, 200, 128, 128].repeat(width * height / 2))
        }
        let mut c = Codec::new(IRQ, 10).unwrap().with_scene(grey);
        let mut h = host();
        open_and_configure(&mut c, &mut h);
        let entries = encode_one(&mut c, &mut h);
        let unit: Vec<u8> = entries.iter().flat_map(|e| nal_bytes(&h, *e)).collect();
        let mut decoder = fujimod_emulator_image_codecs::h264::decode::Decoder::new(&[]).unwrap();
        let picture = decoder.decode(&unit).unwrap();
        assert!(picture.i420[..W * H].iter().all(|y| y.abs_diff(200) < 6));
    }

    #[test]
    fn the_xt5_codec_window_opens() {
        #[derive(serde::Deserialize)]
        struct Fragment {
            device: Vec<DeviceWindow>,
        }
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../catalog/firmware/xt5/4.31/xp5-boot/manifest.d/61-codec.toml"
        );
        let fragment: Fragment = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let window = fragment.device.iter().find(|w| w.kind == "xp5-codec").unwrap();
        let mut graph = DeviceGraph::new();
        register(&mut graph, window).unwrap();
        assert!(graph.at(0xffd1_5100).is_ok() && graph.at(0xff90_e71c).is_ok());
        let mut extra = window.clone();
        extra.options.insert("format".into(), toml::Value::Integer(1));
        assert!(register(&mut DeviceGraph::new(), &extra).is_err());
    }
}
