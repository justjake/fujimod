//! The XP5 video codec IP (kind `xp5-codec`): the H.264 core at 0xffd15000
//! with its shared stream/image interface at 0xffd15e00..0xffd15ff4, the HEVC
//! core at 0xffd14000, and the codec address-high words 0xff90e710..0xff90e71c.
//!
//! The register contract is the X-T5 4.31 driver's (helpers 0x13cbee0..0x13cf474,
//! open 0x13cfe84, configure 0x13d0220/0x13cf638, frame start 0x13d197c, ISR
//! 0x13d1da0, completion task 0x13d200c). Register names below are the driver's
//! own strings. The H.264 and HEVC encode paths and the H.264 decode path are
//! modelled; an HEVC decode (the HEIF library, 0x2dd8a38 -> DecStart
//! 0x13d14e0, and H.265 movie playback) is not.
//!
//! HEVC (recmv opens codec 1 when rec+0x8390, copied from rec+0x40c4, is
//! non-zero; the movie file-format setting 0x62b = 1 or 3 selects H.265):
//! the frame start writes the image addresses (`IMG_WIDX`/`IMG_WADR`, shared
//! with H.264), then `HEVC_CMD` 0x14100 bit 0; the flush/stop 0x13d1b0c
//! (0x13d1bbc) writes the same bit without new image addresses (in decode
//! mode the same command starts a decode, as H.264 bit 14 does). The close
//! (H265Stop 0x13d15f0, task type 4) writes HEVC_CMD bit 1 / H264_CMD bit 12
//! and waits for no interrupt. The picture size is read from
//! 0x14310 (bits 13..0 width, bits 28..16 height, leaf 0x13ca18c; inferred
//! from the field widths, not traced). Status is `0x14120` (pending) and
//! `0x14124` (raw) with the H.264 bit layout (leaves 0x13c9c94/0x13c9da0);
//! the shared ISR and task read both. The pictures are coded by the
//! workspace I_PCM encoder ([`hevc`]): VPS, SPS and PPS (NALU types 32..34)
//! every 15 pictures and one IDR_W_RADL (19) slice per picture, which the
//! completion task accepts as VCL (0x13d178c: types 0, 1, 19, 21). The
//! encoder ignores the HEVC_ENC_* rate, GOP, profile and 4:2:2 / 10-bit
//! registers: the stream is 8-bit 4:2:0 Main, lossless and large.
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
//!   for the JPEG encoder). With `XT5_MOVIE_PATH=real` the movie develop chain
//!   (MOVDC -> RIFP inline -> RESZ -> TNR, emulator-xp5-imaging
//!   imaging_movie.rs) writes the input picture. The scene is a host input
//!   outside the run definition and the checkpoint.
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
//! - `0x15120` (pending) and `0x15124` (raw) both report bit 18 (NALU table
//!   ready) after each picture, and the interrupt is driven while
//!   `0x15120 & H264_INT_EN` is non-zero. Bits 1..0 report a stream bank that
//!   the picture filled (bit 0 = bank 0, bit 1 = bank 1) and are clear
//!   otherwise. The completion task calls the driver's bank callback 0x2db5bfc
//!   for them; recmv then refills that bank (op 0x1a end 0x2db3878 -> CHGBUF
//!   0x13d1078) and frees the old one at ADD_STREAM (0x2db4518). Reporting a
//!   full bank on every picture makes recmv rotate its three 64 MiB stream
//!   buffers per frame; the ring pool then refuses the third allocation and
//!   every later ADD_STREAM fails, so no stream reaches the card. A write to `0x15120` clears the
//!   written bits in both words (the ISR writes back the value of `0x15124`).
//! - A stop (`H264_CMD` bit 14) ends the recording: the encoder is dropped and
//!   bit 19 (stop done) is reported after `run_ticks`.
//!
//! H.264 decode (movie playback, play stream 0x56f33bc / 0x57140cc: open
//! 0x13cfe84 with codec 0 and encode 0, configure 0x13d0dbc, DecStart
//! 0x13d14e0 -> task type 2 -> 0x13d1b0c). Register writes of one X-T5 4.31
//! decode (strict run, PLAY on a recorded 4K MOV; FHD differs in the sizes):
//! `H264_CORESEL` = 0 (1 when encoding), `H264_INT_EN` = 0x2803 (bits 13, 11,
//! 1, 0), `0x15ee0` = 1 (image banks), `IMG_WIDX` 0 / 1 and 0x20 / 0x21 = the Y
//! and C addresses of bank 0 for both field sets (0x14a31000 / 0x15229000, one
//! 3840x2176 plane apart), `IMG_LINE_OFS` = 0xf00, `BADR_STR0` = the stream,
//! `STR_BUF_SIZE` = 0x2000000, `STR_BUF_MODE` = 1 (no NALU table), then
//! `H264_CMD` bit 14. The driver arms a 600 ms timeout (0x13d17d4).
//!
//! That is the first-frame session of the playback standby screen. The play
//! session (PMOVDECODE 0x5714364) programs `0x15ee0` = 2 with four pictures
//! (`IMG_WIDX` 0..3 and 0x20..0x23: two banks in each of two sets, all
//! different), `BADR_STR0` / `BADR_STR1` = 0x83ee9000 / 0x87ee9000 and
//! `STR_BUF_SIZE` = 0x4000000: two of the three 64 MiB windows of the stream
//! ring that PMOVFRAME fills, and calls DecStart once per access unit.
//!
//! - The stream is Annex B (MOV_IF_SetStartCodeSlice2 0x56eb83c writes
//!   4-byte start codes): the access units follow each other from the start
//!   of bank 0, as the encoder writes them, and continue in the other bank at
//!   the end of a bank. The first-frame session ends its one unit with a
//!   start code and a zero byte, a NAL of type 0.
//! - Each `H264_CMD` bit 14 in decode mode decodes the unit at the stream
//!   position with the workspace decoder after `run_ticks` and moves the
//!   position past it. A unit ends at the type 0 NAL or at the first NAL of
//!   the next unit (a start code is required after the slice; the hardware
//!   needs none, a limit of the model). The picture is written as two-plane
//!   8-bit 4:2:0 (Y, then interleaved Cb, Cr, [`CB_FIRST`]) at the pitch
//!   `IMG_LINE_OFS` into an image bank: picture n of a session goes to bank
//!   n modulo the bank count of set 0, then of set 1 (the hardware's order
//!   is not traced; the driver takes whichever `0x15ef0` names).
//! - A unit that reaches the end of a stream bank sets that bank's status
//!   bit (bit 0 or 1) with the picture's; the completion task then gives the
//!   bank its next window (0x13d163c: `BADR_STR0` / `BADR_STR1`, `0x15e64`).
//! - Completion is status bit 11 in `0x15120` / `0x15124`. The completion
//!   task (0x13d2620) then reads `0x15ef0` (leaf 0x13ce77c: bit 5 the set,
//!   bits 2..0 the `IMG_WIDX` of the luma plane, so bank = bits 2..1),
//!   looks the bank's addresses up and calls the DecStart callback with
//!   {id, Y address, C address}. Bit 13 is the decode error (0x13d2510);
//!   the model never raises it: a stream it cannot decode stops the run.
//! - The close (`H264_CMD` bit 12), a halt and a new core select end the
//!   session and drop the decoder.
//!
//! The decoder holds reference pictures, so an exact checkpoint is refused
//! while a decode session is open, as while a movie is recording.
//!
//! Halt (`0x15ff4` bit 0, leaf 0x13cf43c): the terminate 0x13d0e60 (recmv
//! 0x2db994c after a recording, the HEIF library 0x2dd890c / 0x2dda6b4), the
//! suspend 0x13d0f84 and the error path of the completion task (0x13d2510) write
//! 1 and then poll `0x15fd4` (leaf 0x13ceeec) until bit 0 reads 1, with no
//! timeout and no interrupt. The terminate then resets the block through the
//! reset controller (0xff90db04 bit 16 or 17). The model drops the work in
//! flight and the open recording at the halt, and bit 0 of `0x15fd4` reads 1
//! until the next core select (the first register write of an open) or
//! command. `0x15fd0`, the pending word the ISR reads with the same layout,
//! stays 0: any of its bits sends the completion task into its error stop.
//!
//! The encoder holds reference pictures and rate-control history that cannot be
//! serialized, so an exact checkpoint is refused while a recording is open (from
//! the first coded picture until the stop, a new open or a new configuration).
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::graph::{DeviceGraph, DeviceWindow, Mapping};
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_image_codecs::h264::decode::{Bounds, Decoder};
use fujimod_emulator_image_codecs::h264::{Direct, DirectConfig};
use fujimod_emulator_image_codecs::hevc::{self, PcmConfig, PcmEncoder};
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

/// HEVC_CMD (leaf 0x13c9b54): bit 0 is written by both the frame start
/// (0x13d1adc, after the image addresses) and the flush/stop 0x13d1b0c
/// (0x13d1bbc); bit 1 is the close (0x13d28b8), which is stored only.
const HEVC_CMD: u64 = 0x1_4100;
const HEVC_GO: u32 = 1;
const HEVC_STATUS: u64 = 0x1_4120;
const HEVC_RAW_STATUS: u64 = 0x1_4124;
const HEVC_INT_EN: u64 = 0x1_4128;
const HEVC_CORESEL: u64 = 0x1_4200;
/// Picture size: bits 13..0 width, bits 28..16 height (leaf 0x13ca18c).
const HEVC_SIZE: u64 = 0x1_4310;
/// Parameter sets precede every this many HEVC pictures (all are IDR).
const HEVC_PARAMETER_INTERVAL: u32 = 15;
const H264_CMD: u64 = 0x1_5100;
const CMD_START: u32 = 1 << 15;
/// Bit 14: the encoder flush / stop, or, in decode mode, the decode start.
const CMD_STOP: u32 = 1 << 14;
/// Bit 12: the close (0x13d28b8), which waits for no interrupt.
const CMD_CLOSE: u32 = 1 << 12;
const H264_STATUS: u64 = 0x1_5120;
const H264_RAW_STATUS: u64 = 0x1_5124;
const H264_INT_EN: u64 = 0x1_5128;
/// 1 selects the encoder, 0 the decoder.
const H264_CORESEL: u64 = 0x1_5200;
const CORESEL_DECODE: u32 = 0;
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
/// The number of image banks (leaf 0x13ce5dc, 1..4).
const IMG_BANKS: u64 = 0x1_5ee0;
const IMG_WIDX: u64 = 0x1_5ee4;
const IMG_WADR: u64 = 0x1_5ee8;
/// Read: the image bank of the last decoded picture (leaf 0x13ce77c).
const IMG_DECODED: u64 = 0x1_5ef0;
const IMG_LINE_OFS: u64 = 0x1_5f10;
const PERI_STATUS: u64 = 0x1_5fd0;
/// Raw peripheral status (leaf 0x13ceeec); bit 0 reports a completed halt.
const PERI_RAW_STATUS: u64 = 0x1_5fd4;
const PERI_HALTED: u32 = 1;
/// Halt request (leaf 0x13cf43c writes bit 0).
const PERI_HALT: u64 = 0x1_5ff4;

/// Status bits of `0x15120`/`0x15124`.
/// Bits 1..0: stream bank 0 / bank 1 filled by this picture.
const BANK_FULL: u32 = 3;
const NALU_READY: u32 = 1 << 18;
const STOP_DONE: u32 = 1 << 19;
/// Decode mode: one picture decoded (task 0x13d2620).
const PICTURE_DECODED: u32 = 1 << 11;
const STATUS_BITS: u32 = BANK_FULL | NALU_READY | STOP_DONE | PICTURE_DECODED;
/// The decode stream is read in pieces of this size until its end marker.
const STREAM_PIECE: usize = 0x10_0000;
const NALU_TERMINATOR: u32 = 0x3f;
const ENTRY: u32 = 16;
/// The driver caches at most this many bytes of an SPS and of a PPS (avcC
/// builder 0x2dbca5c).
const SPS_LIMIT: usize = 64;
const PPS_LIMIT: usize = 32;

const DECODING: &str = "xp5-codec: the H.264 decoder holds reference pictures that cannot be \
     checkpointed while a decode session is open; leave movie playback first";

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
    /// `0x14120`: HEVC pending status, write one to clear.
    #[serde(default, skip_serializing_if = "is_zero")]
    hevc_pending: u32,
    /// `0x14124`: HEVC raw status.
    #[serde(default, skip_serializing_if = "is_zero")]
    hevc_raw: u32,
    /// The start or stop waiting for `due` is for the HEVC core.
    #[serde(default, skip_serializing_if = "is_false")]
    hevc: bool,
    /// `IMG_WADR` was written since the last HEVC command, so the next
    /// HEVC_CMD go starts a picture; otherwise it is a stop.
    #[serde(default, skip_serializing_if = "is_false")]
    hevc_image: bool,
    /// A decode is waiting for `due`.
    #[serde(default, skip_serializing_if = "is_false")]
    decode: bool,
    /// `0x15ef0`: the image bank of the last decoded picture.
    #[serde(default, skip_serializing_if = "is_zero")]
    decoded: u32,
    /// Pictures decoded in this decode session.
    #[serde(default, skip_serializing_if = "is_zero")]
    decoded_pictures: u32,
    /// A halt (`0x15ff4` bit 0) completed and no open or command followed.
    #[serde(default, skip_serializing_if = "is_false")]
    halted: bool,
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
    /// The open HEVC recording's encoder; refuses checkpoints while present.
    hevc_encoder: Option<PcmEncoder>,
    /// The open decode session's decoder; refuses checkpoints while present.
    decoder: Option<Decoder>,
    /// Bank-full bits collected while one picture is written; always zero
    /// between pictures, so it is not part of the device state.
    filled: u32,
    /// `XT5_CODEC_TRACE` set: log every register write (host diagnostic).
    trace: bool,
    state: State,
}

impl Codec {
    pub fn new(irq: u32, ticks: u64) -> Result<Self, String> {
        if !(32..1020).contains(&irq) || ticks == 0 {
            return Err("invalid XP5 codec interrupt or run_ticks".into());
        }
        let trace = std::env::var_os("XT5_CODEC_TRACE").is_some();
        Ok(Self {
            irq,
            ticks,
            scene: None,
            encoder: None,
            hevc_encoder: None,
            decoder: None,
            filled: 0,
            trace,
            state: State::default(),
        })
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
        let h264 = self.state.pending & self.word(H264_INT_EN);
        let hevc = self.state.hevc_pending & self.word(HEVC_INT_EN);
        cx.set_irq(self.irq, h264 | hevc != 0);
    }
    fn recording(&self) -> bool {
        self.encoder.is_some() || self.hevc_encoder.is_some()
    }
    /// A new open or configuration starts a new stream and a new sequence.
    fn restart_stream(&mut self) {
        self.encoder = None;
        self.hevc_encoder = None;
        self.state.stream_position = 0;
        self.state.bank = 0;
    }
    fn end_decode(&mut self) {
        self.decoder = None;
        self.state.decoded_pictures = 0;
    }

    /// Read `length` stream bytes at `position` (bytes since the configuration):
    /// the banks are a ping-pong pair, as for the encoder.
    fn read_stream(&self, cx: &mut Cx<'_>, position: u64, length: usize, size: u64) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::with_capacity(length);
        let mut at = position;
        while bytes.len() < length {
            let bank = if (at / size).is_multiple_of(2) { BADR_STR0 } else { BADR_STR1 };
            let base = self.high(3) | u64::from(self.word(bank));
            if base == 0 {
                return Err(format!("the stream continues in bank {} at {at:#x}, which has no address", (at / size) % 2));
            }
            let count = (length - bytes.len()).min((size - at % size) as usize);
            bytes.extend(cx.ram_bytes(base + at % size, count)?);
            at += count as u64;
        }
        Ok(bytes)
    }

    /// The Annex B access unit at the stream position: its NALs up to the next
    /// access unit (a parameter set, SEI, delimiter or slice after this unit's
    /// slice) or up to the end marker (a NAL of type 0).
    fn decode_unit(&self, cx: &mut Cx<'_>) -> Result<Vec<u8>, String> {
        let size = u64::from(self.word(STR_BUF_SIZE));
        if !size.is_power_of_two() || !(0x1000..=0x1000_0000).contains(&size) {
            return Err(format!("STR_BUF_SIZE {size:#x} is not a power of two in 4 KiB..256 MiB"));
        }
        let start = self.state.stream_position;
        let mut unit: Vec<u8> = Vec::new();
        let mut scanned = 0;
        let mut slice = false;
        // A unit is far smaller than a bank; the bound stops a stream without a next start code.
        while (unit.len() as u64) < size {
            let piece = (STREAM_PIECE as u64).min(size - unit.len() as u64) as usize;
            unit.extend(self.read_stream(cx, start + unit.len() as u64, piece, size)?);
            while scanned + 4 <= unit.len() {
                // 0x000000 and 0x000002 cannot occur inside a NAL unit (H.264 7.4.1), so
                // after the slice they end it: the last unit of a file is followed by
                // padding, not by a start code. A four-byte start code is left to the
                // three-byte match one byte on.
                if slice
                    && unit[scanned..scanned + 2] == [0, 0]
                    && matches!(unit[scanned + 2], 0 | 2)
                    && unit[scanned + 2..scanned + 4] != [0, 1]
                {
                    unit.truncate(scanned);
                    return Ok(unit);
                }
                if unit[scanned..scanned + 3] != [0, 0, 1] {
                    scanned += 1;
                    continue;
                }
                let kind = unit[scanned + 3] & 31;
                if kind == 0 || (slice && matches!(kind, 1 | 5..=9)) {
                    // Start codes have four bytes here.
                    let end = if scanned > 0 && unit[scanned - 1] == 0 { scanned - 1 } else { scanned };
                    unit.truncate(end);
                    return Ok(unit);
                }
                slice |= matches!(kind, 1 | 5);
                scanned += 3;
            }
        }
        Err(format!(
            "the stream at position {start:#x} has no {} in {size:#x} bytes",
            if slice { "start code after its slice" } else { "slice" }
        ))
    }

    /// Decode the access unit at the stream position and write the picture into
    /// the next image bank. Returns the value of `0x15ef0` and the status bits
    /// of the stream banks the unit finished.
    fn decode_picture(&mut self, cx: &mut Cx<'_>) -> Result<(u32, u32), String> {
        if self.word(STR_BUF_MODE) != 1 {
            return Err("a decode with STR_BUF_MODE 0 is not modelled".into());
        }
        let banks = self.word(IMG_BANKS) & 7;
        if !(1..=4).contains(&banks) {
            return Err(format!("{banks} image banks"));
        }
        let unit = self.decode_unit(cx)?;
        if self.trace {
            cx.log(&format!(
                "xp5-codec: decode unit of {:#x} bytes at stream position {:#x}, first bytes {:02x?}",
                unit.len(),
                self.state.stream_position,
                &unit[..unit.len().min(8)]
            ));
        }
        let opened = self.decoder.is_none();
        if opened {
            self.decoder = Some(Decoder::new_movie_bounded(&[], Bounds::UHD)?);
            self.state.decoded_pictures = 0;
        }
        let picture = self.decoder.as_mut().expect("decoder").decode_movie_unit(&unit)?;
        let (width, height) = (picture.sequence.width, picture.sequence.height);
        // Picture n goes to bank n of set 0, then of set 1 (`IMG_WIDX` bit 5).
        let n = self.state.decoded_pictures;
        let (set, bank) = ((n / banks) % 2, n % banks);
        let index = set << 5 | bank << 1;
        let y = self.state.images.get(&index).copied().ok_or("no luma address for the image bank")?;
        let c = self.state.images.get(&(index | 1)).copied().ok_or("no chroma address for the image bank")?;
        let pitch = self.word(IMG_LINE_OFS) as usize;
        if pitch < width || width % 2 != 0 || height % 2 != 0 {
            return Err(format!("IMG_LINE_OFS {pitch} for a {width}x{height} picture"));
        }
        let high = self.high(2);
        let (luma, chroma) = picture.i420.split_at(width * height);
        let (cb, cr) = chroma.split_at(width * height / 4);
        for row in 0..height {
            cx.ram_write((high | u64::from(y)) + (row * pitch) as u64, &luma[row * width..][..width])?;
        }
        let (first, second) = if CB_FIRST { (cb, cr) } else { (cr, cb) };
        let half = width / 2;
        let mut line = vec![0; width];
        for row in 0..height / 2 {
            for col in 0..half {
                line[col * 2] = first[row * half + col];
                line[col * 2 + 1] = second[row * half + col];
            }
            cx.ram_write((high | u64::from(c)) + (row * pitch) as u64, &line)?;
        }
        if opened {
            cx.log(&format!(
                "xp5-codec: H.264 decode {width}x{height} (coded {}x{}), {banks} image bank(s), Y {y:#x} C {c:#x} pitch {pitch:#x}",
                picture.sequence.coded_width, picture.sequence.coded_height
            ));
        }
        self.state.decoded_pictures = n.wrapping_add(1) % (2 * banks);
        // A unit that reaches the end of a stream bank finishes that bank.
        let size = u64::from(self.word(STR_BUF_SIZE));
        let (before, after) = (self.state.stream_position, self.state.stream_position + unit.len() as u64);
        let mut finished = 0;
        for crossed in before / size..after / size {
            finished |= 1 << (crossed % 2);
        }
        self.state.stream_position = after;
        self.state.bank = ((after / size) % 2) as u32;
        Ok((index, finished))
    }

    /// The halt request: the work in flight and the open recording end here.
    fn halt(&mut self, cx: &mut Cx<'_>) {
        if self.state.due.take().is_some() {
            cx.log("xp5-codec: halt drops the command in flight");
        }
        self.state.encode = false;
        self.state.stop = false;
        self.state.hevc = false;
        self.state.decode = false;
        self.end_decode();
        if self.recording() {
            cx.log(&format!("xp5-codec: halt ends the recording after {} pictures", self.state.pictures));
        }
        self.encoder = None;
        self.hevc_encoder = None;
        self.state.halted = true;
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
    /// Returns the bank-full status bits: bit `n` when the picture reached the
    /// end of stream bank `n`.
    fn encode(&mut self, cx: &mut Cx<'_>) -> Result<u32, String> {
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
        let nals: Vec<(u32, &[u8])> =
            nal_units(&unit)?.into_iter().map(|nal| (u32::from(nal[0] & 31), nal)).collect();
        self.publish(cx, &nals)
    }

    /// Code one HEVC picture (I_PCM, see [`hevc`]) and publish it.
    fn encode_hevc(&mut self, cx: &mut Cx<'_>) -> Result<u32, String> {
        if self.word(STR_BUF_MODE) != 0 {
            return Err("STR_BUF_MODE 1 (no NALU table) is not modelled".into());
        }
        let size = self.word(HEVC_SIZE);
        let (width, height) = ((size & 0x3fff) as usize, (size >> 16 & 0x1fff) as usize);
        let settings = PcmConfig {
            width,
            height,
            level_idc: PcmConfig::level_for(width, height),
            parameter_interval: HEVC_PARAMETER_INTERVAL,
        };
        if self.hevc_encoder.as_ref().is_some_and(|e| e.config() != settings) {
            cx.log("xp5-codec: HEVC parameters changed within a recording; new sequence");
            self.hevc_encoder = None;
        }
        if self.hevc_encoder.is_none() {
            let encoder = PcmEncoder::new(settings).map_err(|e| format!("HEVC size {size:#x}: {e}"))?;
            cx.log(&format!("xp5-codec: HEVC sequence {settings:?} (I_PCM)"));
            self.hevc_encoder = Some(encoder);
        }
        let picture = self.picture(cx, width, height)?;
        let unit = self.hevc_encoder.as_mut().expect("encoder").encode_i420(&picture)?;
        let nals: Vec<(u32, &[u8])> =
            hevc::nal_units(&unit).into_iter().map(|nal| (u32::from(nal[0] >> 1 & 63), nal)).collect();
        self.publish(cx, &nals)
    }

    /// Write typed NAL payloads with 4-byte start codes into the stream
    /// banks and the NALU table. Returns the bank-full status bits.
    fn publish(&mut self, cx: &mut Cx<'_>, nals: &[(u32, &[u8])]) -> Result<u32, String> {
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
        for (index, &(kind, nal)) in nals.iter().enumerate() {
            let start = self.state.stream_position;
            if matches!(kind, 7 | 8 | 32..=34) && start % size + nal.len() as u64 + 4 >= size {
                cx.log(&format!(
                    "xp5-codec: parameter set type {kind} wraps across stream banks; the driver does not join it"
                ));
            }
            self.write_stream(cx, &[0, 0, 0, 1], size)?;
            self.write_stream(cx, nal, size)?;
            let entry = [kind, nal.len() as u32 + 4, start as u32, 0];
            cx.ram_write(table + first + u64::from(ENTRY) * index as u64, &words(&entry))?;
        }
        let filled = std::mem::take(&mut self.filled);
        let end = first + u64::from(ENTRY) * nals.len() as u64;
        cx.ram_write(table + end, &words(&[NALU_TERMINATOR, 0, 0, 0]))?;
        self.state.nalu_bytes = (end as u32) & 0x7ff_ffff;
        self.state.pictures += 1;
        Ok(filled)
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
                self.filled |= 1 << self.state.bank;
                self.state.bank ^= 1;
            }
        }
        Ok(())
    }

    fn register_read(&self, register: u64) -> u32 {
        match register {
            H264_STATUS => self.state.pending,
            H264_RAW_STATUS => self.state.raw,
            HEVC_STATUS => self.state.hevc_pending,
            HEVC_RAW_STATUS => self.state.hevc_raw,
            HEVC_CMD => self.word(HEVC_CMD) & !HEVC_GO,
            PERI_STATUS | PERI_HALT => 0,
            PERI_RAW_STATUS => u32::from(self.state.halted) * PERI_HALTED,
            IMG_DECODED => self.state.decoded,
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
            HEVC_STATUS => {
                self.state.hevc_pending &= !value;
                self.state.hevc_raw &= !value;
                self.update_irq(cx);
            }
            // The ISR writes back what it read; nothing is ever pending here.
            PERI_STATUS | STR_BANK => {}
            PERI_HALT => {
                if value & 1 != 0 {
                    self.halt(cx);
                }
            }
            NALU_RESET => {
                if value & 1 != 0 {
                    self.state.nalu_bytes = 0;
                }
            }
            H264_INT_EN | HEVC_INT_EN => {
                self.set_word(register, value);
                self.update_irq(cx);
            }
            H264_CORESEL | HEVC_CORESEL | STR_BUF_SIZE => {
                // A core select is the first write of an open.
                if register != STR_BUF_SIZE {
                    self.state.halted = false;
                    self.end_decode();
                }
                self.set_word(register, value);
                self.restart_stream();
            }
            IMG_WADR => {
                let index = self.word(IMG_WIDX) & 0xff;
                self.state.images.insert(index, value);
                self.state.hevc_image = true;
                self.set_word(register, value);
            }
            HEVC_CMD => {
                if value & HEVC_GO != 0 {
                    self.state.halted = false;
                    if self.state.due.is_some() && !self.state.hevc {
                        return cx.fail("xp5-codec: HEVC command while an H.264 command is in flight");
                    }
                    if std::mem::take(&mut self.state.hevc_image) {
                        if self.state.encode {
                            return cx.fail("xp5-codec: HEVC start while a picture is in flight");
                        }
                        self.state.encode = true;
                    } else {
                        // The stop (0x13d1b0c) does not wait for the picture
                        // in flight; it completes after that picture.
                        self.state.stop = true;
                    }
                    self.state.hevc = true;
                    if self.state.due.is_none() {
                        match cx.now().checked_add(self.ticks) {
                            Some(at) => self.state.due = Some(at),
                            None => return cx.fail("xp5-codec deadline overflow"),
                        }
                    }
                }
                self.set_word(register, value & !HEVC_GO);
            }
            H264_CMD => {
                if value & (CMD_START | CMD_STOP) != 0 {
                    self.state.halted = false;
                    if value & CMD_START != 0 {
                        if self.state.encode {
                            return cx.fail("xp5-codec: H.264 start while a picture is in flight");
                        }
                        self.state.encode = true;
                    }
                    if value & CMD_STOP != 0 {
                        if self.word(H264_CORESEL) == CORESEL_DECODE {
                            if self.state.decode {
                                return cx.fail("xp5-codec: H.264 decode start while a picture is in flight");
                            }
                            self.state.decode = true;
                        } else {
                            self.state.stop = true;
                        }
                    }
                    if self.state.due.is_none() {
                        match cx.now().checked_add(self.ticks) {
                            Some(at) => self.state.due = Some(at),
                            None => return cx.fail("xp5-codec deadline overflow"),
                        }
                    }
                }
                if value & CMD_CLOSE != 0 {
                    self.end_decode();
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
            || (state.pending | state.raw | state.hevc_pending | state.hevc_raw) & !STATUS_BITS != 0
            || (state.encode || state.stop || state.decode) != state.due.is_some()
            || state.decoded & !0x27 != 0
            || (state.hevc && state.due.is_none())
        {
            return Err("invalid XP5 codec checkpoint".into());
        }
        Ok(())
    }
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}
fn is_false(value: &bool) -> bool {
    !*value
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
        if self.trace {
            cx.log(&format!("xp5-codec: write {address:#x} = {value:#x}"));
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
        let hevc = std::mem::take(&mut self.state.hevc);
        if std::mem::take(&mut self.state.encode) {
            let coded = if hevc { self.encode_hevc(cx) } else { self.encode(cx) };
            let filled = match coded {
                Ok(filled) => filled,
                Err(error) => {
                    let codec = if hevc { "HEVC" } else { "H.264" };
                    return cx.fail(format!("xp5-codec: {codec} encode failed: {error}"));
                }
            };
            if hevc {
                self.state.hevc_pending |= NALU_READY | filled;
                self.state.hevc_raw |= NALU_READY | filled;
            } else {
                self.state.pending |= NALU_READY | filled;
                self.state.raw |= NALU_READY | filled;
            }
        }
        if std::mem::take(&mut self.state.decode) {
            let finished = match self.decode_picture(cx) {
                Ok((index, finished)) => {
                    self.state.decoded = index;
                    finished
                }
                Err(error) => return cx.fail(format!("xp5-codec: H.264 decode failed: {error}")),
            };
            self.state.pending |= PICTURE_DECODED | finished;
            self.state.raw |= PICTURE_DECODED | finished;
        }
        if std::mem::take(&mut self.state.stop) {
            let open = if hevc { self.hevc_encoder.take().is_some() } else { self.encoder.take().is_some() };
            if open {
                cx.log(&format!("xp5-codec: recording stopped after {} pictures", self.state.pictures));
            }
            if hevc {
                self.state.hevc_pending |= STOP_DONE;
                self.state.hevc_raw |= STOP_DONE;
            } else {
                self.state.pending |= STOP_DONE;
                self.state.raw |= STOP_DONE;
            }
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
            map.insert("recording".into(), Value::Bool(self.recording()));
            map.insert("decoding".into(), Value::Bool(self.decoder.is_some()));
        }
        state
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.check(&state)?;
        self.state = state;
        self.encoder = None;
        self.hevc_encoder = None;
        self.decoder = None;
        Ok(())
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        if self.recording() {
            return Err(RECORDING.into());
        }
        if self.decoder.is_some() {
            return Err(DECODING.into());
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
        self.hevc_encoder = None;
        self.decoder = None;
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.checkpoint()?.state())
    }
}

/// The `XT5_SCENE` scene at the programmed size, unless `XT5_MOVIE_PATH` is
/// `real` ([`fujimod_emulator_imaging::frame::movie_path_real`]).
pub fn host_movie_422(width: usize, height: usize) -> Option<Vec<u8>> {
    if fujimod_emulator_imaging::frame::movie_path_real() {
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
        // Bits 1..0 name the bank this picture filled, if any.
        let filled = if c.state.bank != bank { 1 << bank } else { 0 };
        assert_eq!((status, raw), (NALU_READY | filled, NALU_READY | filled));
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

    /// The HEVC core: frame start (image addresses, then HEVC_CMD bit 0),
    /// status on 0x14120/0x14124, VPS/SPS/PPS then IDR_W_RADL entries, and a
    /// stop (HEVC_CMD bit 0 without new image addresses).
    #[test]
    fn hevc_pictures_and_stop_follow_the_driver_contract() {
        let mut c = Codec::new(IRQ, 10).unwrap();
        let mut h = host();
        open_and_configure(&mut c, &mut h);
        reg(&mut c, &mut h, HEVC_CORESEL, 1);
        reg(&mut c, &mut h, HEVC_INT_EN, 0x010c_0000);
        reg(&mut c, &mut h, HEVC_SIZE, (H as u32) << 16 | W as u32);
        let mut expected = PcmEncoder::new(PcmConfig {
            width: W,
            height: H,
            level_idc: PcmConfig::level_for(W, H),
            parameter_interval: HEVC_PARAMETER_INTERVAL,
        })
        .unwrap();
        for n in 0..2 {
            frame(&mut h, n);
            reg(&mut c, &mut h, IMG_WIDX, 0);
            reg(&mut c, &mut h, IMG_WADR, Y_AT as u32);
            reg(&mut c, &mut h, IMG_WIDX, 1);
            reg(&mut c, &mut h, IMG_WADR, C_AT as u32);
            reg(&mut c, &mut h, HEVC_CMD, HEVC_GO);
            assert_eq!(get(&mut c, &mut h, HEVC_CMD), 0);
            h.now = c.next_deadline(h.now).unwrap();
            let mut cx = Cx::new(&mut h, 0, 0, 0);
            c.advance(&mut cx);
            assert_eq!(cx.refusal(), None);
            assert_eq!(h.irqs.last(), Some(&(IRQ, true)));
            let raw = get(&mut c, &mut h, HEVC_RAW_STATUS);
            assert_eq!(raw & NALU_READY, NALU_READY);
            assert_eq!(get(&mut c, &mut h, H264_STATUS), 0);
            reg(&mut c, &mut h, HEVC_STATUS, raw);
            assert_eq!(h.irqs.last(), Some(&(IRQ, false)));
            let end = get(&mut c, &mut h, NALU_END) as usize;
            let table = h.bytes(NALU_AT, end + 16).unwrap();
            let word = |i: usize| u32::from_le_bytes(table[i * 4..i * 4 + 4].try_into().unwrap());
            let entries: Vec<[u32; 3]> =
                (0..end / 16).map(|e| [word(e * 4), word(e * 4 + 1), word(e * 4 + 2)]).collect();
            assert_eq!(word(end / 4), NALU_TERMINATOR);
            reg(&mut c, &mut h, NALU_RESET, 1);
            let types: Vec<u32> = entries.iter().map(|e| e[0]).collect();
            assert_eq!(types, if n == 0 { vec![32, 33, 34, 19] } else { vec![19] });
            let joined: Vec<u8> = entries.iter().flat_map(|e| nal_bytes(&h, *e)).collect();
            let i420 = semi_planar_to_i420(
                &h.bytes(Y_AT, PITCH * H).unwrap(),
                &h.bytes(C_AT, PITCH * H / 2).unwrap(),
                PITCH,
                W,
                H,
            );
            assert_eq!(joined, expected.encode_i420(&i420).unwrap());
        }
        assert_eq!(c.checkpoint().unwrap_err(), RECORDING);
        reg(&mut c, &mut h, HEVC_CMD, HEVC_GO);
        h.now = c.next_deadline(h.now).unwrap();
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        c.advance(&mut cx);
        assert_eq!(cx.refusal(), None);
        assert_eq!(get(&mut c, &mut h, HEVC_STATUS), STOP_DONE);
        reg(&mut c, &mut h, HEVC_STATUS, STOP_DONE);
        assert_eq!(h.irqs.last(), Some(&(IRQ, false)));
        let saved = c.checkpoint().unwrap();
        let mut restored = Codec::new(IRQ, 10).unwrap();
        restored.restore_checkpoint(&saved).unwrap();
        assert_eq!(restored.checkpoint().unwrap().state(), saved.state());
    }

    /// A stop written while the last HEVC picture is in flight completes
    /// after that picture: NALU ready and stop done in one completion.
    #[test]
    fn an_hevc_stop_during_a_picture_waits_for_it() {
        let mut c = Codec::new(IRQ, 10).unwrap();
        let mut h = host();
        open_and_configure(&mut c, &mut h);
        reg(&mut c, &mut h, HEVC_INT_EN, 0x010c_0000);
        reg(&mut c, &mut h, HEVC_SIZE, (H as u32) << 16 | W as u32);
        frame(&mut h, 0);
        reg(&mut c, &mut h, IMG_WIDX, 0);
        reg(&mut c, &mut h, IMG_WADR, Y_AT as u32);
        reg(&mut c, &mut h, IMG_WIDX, 1);
        reg(&mut c, &mut h, IMG_WADR, C_AT as u32);
        reg(&mut c, &mut h, HEVC_CMD, HEVC_GO);
        reg(&mut c, &mut h, HEVC_CMD, HEVC_GO);
        h.now = c.next_deadline(h.now).unwrap();
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        c.advance(&mut cx);
        assert_eq!(cx.refusal(), None);
        let raw = get(&mut c, &mut h, HEVC_RAW_STATUS);
        assert_eq!(raw & (NALU_READY | STOP_DONE), NALU_READY | STOP_DONE);
        assert!(c.checkpoint().is_ok(), "the recording is closed");
    }

    const DEC_STR_AT: u64 = RAM + 0x38_0000;
    const DEC_Y: [u64; 2] = [RAM + 0x8000, RAM + 0xc000];
    const DEC_C: [u64; 2] = [RAM + 0xa000, RAM + 0xe000];

    /// The access units of `count` pictures coded by the encode path (GOP 2), each
    /// as Annex B bytes with 4-byte start codes.
    fn coded_units(count: usize) -> Vec<Vec<u8>> {
        let mut c = Codec::new(IRQ, 1000).unwrap();
        let mut h = host();
        open_and_configure(&mut c, &mut h);
        (0..count)
            .map(|n| {
                frame(&mut h, n);
                encode_one(&mut c, &mut h).iter().flat_map(|entry| nal_bytes(&h, *entry)).collect()
            })
            .collect()
    }

    /// The playback open and configure (codec 0, decode) for `banks` image banks.
    fn open_decode(c: &mut Codec, h: &mut RecordingHost, banks: u32) {
        reg(c, h, 0x1_4088, 1);
        reg(c, h, 0x1_4080, 0x7_0001);
        reg(c, h, 0x1_4008, 0x8000_0006);
        reg(c, h, H264_CORESEL, CORESEL_DECODE);
        reg(c, h, H264_INT_EN, 0x2803);
        reg(c, h, 0x1_5fd8, 0x7_7770);
        for address in [0xff90_e710, 0xff90_e714, 0xff90_e718, 0xff90_e71c] {
            write(c, h, address, 0);
        }
        reg(c, h, 0x1_5ef4, 1);
        reg(c, h, IMG_BANKS, banks);
        for set in 0..2 {
            for bank in 0..banks {
                reg(c, h, IMG_WIDX, (set << 5) | (bank * 2));
                reg(c, h, IMG_WADR, DEC_Y[bank as usize] as u32);
                reg(c, h, IMG_WIDX, (set << 5) | (bank * 2 + 1));
                reg(c, h, IMG_WADR, DEC_C[bank as usize] as u32);
            }
        }
        reg(c, h, IMG_LINE_OFS, PITCH as u32);
        reg(c, h, 0x1_5e00, 0x9100_0000);
        reg(c, h, 0x1_5e04, 0x9100_0000);
        reg(c, h, 0x1_5f14, 0);
        reg(c, h, BADR_STR0, DEC_STR_AT as u32);
        reg(c, h, BADR_STR1, 0);
        reg(c, h, STR_BUF_SIZE, 0x1_0000);
        reg(c, h, STR_BUF_MODE, 1);
        reg(c, h, 0x1_5ed4, 0);
        reg(c, h, 0x1_5ed8, 0);
    }

    /// A configure (0x13d0dbc, which starts the stream at bank 0 again) and
    /// DecStart: the unit and its end marker in the stream, H264_CMD bit 14, the
    /// deadline. Returns the refusal, if any.
    fn decode_start(c: &mut Codec, h: &mut RecordingHost, unit: &[u8], marker: bool) -> Option<String> {
        reg(c, h, STR_BUF_SIZE, 0x1_0000);
        let at = (DEC_STR_AT - RAM) as usize;
        h.ram[at..at + 0x1_0000].fill(0xff);
        h.ram[at..at + unit.len()].copy_from_slice(unit);
        if marker {
            h.ram[at + unit.len()..at + unit.len() + 8].copy_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0]);
        }
        reg(c, h, H264_CMD, CMD_STOP);
        assert_eq!(get(c, h, H264_STATUS), 0);
        h.now = c.next_deadline(h.now).expect("a decode deadline");
        let mut cx = Cx::new(h, 0, 0, 0);
        c.advance(&mut cx);
        cx.refusal().map(str::to_owned)
    }

    /// The picture in image bank `bank` as tight I420.
    fn bank_i420(h: &RecordingHost, bank: usize) -> Vec<u8> {
        let luma = h.bytes(DEC_Y[bank], PITCH * H).unwrap();
        let chroma = h.bytes(DEC_C[bank], PITCH * H / 2).unwrap();
        semi_planar_to_i420(&luma, &chroma, PITCH, W, H)
    }

    /// Movie playback decodes the pictures the encode path wrote: each DecStart
    /// decodes the unit in the stream bank up to its end marker, writes the
    /// picture into the image bank as two-plane 4:2:0 at the programmed pitch and
    /// completes with status bit 11, the interrupt and the bank in 0x15ef0. The
    /// bytes are the workspace decoder's; a P picture needs the session's
    /// reference picture, so the session refuses a checkpoint until the close.
    #[test]
    fn decode_writes_the_coded_pictures_into_the_image_bank() {
        let units = coded_units(3);
        let mut reference = Decoder::new_movie(&[]).unwrap();
        let mut c = Codec::new(IRQ, 1000).unwrap();
        let mut h = host();
        open_decode(&mut c, &mut h, 1);
        c.checkpoint().expect("an open without a decoded picture is idle");
        for (n, unit) in units.iter().enumerate() {
            let expected = reference.decode_movie_unit(unit).unwrap();
            assert_eq!(decode_start(&mut c, &mut h, unit, true), None);
            assert_eq!(h.irqs.last(), Some(&(IRQ, true)));
            // ISR 0x13d1da0 and task 0x13d2620.
            assert_eq!(get(&mut c, &mut h, 0x1_5fd0), 0);
            assert_eq!(get(&mut c, &mut h, H264_STATUS), PICTURE_DECODED);
            assert_eq!(get(&mut c, &mut h, H264_RAW_STATUS), PICTURE_DECODED);
            reg(&mut c, &mut h, H264_STATUS, PICTURE_DECODED);
            assert_eq!(h.irqs.last(), Some(&(IRQ, false)));
            // One bank: set 0, then set 1, which has the same addresses.
            assert_eq!(get(&mut c, &mut h, IMG_DECODED), (n as u32 % 2) << 5);
            assert_eq!(bank_i420(&h, 0), expected.i420, "picture {n}");
            // The picture is the coded one: the gradient of `frame` and its chroma.
            let luma = h.bytes(DEC_Y[0], PITCH * H).unwrap();
            let original = |x: usize, row: usize| (16 + (x * 3 + row + n * 4) % 200) as u8;
            let error: u32 = (0..H)
                .flat_map(|row| (0..W).map(move |x| (x, row)))
                .map(|(x, row)| u32::from(luma[row * PITCH + x].abs_diff(original(x, row))))
                .sum();
            assert!(error / ((W * H) as u32) < 8, "mean luma error {}", error / ((W * H) as u32));
            let chroma = h.bytes(DEC_C[0], PITCH * H / 2).unwrap();
            assert!(chroma[20].abs_diff(90) < 12 && chroma[21].abs_diff(170) < 12);
            // Bytes past the picture width are untouched.
            assert_eq!(luma[W..PITCH], vec![0; PITCH - W]);
            assert_eq!(c.checkpoint().unwrap_err(), DECODING);
        }
        // Close (H265Stop 0x13d15f0): no interrupt, the session ends.
        reg(&mut c, &mut h, H264_CMD, CMD_CLOSE);
        assert_eq!(c.next_deadline(h.now), None);
        let saved = c.checkpoint().unwrap();
        let mut restored = Codec::new(IRQ, 1000).unwrap();
        restored.restore_checkpoint(&saved).unwrap();
        assert_eq!(restored.checkpoint().unwrap().state(), saved.state());
        // A new session starts with an IDR again; a P picture alone has no reference.
        assert!(decode_start(&mut c, &mut h, &units[1], true).unwrap().contains("H.264 decode failed"));
    }

    /// The play session: the units follow each other in the stream, across the
    /// end of bank 0 into bank 1, and each DecStart decodes the unit at the stream
    /// position without a new configuration. With two image banks picture n goes
    /// to bank n modulo 2 of set 0, then of set 1, and 0x15ef0 names set and bank.
    /// The unit that reaches the end of stream bank 0 reports that bank (bit 0)
    /// next to the picture.
    #[test]
    fn decode_follows_the_stream_across_units_and_banks() {
        let units = coded_units(16);
        let stream: Vec<u8> = units.iter().flatten().copied().chain([0, 0, 0, 1, 0, 0, 0, 0]).collect();
        let bank = 0x1000usize;
        assert!(units[0].len() < bank && stream.len() > bank && stream.len() < 2 * bank, "{}", stream.len());
        let mut c = Codec::new(IRQ, 1000).unwrap();
        let mut h = host();
        open_decode(&mut c, &mut h, 2);
        let second = DEC_STR_AT + 0x4_0000;
        reg(&mut c, &mut h, BADR_STR1, second as u32);
        reg(&mut c, &mut h, STR_BUF_SIZE, bank as u32);
        let at = (DEC_STR_AT - RAM) as usize;
        h.ram[at..at + bank].copy_from_slice(&stream[..bank]);
        let at = (second - RAM) as usize;
        h.ram[at..at + stream.len() - bank].copy_from_slice(&stream[bank..]);
        let mut reference = Decoder::new_movie(&[]).unwrap();
        let mut position = 0;
        let mut reported = 0;
        for (n, unit) in units.iter().enumerate() {
            let expected = reference.decode_movie_unit(unit).unwrap();
            reg(&mut c, &mut h, H264_CMD, CMD_STOP);
            h.now = c.next_deadline(h.now).expect("a decode deadline");
            let mut cx = Cx::new(&mut h, 0, 0, 0);
            c.advance(&mut cx);
            assert_eq!(cx.refusal(), None, "picture {n}");
            let crossed = position / bank != (position + unit.len()) / bank;
            position += unit.len();
            let status = get(&mut c, &mut h, H264_STATUS);
            assert_eq!(status, PICTURE_DECODED | u32::from(crossed), "picture {n}");
            assert_eq!(get(&mut c, &mut h, H264_RAW_STATUS), status);
            reported += u32::from(crossed);
            reg(&mut c, &mut h, H264_STATUS, status);
            assert_eq!(h.irqs.last(), Some(&(IRQ, false)));
            let n = n as u32;
            assert_eq!(get(&mut c, &mut h, IMG_DECODED), ((n / 2) % 2) << 5 | (n % 2) << 1);
            assert_eq!(bank_i420(&h, n as usize % 2), expected.i420, "picture {n}");
            assert_eq!(get(&mut c, &mut h, STR_BANK) >> 12, u32::from(position >= bank));
        }
        assert_eq!(reported, 1);
        // The stream ends at the marker: a further DecStart finds no slice.
        reg(&mut c, &mut h, H264_CMD, CMD_STOP);
        h.now = c.next_deadline(h.now).unwrap();
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        c.advance(&mut cx);
        assert!(cx.refusal().unwrap().contains("H.264 decode failed"));
    }

    /// The last unit of a file has no start code after it, only padding: zero
    /// bytes (and 0x000002) cannot occur inside a NAL unit, so they end it.
    #[test]
    fn padding_after_the_last_unit_ends_it() {
        let units = coded_units(2);
        let mut reference = Decoder::new_movie(&[]).unwrap();
        let mut c = Codec::new(IRQ, 1000).unwrap();
        let mut h = host();
        open_decode(&mut c, &mut h, 1);
        for (n, (unit, pad)) in units.iter().zip([[0u8, 0, 0, 0], [0, 0, 2, 0xff]]).enumerate() {
            let expected = reference.decode_movie_unit(unit).unwrap();
            reg(&mut c, &mut h, STR_BUF_SIZE, 0x1_0000);
            let at = (DEC_STR_AT - RAM) as usize;
            h.ram[at..at + 0x1_0000].fill(0xff);
            h.ram[at..at + unit.len()].copy_from_slice(unit);
            h.ram[at + unit.len()..at + unit.len() + 4].copy_from_slice(&pad);
            reg(&mut c, &mut h, H264_CMD, CMD_STOP);
            h.now = c.next_deadline(h.now).expect("a decode deadline");
            let mut cx = Cx::new(&mut h, 0, 0, 0);
            c.advance(&mut cx);
            assert_eq!(cx.refusal(), None, "unit {n}");
            reg(&mut c, &mut h, H264_STATUS, PICTURE_DECODED);
            assert_eq!(bank_i420(&h, 0), expected.i420, "picture {n}");
        }
    }

    /// A stream without the end marker, a stream that is not H.264, and a decode
    /// in NALU-table mode stop the run instead of completing.
    #[test]
    fn a_stream_the_decoder_cannot_follow_stops_the_run() {
        let units = coded_units(1);
        let mut c = Codec::new(IRQ, 1000).unwrap();
        let mut h = host();
        open_decode(&mut c, &mut h, 1);
        assert!(decode_start(&mut c, &mut h, &units[0], false).unwrap().contains("no start code after its slice"));
        open_decode(&mut c, &mut h, 1);
        let garbage = [0, 0, 0, 1, 0x65, 0x88, 0x84, 0x00, 0x12, 0x34];
        assert!(decode_start(&mut c, &mut h, &garbage, true).unwrap().contains("H.264 decode failed"));
        open_decode(&mut c, &mut h, 1);
        reg(&mut c, &mut h, STR_BUF_MODE, 0);
        assert!(decode_start(&mut c, &mut h, &units[0], true).unwrap().contains("STR_BUF_MODE"));
        // In encode mode bit 14 is still the stop.
        let mut c = Codec::new(IRQ, 1000).unwrap();
        open_and_configure(&mut c, &mut h);
        reg(&mut c, &mut h, H264_CMD, CMD_STOP);
        h.now = c.next_deadline(h.now).unwrap();
        c.advance(&mut Cx::new(&mut h, 0, 0, 0));
        assert_eq!(get(&mut c, &mut h, H264_STATUS), STOP_DONE);
    }

    /// The terminate 0x13d0e60 after a recording: the halt request (0x15ff4 = 1) is
    /// acknowledged in the raw peripheral status (0x15fd4 bit 0) at once, with no
    /// interrupt and nothing in the pending word 0x15fd0; a start in flight is
    /// dropped and the recording ends, so a checkpoint is possible again. The
    /// next open (core select) clears the acknowledge.
    #[test]
    fn a_halt_is_acknowledged_and_ends_the_recording() {
        let mut c = Codec::new(IRQ, 1000).unwrap();
        let mut h = host();
        open_and_configure(&mut c, &mut h);
        assert_eq!(get(&mut c, &mut h, PERI_RAW_STATUS), 0);
        frame(&mut h, 0);
        encode_one(&mut c, &mut h);
        assert!(c.checkpoint().is_err());
        reg(&mut c, &mut h, IMG_WIDX, 0);
        reg(&mut c, &mut h, IMG_WADR, Y_AT as u32);
        reg(&mut c, &mut h, H264_CMD, CMD_START);
        assert!(c.next_deadline(h.now).is_some());
        let irqs = h.irqs.len();
        reg(&mut c, &mut h, PERI_HALT, 1);
        assert_eq!(get(&mut c, &mut h, PERI_RAW_STATUS) & 1, 1);
        assert_eq!(get(&mut c, &mut h, PERI_STATUS), 0);
        assert_eq!(get(&mut c, &mut h, PERI_HALT), 0);
        assert_eq!(c.next_deadline(h.now), None);
        assert_eq!(h.irqs.len(), irqs);
        let saved = c.checkpoint().expect("no recording after the halt");
        let mut restored = Codec::new(IRQ, 1000).unwrap();
        restored.restore_checkpoint(&saved).unwrap();
        assert_eq!(get(&mut restored, &mut h, PERI_RAW_STATUS) & 1, 1);
        reg(&mut c, &mut h, H264_CORESEL, 1);
        assert_eq!(get(&mut c, &mut h, PERI_RAW_STATUS), 0);
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
