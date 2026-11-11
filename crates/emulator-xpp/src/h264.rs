//! X-Processor Pro movie H.264 encoder at 0xfff5c000.
//!
//! X-T20 2.11 drives this block from the UniH264 wrappers: three 8-byte-aligned
//! input pointers at +0x100/+0x104/+0x108, width and height at +0x018,
//! three 8-byte-aligned output pointers at +0x01c/+0x028/+0x02c, start/notify
//! at +0x200, ready at +0x204 and the movie format word at +0x208. The driver
//! registers source 0x31 (0x1565430); row 0x31 of the 0xed4720 source table is
//! GIC line 0x6e, so the runtime interrupt is 110.
//!
//! Input: 0x184c9d0 stores context 0x1bc65b4 +0x30/+0x38/+0x34, and 0x16b7318
//! maps them to +0x100/+0x104/+0x108. The main frame at +0x100 is read as
//! packed 4:2:2 (Y0 Y1 Cb Cr), converted to planar 4:2:0 and encoded by the
//! software backend. The other two pointers are opaque references.
//!
//! Output: 0x184c8c4 copies context 0x1bc65b4 +0x04/+0x18/+0x1c to config
//! +0x94/+0x98/+0x9c; 0x16b74d8 copies those to frame record +0x4c/+0x50/+0x54
//! and 0x1566200 writes them to +0x01c/+0x028/+0x02c. The only writer of that
//! context is 0x184c9d0, which clears 0x44 bytes before storing the input
//! pointers. Nothing in the image writes +0x04/+0x18/+0x1c, so a stock
//! recording presents 0/0/0. A zero descriptor is therefore accepted, not
//! refused; where the stock bitstream actually lands is not proven. A non-zero
//! descriptor keeps the model's declared reading of +0x01c as the output base
//! and +0x028 as its end; +0x02c is stored but neither its role nor a
//! base/end/next ring law is runtime-witnessed.
//!
//! Framing is firmware-proven Annex B: the parser at 0x1553bc0..0x1554234 scans
//! 00 00 00 01 start codes, accepts NAL types 1..8 and copies SPS/PPS to
//! 0x1a4fbc8/0x1a4fbac. The MOV writer converts each start code to a 4-byte
//! big-endian NAL length at 0x167c50c..0x167c5ac. The first frame of a sequence
//! publishes SPS/PPS and an IDR; later frames publish one coded picture with
//! the same parameter sets. Stock clips carry 720 samples in 12 chunks of 60
//! and key every 60 samples, so the declared GOP is 60.
//!
//! The adjacent 0xfff5b000 page is a separate register block (0xca1970 and
//! 0xca1b98 write +0x00/+0x04/+0x0c/+0x10); this driver never references it,
//! so it is not a clock or gate for this block.
//!
//! Real movie bitstream handoff: this block is not the producer the MOV writer
//! consumes. The writer's stream descriptor is global 0x1c26dfc
//! {data pointer, length, state, flag}. It is filled by the codec callback
//! 0x167aaf4, registered at 0x1bdd024 by 0x1554e30 (called from 0x167bff8).
//! The codec library 0x1555xxx runs the 0xfffcc4xx window, reports the
//! bitstream address at 0xfffcc4ac and sizes at 0xfffccd88/0xfffccd8c, and
//! raises source 0x3e (GIC line 137). The writer then appends the NALs at
//! 0x167c354 and writes a 4-byte big-endian length at 0x167c50c..0x167c5ac.
//! The model at 0xfff5c000 therefore keeps its payload internal and never
//! writes 0x1c26dfc; faking that handoff would bypass the real codec block.
//!
//! Not modeled, and refused: any format other than the movie library's
//! 0x04000400 word, non-macroblock-aligned pictures, a partial or oversized
//! output span, and input/output overlap.

use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_image_codecs::h264::{self, Codec, Config};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Register window size in bytes.
pub const WINDOW: u64 = 0x1000;
const RESET: u64 = 0x000;
const INIT: u64 = 0x004;
const FORMAT: u64 = 0x008;
const PHASE: u64 = 0x010;
const SIZE: u64 = 0x018;
const OUTPUT: u64 = 0x01C;
const OUTPUT_END: u64 = 0x028;
const OUTPUT_NEXT: u64 = 0x02C;
const CONTROL: u64 = 0x200;
const STATUS: u64 = 0x204;
const MODE: u64 = 0x208;
const CLOCK: u64 = 0x304;
const NOTIFY: u64 = 0x308;
const INPUT_MAIN: u64 = 0x100;
const INPUT_NEXT: u64 = 0x104;
const INPUT_PREVIOUS: u64 = 0x108;
const READY: u32 = 1 << 16;
const IRQ_ENABLE: u32 = 1 << 16;
const START: u32 = 1;

/// The movie library's only observed format word (+0x208).
const MOVIE_FORMAT: u32 = 0x0400_0400;
/// Declared host codec profile. No register in the observed law carries these.
const FRAMES_PER_SECOND: u32 = 60;
const BITRATE: u32 = 40_000_000;
const MAX_QP: u8 = 26;
/// Stock clips key every 60 samples; the sequence starts with an IDR.
const GOP: usize = 60;
/// Upper bound on one access unit in the model's output span.
const MAX_OUTPUT: u64 = 16 * 1024 * 1024;
/// Exact checkpoint replay needs every planar input since the sequence start.
/// Refuse a checkpoint rather than restore an inexact rate-control state once
/// this bound is reached.
const MAX_REPLAY_BYTES: usize = 256 * 1024 * 1024;

struct Pending {
    deadline: u64,
    output: Option<u64>,
    payload: Arc<[u8]>,
    parameters: Arc<[u8]>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingCheckpoint {
    deadline: u64,
    output: Option<u64>,
    payload_len: usize,
    parameters_len: usize,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    pending: Option<PendingCheckpoint>,
    config: Option<Config>,
    parameter_sets: Vec<u8>,
    last_frame_len: usize,
    frames: u64,
    output_bytes: u64,
    irq_level: bool,
    history_lengths: Vec<usize>,
    history_truncated: bool,
}
pub struct Encoder {
    base: u64,
    irq: u32,
    ticks: u64,
    words: BTreeMap<u64, u32>,
    pending: Option<Pending>,
    codec: Option<Codec>,
    config: Option<Config>,
    parameter_sets: Vec<u8>,
    last_frame: Arc<[u8]>,
    frames: u64,
    output_bytes: u64,
    irq_level: bool,
    /// Planar inputs since the sequence start. Replaying them rebuilds the
    /// exact codec state, including rate-control history, after a restore.
    history: Vec<Arc<[u8]>>,
    history_bytes: usize,
    history_truncated: bool,
}
impl Encoder {
    pub fn new(base: u64, irq: u32, ticks: u64) -> Result<Self, String> {
        if base & 0xfff != 0 || !(32..512).contains(&irq) || ticks == 0 {
            return Err("Invalid X-Processor Pro H.264 encoder".into());
        }
        Ok(Self {
            base,
            irq,
            ticks,
            words: BTreeMap::new(),
            pending: None,
            codec: None,
            config: None,
            parameter_sets: Vec::new(),
            last_frame: Vec::new().into(),
            frames: 0,
            output_bytes: 0,
            irq_level: false,
            history: Vec::new(),
            history_bytes: 0,
            history_truncated: false,
        })
    }
    fn word(&self, offset: u64) -> u32 {
        self.words.get(&offset).copied().unwrap_or(0)
    }
    fn set(&mut self, offset: u64, value: u32) {
        if value == 0 {
            self.words.remove(&offset);
        } else {
            self.words.insert(offset, value);
        }
    }
    fn ready(&self) -> bool {
        self.pending.is_none()
    }
    fn clear_sequence(&mut self) {
        self.codec = None;
        self.config = None;
        self.parameter_sets.clear();
        self.history.clear();
        self.history_bytes = 0;
        self.history_truncated = false;
    }
    fn valid(offset: u64) -> bool {
        matches!(
            offset,
            0x000
                | 0x004
                | 0x008
                | 0x010
                | 0x014
                | 0x018
                | 0x01C
                | 0x020
                | 0x024
                | 0x028
                | 0x02C
                | 0x030
                | 0x034
                | 0x038
                | 0x03C
                | 0x040
                | 0x044
                | 0x048
                | 0x04C
                | 0x050
                | 0x054
                | 0x058
                | 0x05C
                | 0x060
                | 0x064
                | 0x068
                | 0x06C
                | 0x070
                | 0x074
                | 0x078
                | 0x07C
                | 0x080
                | 0x084
                | 0x088
                | 0x08C
                | 0x100
                | 0x104
                | 0x108
                | 0x110
                | 0x114
                | 0x118
                | 0x11C
                | 0x200
                | 0x204
                | 0x208
                | 0x304
                | 0x308
        )
    }
    fn set_line(&mut self, cx: &mut Cx<'_>, high: bool) {
        if high != self.irq_level {
            self.irq_level = high;
            cx.set_irq(self.irq, high);
        }
    }
    fn reset(&mut self, cx: &mut Cx<'_>) {
        self.words.clear();
        self.pending = None;
        self.clear_sequence();
        self.last_frame = Vec::new().into();
        self.set_line(cx, false);
    }
    fn start(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if self.pending.is_some() {
            return Err("H.264 start while a frame is busy".into());
        }
        let format = self.word(FORMAT);
        if !(1..=4).contains(&(format & 7)) {
            return Err(format!("H.264 unsupported mode fields {format:#x}"));
        }
        let mode = self.word(MODE);
        if mode != MOVIE_FORMAT {
            return Err(format!("H.264 unsupported format {mode:#x}"));
        }
        let size = self.word(SIZE);
        if size & 0xc000_c000 != 0 {
            return Err("H.264 picture size exceeds its 14-bit fields".into());
        }
        let config = Config {
            width: ((size >> 16) & 0x3fff) as usize,
            height: (size & 0x3fff) as usize,
            frames_per_second: FRAMES_PER_SECOND,
            bitrate: BITRATE,
            max_qp: MAX_QP,
            gop: GOP,
        };
        config.validate()?;
        if self.config.is_some_and(|old| old != config) {
            return Err("H.264 sequence configuration changed without initialization".into());
        }
        let main = u64::from(self.word(INPUT_MAIN));
        if main == 0 {
            return Err("H.264 main frame pointer is zero".into());
        }
        let out = u64::from(self.word(OUTPUT));
        let end = u64::from(self.word(OUTPUT_END));
        let next = u64::from(self.word(OUTPUT_NEXT));
        let output = match (out, end, next) {
            (0, 0, 0) => None,
            (0, _, _) | (_, 0, _) => {
                return Err("H.264 output span is incomplete".into());
            }
            _ => {
                if end <= out {
                    return Err("H.264 output end is not after its base".into());
                }
                if end - out > MAX_OUTPUT {
                    return Err("H.264 output span exceeds the model bound".into());
                }
                Some(out)
            }
        };
        let length = config
            .width
            .checked_mul(config.height)
            .and_then(|n| n.checked_mul(2))
            .ok_or("H.264 input size overflow")?;
        if output.is_some() && main < end && out < main + length as u64 {
            return Err("H.264 input frame overlaps bitstream output".into());
        }
        let packed = cx.ram_bytes(main, length)?;
        let planar = h264::planar_420(config.width, config.height, &packed)?;
        let (payload, parameters) = match self.encode(cx, config, &planar, output, end) {
            Ok(encoded) => encoded,
            Err(error) => {
                self.clear_sequence();
                return Err(error);
            }
        };
        self.pending = Some(Pending {
            deadline: cx
                .now()
                .checked_add(self.ticks)
                .ok_or("H.264 deadline overflow")?,
            output,
            payload: payload.into(),
            parameters: parameters.into(),
        });
        self.config = Some(config);
        self.record_history(planar);
        Ok(())
    }
    fn encode(
        &mut self,
        cx: &Cx<'_>,
        config: Config,
        planar: &[u8],
        output: Option<u64>,
        end: u64,
    ) -> Result<(Vec<u8>, Vec<u8>), String> {
        if self.codec.is_none() {
            self.codec = Some(Codec::new(h264::Backend::Software, config, &self.history)?);
        }
        let encoded = self.codec.as_mut().unwrap().encode(config, planar)?;
        let (payload, parameters) = h264::transport_frame(&encoded, &self.parameter_sets)?;
        if let Some(write) = output {
            if payload.len() as u64 > end - write {
                return Err("H.264 frame exceeds the output span".into());
            }
            // Refuse an unmapped destination before accepting the work.
            cx.ram_bytes(write, payload.len())?;
        }
        Ok((payload, parameters))
    }
    fn record_history(&mut self, planar: Vec<u8>) {
        if self.history_truncated {
            return;
        }
        if self.history_bytes + planar.len() > MAX_REPLAY_BYTES {
            self.history_truncated = true;
            self.history.clear();
            self.history_bytes = 0;
            return;
        }
        self.history_bytes += planar.len();
        self.history.push(Arc::from(planar));
    }
    fn snapshot(&self) -> State {
        State {
            words: self.words.clone(),
            pending: self.pending.as_ref().map(|pending| PendingCheckpoint {
                deadline: pending.deadline,
                output: pending.output,
                payload_len: pending.payload.len(),
                parameters_len: pending.parameters.len(),
            }),
            config: self.config,
            parameter_sets: self.parameter_sets.clone(),
            last_frame_len: self.last_frame.len(),
            frames: self.frames,
            output_bytes: self.output_bytes,
            irq_level: self.irq_level,
            history_lengths: self.history.iter().map(|frame| frame.len()).collect(),
            history_truncated: self.history_truncated,
        }
    }
    fn decoded(&self, saved: &Checkpoint) -> Result<State, String> {
        let state: State = saved.decode(self.name())?;
        let pending_payload = state.pending.as_ref().map_or(0, |p| p.payload_len);
        let pending_parameters = state.pending.as_ref().map_or(0, |p| p.parameters_len);
        if saved.blobs().len() != 3 + state.history_lengths.len()
            || saved.blobs()[0].len() != pending_payload
            || saved.blobs()[1].len() != pending_parameters
            || saved.blobs()[2].len() != state.last_frame_len
            || saved.blobs()[3..]
                .iter()
                .zip(&state.history_lengths)
                .any(|(blob, len)| blob.len() != *len)
        {
            return Err("H.264 checkpoint blob sizes differ".into());
        }
        if state.words.iter().any(|(&offset, &value)| {
            offset & 3 != 0 || offset >= WINDOW || !Self::valid(offset) || value == 0
        }) {
            return Err("H.264 checkpoint register differs".into());
        }
        if let Some(pending) = &state.pending {
            if pending.deadline == 0
                || pending.payload_len == 0
                || pending.payload_len > MAX_OUTPUT as usize
                || pending.parameters_len > 128
                || pending.output.is_some_and(|output| output & 7 != 0)
            {
                return Err("H.264 checkpoint pending frame differs".into());
            }
        } else if pending_payload != 0 || pending_parameters != 0 {
            return Err("H.264 checkpoint pending blobs differ".into());
        }
        if let Some(config) = state.config {
            config.validate()?;
            if state
                .history_lengths
                .iter()
                .any(|len| *len != config.frame_bytes())
                || state.parameter_sets.len() > 128
                || state.history_lengths.iter().sum::<usize>() > MAX_REPLAY_BYTES
            {
                return Err("H.264 checkpoint sequence differs".into());
            }
        } else if !state.history_lengths.is_empty() || !state.parameter_sets.is_empty() {
            return Err("H.264 checkpoint sequence blobs differ".into());
        }
        Ok(state)
    }
}
impl Device for Encoder {
    fn name(&self) -> &'static str {
        "xpp-h264-encoder"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(self.base);
        if size != 4 || offset & 3 != 0 || offset >= WINDOW || !Self::valid(offset) {
            cx.unmodeled();
            return 0;
        }
        u64::from(match offset {
            RESET => self.word(RESET) & !START | u32::from(self.ready()),
            STATUS => {
                if self.ready() {
                    READY
                } else {
                    0
                }
            }
            _ => self.word(offset),
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(self.base);
        let Ok(value) = u32::try_from(value) else {
            return cx.unmodeled();
        };
        if size != 4 || offset & 3 != 0 || offset >= WINDOW || !Self::valid(offset) {
            return cx.unmodeled();
        }
        match offset {
            RESET => {
                if value != 0 {
                    return cx.fail("H.264 reset word is not zero");
                }
                self.reset(cx);
            }
            STATUS => cx.unsupported("H.264 status word is read-only"),
            CONTROL => {
                let running = self.word(CONTROL) & START != 0;
                self.set(CONTROL, value);
                if value & IRQ_ENABLE == 0 {
                    self.set_line(cx, false);
                }
                if value & START == 0 {
                    if self.pending.take().is_some() {
                        // A stop before completion aborts the sequence, so the
                        // next frame restarts with parameter sets and an IDR.
                        self.clear_sequence();
                        self.set_line(cx, false);
                    }
                } else if !running && let Err(error) = self.start(cx) {
                    if let Some(w) = self.words.get_mut(&CONTROL) {
                        *w &= !START;
                    }
                    cx.fail(error);
                }
            }
            INPUT_MAIN | INPUT_NEXT | INPUT_PREVIOUS | OUTPUT | OUTPUT_END | OUTPUT_NEXT => {
                if value & 7 != 0 {
                    return cx.fail("H.264 frame pointer is not 8-byte aligned");
                }
                self.set(offset, value);
            }
            INIT | CLOCK | NOTIFY => {
                if value > 1 {
                    return cx.fail("H.264 enable word is not a flag");
                }
                self.set(offset, value);
            }
            SIZE => {
                if value & 0xc000_c000 != 0 {
                    return cx.fail("H.264 picture size exceeds its 14-bit fields");
                }
                self.set(offset, value);
            }
            PHASE => {
                if value & 3 != 0 || value >= 0x4000 {
                    return cx.fail("H.264 phase word exceeds its 14-bit field");
                }
                self.set(offset, value);
            }
            _ => self.set(offset, value),
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if !self
            .pending
            .as_ref()
            .is_some_and(|p| cx.now() >= p.deadline)
        {
            return;
        }
        let pending = self.pending.take().unwrap();
        if let Some(output) = pending.output
            && let Err(error) = cx.ram_write(output, &pending.payload)
        {
            self.clear_sequence();
            self.set_line(cx, false);
            return cx.fail(format!("H.264 output write failed: {error}"));
        }
        self.frames += 1;
        self.output_bytes += pending.payload.len() as u64;
        self.last_frame = pending.payload;
        self.parameter_sets = pending.parameters.to_vec();
        if let Some(control) = self.words.get_mut(&CONTROL) {
            *control &= !START;
        }
        let high = self.word(CONTROL) & IRQ_ENABLE != 0;
        self.set_line(cx, high);
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.pending.as_ref().map(|p| p.deadline)
    }
    fn save(&self) -> Value {
        json!({
            "frames": self.frames,
            "output_bytes": self.output_bytes,
            "pending": self.pending.as_ref().map(|p| json!({
                "deadline": p.deadline,
                "output": p.output,
                "bytes": p.payload.len(),
            })),
            "registers": self.words,
            "irq": self.irq_level,
        })
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        if self.history_truncated {
            return Err("H.264 sequence is too long for an exact replay checkpoint".into());
        }
        let mut blobs: Vec<Arc<[u8]>> = Vec::new();
        if let Some(pending) = &self.pending {
            blobs.push(pending.payload.clone());
            blobs.push(pending.parameters.clone());
        } else {
            blobs.push(Vec::<u8>::new().into());
            blobs.push(Vec::<u8>::new().into());
        }
        blobs.push(self.last_frame.clone());
        blobs.extend(self.history.iter().cloned());
        Ok(Checkpoint::encode(self.name(), &self.snapshot())?.with_blobs(blobs))
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        self.decoded(saved).map(|_| ())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        let state = self.decoded(saved)?;
        let pending = state.pending.as_ref().map(|pending| Pending {
            deadline: pending.deadline,
            output: pending.output,
            payload: saved.blobs()[0].clone(),
            parameters: saved.blobs()[1].clone(),
        });
        let history: Vec<Arc<[u8]>> = saved.blobs()[3..].to_vec();
        let codec = match state.config {
            Some(config) => Some(Codec::new(h264::Backend::Software, config, &history)?),
            None => None,
        };
        self.words = state.words;
        self.pending = pending;
        self.codec = codec;
        self.config = state.config;
        self.parameter_sets = state.parameter_sets;
        self.last_frame = saved.blobs()[2].clone();
        self.frames = state.frames;
        self.output_bytes = state.output_bytes;
        self.irq_level = state.irq_level;
        self.history_bytes = history.iter().map(|frame| frame.len()).sum();
        self.history_truncated = false;
        self.history = history;
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        if self.history_truncated {
            return Err("H.264 sequence is too long for an exact fingerprint".into());
        }
        let pending_payload: &[u8] = self.pending.as_ref().map_or(&[], |p| p.payload.as_ref());
        let pending_parameters: &[u8] =
            self.pending.as_ref().map_or(&[], |p| p.parameters.as_ref());
        Ok(json!({
            "state": self.snapshot(),
            "pending_payload": fujimod_emulator_devices::state::bytes(pending_payload),
            "pending_parameters": fujimod_emulator_devices::state::bytes(pending_parameters),
            "last_frame": fujimod_emulator_devices::state::bytes(&self.last_frame),
            "history": self.history.iter()
                .map(|b| fujimod_emulator_devices::state::bytes(b))
                .collect::<Vec<_>>(),
        }))
    }
    fn diagnostic_buffer(&self, name: &str) -> Result<&[u8], String> {
        match name {
            "last-frame" => Ok(self.last_frame.as_ref()),
            _ => Err(format!("H.264 encoder has no diagnostic buffer {name}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::Host;
    use fujimod_emulator_devices::test_host::{
        TestHost, advance, advance_from_checkpoint, read, write,
    };

    const BASE: u64 = 0xfff5c000;
    // Row 0x31 of the 0xed4720 source table is GIC line 0x6e.
    const LINE: u32 = 110;

    fn configured(h: &mut TestHost) -> Encoder {
        let mut d = Encoder::new(BASE, LINE, 10).unwrap();
        let mut packed = Vec::with_capacity(16 * 16 * 2);
        for _ in 0..16 {
            for column in 0..16 {
                let pair = if column < 8 { 40 } else { 200 };
                packed.extend_from_slice(&[pair, pair, 96, 160]);
            }
        }
        h.ram_write(0x1000, &packed).unwrap();
        for (offset, value) in [
            (RESET, 0),
            (FORMAT, 0x21),
            (SIZE, 16 << 16 | 16),
            (MODE, MOVIE_FORMAT),
            (OUTPUT, 0x40000),
            (OUTPUT_END, 0x50000),
            (OUTPUT_NEXT, 0x42000),
            (INPUT_MAIN, 0x1000),
            (INPUT_NEXT, 0x2000),
            (INPUT_PREVIOUS, 0x3000),
        ] {
            write(&mut d, h, BASE + offset, value);
        }
        d
    }

    #[test]
    fn encodes_one_16x16_frame_and_publishes_status_and_irq() {
        let mut h = TestHost::new();
        let mut d = configured(&mut h);
        assert_eq!(read(&mut d, &mut h, BASE + RESET) & 1, 1);
        assert_eq!(read(&mut d, &mut h, BASE + STATUS), u64::from(READY));

        write(&mut d, &mut h, BASE + CONTROL, 0x10001);
        assert_eq!(read(&mut d, &mut h, BASE + STATUS), 0);
        assert_eq!(d.next_deadline(0), Some(10));
        advance(&mut d, &mut h, 9);
        assert_eq!(d.next_deadline(9), Some(10));
        advance(&mut d, &mut h, 10);
        assert_eq!(read(&mut d, &mut h, BASE + STATUS), u64::from(READY));
        assert_eq!(read(&mut d, &mut h, BASE + CONTROL), 0x10000);
        assert_eq!(d.save()["irq"], true);

        let first = d.diagnostic_buffer("last-frame").unwrap().to_vec();
        let kind = |nal: u8| {
            first
                .windows(5)
                .position(|w| w == [0, 0, 0, 1, nal])
                .unwrap_or_else(|| panic!("missing NAL {nal:#x}"))
        };
        let (sps, pps, idr) = (kind(0x67), kind(0x68), kind(0x65));
        assert!(sps < pps && pps < idr, "SPS/PPS/IDR order");
        let mut decoder = h264::decode::Decoder::new(&[]).unwrap();
        let picture = decoder.decode(&first).unwrap();
        assert_eq!((picture.sequence.width, picture.sequence.height), (16, 16));

        // A later frame is one P picture with the initial parameter sets.
        write(&mut d, &mut h, BASE + CONTROL, 0x10001);
        advance(&mut d, &mut h, 20);
        let second = d.diagnostic_buffer("last-frame").unwrap();
        assert_eq!(second[4] & 31, 1, "second frame is a P slice");
        assert!(!second.windows(5).any(|w| w == [0, 0, 0, 1, 0x67]));
        let picture = decoder.decode(second).unwrap();
        assert_eq!((picture.sequence.width, picture.sequence.height), (16, 16));
        assert_eq!(d.save()["frames"], 2);

        write(&mut d, &mut h, BASE + CONTROL, 0);
        assert_eq!(d.save()["irq"], false);
        assert_eq!(d.save()["frames"], 2);
    }

    #[test]
    fn checkpoint_restores_a_pending_second_frame_and_its_history() {
        let mut h = TestHost::new();
        let mut d = configured(&mut h);
        write(&mut d, &mut h, BASE + CONTROL, 0x10001);
        advance(&mut d, &mut h, 10);
        assert_eq!(d.save()["frames"], 1);

        // The second frame is pending with one planar input in the replay
        // history. The helper saves, advances, restores and advances again,
        // then compares guest RAM, effects, IRQs and the device fingerprint.
        write(&mut d, &mut h, BASE + CONTROL, 0x10001);
        advance_from_checkpoint(&mut d, &mut h, 20);
        assert_eq!(d.save()["frames"], 2);
        assert_eq!(d.save()["irq"], true);
    }

    #[test]
    fn history_replay_preserves_rate_control_state() {
        let config = Config {
            width: 64,
            height: 64,
            frames_per_second: 60,
            bitrate: 40_000_000,
            max_qp: 26,
            gop: 60,
        };
        let frames: Vec<Vec<u8>> = (0..61u64)
            .map(|index| {
                (0..config.frame_bytes())
                    .map(|pixel| ((index * 17 + pixel as u64 * 29) % 251) as u8)
                    .collect()
            })
            .collect();
        let mut continuous = Codec::new(h264::Backend::Software, config, &[]).unwrap();
        let mut out = Vec::new();
        for frame in &frames {
            out.push(continuous.encode(config, frame).unwrap());
        }
        let history: Vec<Arc<[u8]>> = frames[..60].iter().cloned().map(Arc::from).collect();
        let mut replayed = Codec::new(h264::Backend::Software, config, &history).unwrap();
        assert_eq!(
            replayed.encode(config, &frames[60]).unwrap(),
            out[60],
            "replay with the full history, including rate control"
        );
    }

    #[test]
    fn checkpoint_at_the_gop_boundary_replays_sixty_frames() {
        let mut h = TestHost::new();
        let mut d = configured(&mut h);
        for index in 0..60u64 {
            write(&mut d, &mut h, BASE + CONTROL, 0x10001);
            advance(&mut d, &mut h, 10 * (index + 1));
        }
        // Frame 60 is pending with a full 60-frame history. The restore must
        // replay the GOP and produce the same IDR as the uninterrupted run.
        write(&mut d, &mut h, BASE + CONTROL, 0x10001);
        advance_from_checkpoint(&mut d, &mut h, 610);
        assert_eq!(d.save()["frames"], 61);
        let frame = d.diagnostic_buffer("last-frame").unwrap();
        assert!(frame.windows(5).any(|w| w == [0, 0, 0, 1, 0x65]));
    }

    #[test]
    fn accepts_the_stock_zero_output_span_without_writing_guest_ram() {
        let mut h = TestHost::new();
        h.ram.resize(0x2000000, 0);
        // The MOV writer's stream descriptor at 0x1c26dfc is filled by the
        // codec callback 0x167aaf4 (slot 0x1bdd024), not by this block.
        h.ram[0x1C26DFC..0x1C26E0C].fill(0xaa);
        let mut d = configured(&mut h);
        // A stock recording reaches 0x1566200 with context +0x04/+0x18/+0x1c
        // still cleared by 0x184c9d0, so all three registers are zero.
        for offset in [OUTPUT, OUTPUT_END, OUTPUT_NEXT] {
            write(&mut d, &mut h, BASE + offset, 0);
        }
        h.effects.clear();
        write(&mut d, &mut h, BASE + CONTROL, 0x10001);
        advance(&mut d, &mut h, 10);
        assert_eq!(read(&mut d, &mut h, BASE + STATUS), u64::from(READY));
        assert_eq!(d.save()["frames"], 1);
        assert_eq!(d.save()["irq"], true);
        assert!(
            !h.effects.contains(&"dma"),
            "a zero output descriptor must not write guest RAM"
        );
        assert_eq!(
            &h.ram[0x1C26DFC..0x1C26E0C],
            &[0xaa; 16],
            "the movie writer handoff is not faked"
        );
        let frame = d.diagnostic_buffer("last-frame").unwrap();
        assert_eq!(&frame[..5], [0, 0, 0, 1, 0x67], "SPS still produced");
    }

    #[test]
    fn refuses_partial_and_malformed_output_descriptors() {
        let mut h = TestHost::new();
        for (out, end, next, text) in [
            (0x40000, 0, 0, "incomplete"),
            (0, 0x50000, 0, "incomplete"),
            (0, 0, 0x40000, "incomplete"),
            (0x50000, 0x40000, 0, "not after its base"),
            (0x40000, 0x1400008, 0, "exceeds the model bound"),
        ] {
            let mut d = configured(&mut h);
            write(&mut d, &mut h, BASE + OUTPUT, out);
            write(&mut d, &mut h, BASE + OUTPUT_END, end);
            write(&mut d, &mut h, BASE + OUTPUT_NEXT, next);
            let mut cx = h.cx();
            d.write(&mut cx, BASE + CONTROL, 4, 0x10001);
            let refusal = cx.refusal().expect("malformed descriptor must refuse");
            assert!(refusal.contains(text), "{refusal}");
            assert_eq!(read(&mut d, &mut h, BASE + STATUS), u64::from(READY));
        }
    }

    #[test]
    fn keys_every_sixty_frames_like_the_stock_clip() {
        let mut h = TestHost::new();
        let mut d = configured(&mut h);
        for index in 0..61u64 {
            write(&mut d, &mut h, BASE + CONTROL, 0x10001);
            advance(&mut d, &mut h, 10 * (index + 1));
            let frame = d.diagnostic_buffer("last-frame").unwrap();
            let idr = frame.windows(5).any(|w| w == [0, 0, 0, 1, 0x65]);
            assert_eq!(idr, index == 0 || index == 60, "frame {index} keyframe law");
        }
        assert_eq!(d.save()["frames"], 61);
    }

    #[test]
    fn movie_samples_use_the_four_byte_length_writer_law() {
        let mut h = TestHost::new();
        let mut d = configured(&mut h);
        write(&mut d, &mut h, BASE + CONTROL, 0x10001);
        advance(&mut d, &mut h, 10);
        let stream = d.diagnostic_buffer("last-frame").unwrap();
        // 0x167c50c..0x167c5ac replaces each 00 00 00 01 start code with a
        // big-endian NAL length; the guest parser strips 7/8 before the MOV
        // sample is written, so the stock sample is the picture NAL only.
        let mut sample = Vec::new();
        let mut starts = stream
            .windows(4)
            .enumerate()
            .filter_map(|(i, w)| (w == [0, 0, 0, 1]).then_some(i))
            .peekable();
        while let Some(start) = starts.next() {
            let end = starts.peek().copied().unwrap_or(stream.len());
            let nal = &stream[start + 4..end];
            sample.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            sample.extend_from_slice(nal);
        }
        assert!(!sample.windows(4).any(|w| w == [0, 0, 0, 1]));
        let mut offset = 0;
        let mut kinds = Vec::new();
        while offset < sample.len() {
            let len = u32::from_be_bytes(sample[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            kinds.push(sample[offset] & 0x1f);
            offset += len;
        }
        assert_eq!(kinds, [7, 8, 5], "first frame is SPS, PPS, IDR");
        // The guest parser keeps 7/8 for avcC; the MOV sample carries the
        // picture NAL only, matching the first stock sample (type 5).
        let picture: Vec<u8> = kinds
            .iter()
            .copied()
            .filter(|k| !matches!(k, 7 | 8))
            .collect();
        assert_eq!(picture, [5]);
    }

    #[test]
    fn refuses_unaligned_pointers_unmodeled_words_and_unsupported_formats() {
        let mut h = TestHost::new();
        let mut d = Encoder::new(BASE, LINE, 10).unwrap();
        {
            let mut cx = h.cx();
            d.write(&mut cx, BASE + INPUT_MAIN, 4, 0x1002);
            assert!(cx.refusal().unwrap().contains("8-byte aligned"));
        }
        {
            let mut cx = h.cx();
            d.write(&mut cx, BASE + 0x404, 4, 1);
            assert!(cx.is_unmodeled());
        }
        {
            let mut cx = h.cx();
            d.write(&mut cx, BASE + STATUS, 4, 1);
            assert!(cx.refusal().unwrap().contains("read-only"));
        }
        let mut d = configured(&mut h);
        write(&mut d, &mut h, BASE + MODE, 0);
        {
            let mut cx = h.cx();
            d.write(&mut cx, BASE + CONTROL, 4, 0x10001);
            assert!(cx.refusal().unwrap().contains("unsupported format"));
        }
        assert_eq!(read(&mut d, &mut h, BASE + STATUS), u64::from(READY));
        let mut d = configured(&mut h);
        write(&mut d, &mut h, BASE + SIZE, 15 << 16 | 15);
        {
            let mut cx = h.cx();
            d.write(&mut cx, BASE + CONTROL, 4, 0x10001);
            assert!(cx.refusal().unwrap().contains("H.264 requires"));
        }
        assert_eq!(read(&mut d, &mut h, BASE + STATUS), u64::from(READY));
    }
}
