//! Register-driven FFF60000 AVC codec. Encoding consumes packed YYCbCr;
//! alternate decoding consumes Annex B stream banks and publishes I420 frames.
//! DMA precedes status/IRQ publication. The declared clock is 64 luma samples
//! per tick (setup/stop: two ticks), independent of host codec speed.
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint};
use fujimod_emulator_image_codecs::h264::{self, Codec, Config};
use fujimod_emulator_imaging::exr2_isp::dma::physical;
use fujimod_emulator_imaging::exr2_isp::{due, word};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
mod decode;
use decode::{DecodeInput, DecodeOutput, DecodeRuntime, DecodeState};

pub use fujimod_catalog_construction::AvcCodecOptions as Options;
use fujimod_catalog_construction::VideoEncoderBackend as Backend;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Pending {
    deadline: u64,
    writes: Vec<(u64, usize)>,
    cursor: u32,
    bank: u32,
    bits: u32,
    flags: u32,
    mode: u32,
    decoded: Option<DecodeOutput>,
    decode_input: Option<DecodeInput>,
}
#[derive(Serialize, Deserialize)]
struct State {
    backend: Backend,
    r: Vec<u32>,
    pending: Option<Pending>,
    finish_requested: bool,
    irq_level: bool,
    bank_available: [bool; 2],
    config: Option<Config>,
    parameter_sets: Vec<u8>,
    frames: u64,
    output_bytes: u64,
    history_lengths: Vec<usize>,
    payload_length: usize,
    mode: u32,
    decode: DecodeState,
}
pub struct AvcCodec {
    state: State,
    irq: Irq,
    payload: Arc<[u8]>,
    history: Vec<Arc<[u8]>>,
    codec: Option<Codec>,
    decode_runtime: DecodeRuntime,
}
impl AvcCodec {
    pub fn new(options: Options, irq: Irq) -> Result<Self, String> {
        h264::validate_backend(options.backend)?;
        Ok(Self {
            state: State {
                backend: options.backend,
                r: vec![0; 0x1000 / 4],
                pending: None,
                finish_requested: false,
                irq_level: false,
                bank_available: [false; 2],
                config: None,
                parameter_sets: Vec::new(),
                frames: 0,
                output_bytes: 0,
                history_lengths: Vec::new(),
                payload_length: 0,
                mode: 1,
                decode: DecodeState::default(),
            },
            irq,
            payload: Arc::from([]),
            history: Vec::new(),
            codec: None,
            decode_runtime: DecodeRuntime::default(),
        })
    }
    fn r(&self, offset: usize) -> u32 {
        self.state.r[offset / 4]
    }
    fn set(&mut self, offset: usize, value: u32) {
        self.state.r[offset / 4] = value;
    }
    fn valid(o: usize) -> bool {
        matches!(o,0x400..=0x40c|0x440|0x444|0x490..=0x4a4|0x4ac..=0x4b8|0x4c4|0x4c8|0x4fc|0xb00|0xb04|0xb08|0xb6c|0xb74..=0xb80|0xbd0..=0xbe0|0xc00|0xc20|0xc24|0xc80..=0xc9c|0xca8|0xcbc|0xd00..=0xd28|0xd44..=0xd7c|0xd88|0xd8c|0xdc8|0xdcc|0xde0|0xe00..=0xe24|0xe30..=0xe50|0xe58..=0xef0|0xf00..=0xf1c|0xfc0)
    }
    fn configuration(&self) -> Result<Config, String> {
        // Admit the progressive packed-input path. Scaling, cropped/field
        // input, alternate codecs, auxiliary image prediction and uploaded
        // codec programs have no corresponding software implementation.
        for (o, mask, value) in [
            (0xe00, !0x7ff, 0),
            (0xe04, !0x7ff, 0x2000),
            (0xe18, u32::MAX, 0),
            (0xe1c, u32::MAX, 0),
            (0xe38, u32::MAX, 0x8000),
            (0xe3c, u32::MAX, 0),
            (0xe40, u32::MAX, 0),
            (0xe44, u32::MAX, 4),
            (0xe48, u32::MAX, 0x400),
            (0xe94, u32::MAX, 0x348),
            (0xea0, u32::MAX, 0x100),
            (0x440, u32::MAX, 0),
            (0x444, u32::MAX, 0),
            (0x4fc, u32::MAX, 0),
        ] {
            if self.r(o) & mask != value {
                return Err(format!(
                    "H.264 unsupported mode register {o:#x}={:#x}",
                    self.r(o)
                ));
            }
        }
        for (index, value) in self.state.r.iter().enumerate() {
            let offset = index * 4;
            if *value != 0
                && !matches!(offset,
                0x400..=0x408 | 0x490..=0x4a4 | 0x4ac | 0xbd0..=0xbe0 |
                0xc20 | 0xc24 | 0xca8 | 0xd88 | 0xd8c |
                0xe00..=0xe14 | 0xe20 | 0xe24 | 0xe30 | 0xe34 | 0xe38 |
                0xe44 | 0xe48 | 0xe94 | 0xea0 | 0xf00..=0xfc0)
            {
                return Err(format!("H.264 unsupported register {offset:#x}={value:#x}"));
            }
        }
        let rate = match self.r(0xe0c) {
            23 => 24,
            29 => 30,
            59 => 60,
            rate => rate,
        };
        let config = Config {
            width: (self.r(0xe00) & 0x7ff) as usize,
            height: (self.r(0xe04) & 0x7ff) as usize,
            frames_per_second: rate,
            bitrate: self
                .r(0xe08)
                .checked_mul(1024)
                .ok_or("H.264 bitrate overflow")?,
            max_qp: u8::try_from(self.r(0xe10)).map_err(|_| "H.264 QP range")?,
            gop: self.r(0xe34) as usize,
        };
        config.validate()?;
        Ok(config)
    }
    fn notice(&mut self, now: u64, flags: u32) -> Result<(), String> {
        if self.state.pending.is_some() {
            return Err("H.264 command while busy or interrupt unacknowledged".into());
        }
        self.state.pending = Some(Pending {
            deadline: due(now, 2)?,
            writes: Vec::new(),
            cursor: self.r(0x4ac),
            bank: (self.r(0x4a4) >> 5) & 1,
            bits: 0,
            flags,
            mode: self.state.mode,
            decoded: None,
            decode_input: None,
        });
        Ok(())
    }
    fn encode(&mut self, cx: &Cx<'_>) -> Result<(), String> {
        if self.state.mode != 1 {
            return Err("AVC mode change requires initialization".into());
        }
        if self.state.pending.is_some() {
            return Err("H.264 frame submitted while busy".into());
        }
        let config = self.configuration()?;
        self.state.mode = 1;
        if self.state.config.is_some_and(|c| c != config) {
            return Err("H.264 sequence configuration changed without initialization".into());
        }
        if self.r(0x4a4) & 0x10 == 0 {
            return Err("H.264 bitstream output is disabled".into());
        }
        let sizes = [self.r(0x494) as usize, self.r(0x49c) as usize];
        let bases = [
            physical(self.r(0x490) as u64, sizes[0])?,
            physical(self.r(0x498) as u64, sizes[1])?,
        ];
        if sizes.iter().any(|n| n % 4 != 0)
            || bases.iter().any(|n| n % 4 != 0)
            || bases[0] < bases[1] + sizes[1] as u64 && bases[1] < bases[0] + sizes[0] as u64
        {
            return Err("H.264 bitstream banks overlap or are unaligned".into());
        }
        let length = config.width * config.height * 2;
        let source = physical(self.r(0x400) as u64, length)?;
        if bases
            .iter()
            .zip(sizes)
            .any(|(base, size)| source < base + size as u64 && *base < source + length as u64)
        {
            return Err("H.264 input overlaps bitstream output".into());
        }
        let packed = cx.ram_bytes(source, length)?;
        let planar: Arc<[u8]> = h264::planar_420(config.width, config.height, &packed)?.into();
        // Each bounded GOP starts with a fresh IDR and constant parameter-set
        // IDs. Replaying only these owned inputs reconstructs the exact codec.
        if self.history.len() == config.gop {
            self.history.clear();
            self.codec = None;
        }
        if self.codec.is_none() {
            self.codec = Some(Codec::new(self.state.backend, config, &self.history)?);
        }
        let encoded = self.codec.as_mut().unwrap().encode(config, &planar)?;
        let (mut payload, parameter_sets) =
            h264::transport_frame(&encoded, &self.state.parameter_sets)?;
        let bits = u32::try_from(payload.len() * 8).map_err(|_| "H.264 frame size overflow")?;
        payload.resize(payload.len().next_multiple_of(4), 0);
        if payload.len() > *sizes.iter().min().unwrap() {
            return Err("H.264 frame exceeds one bitstream bank".into());
        }
        let mut bank = ((self.r(0x4a4) >> 5) & 1) as usize;
        let mut cursor = self.r(0x4ac) as usize;
        if cursor >= sizes[bank] {
            return Err("H.264 output cursor outside ring".into());
        }
        let mut writes = Vec::new();
        let mut available = self.state.bank_available;
        let mut remaining = payload.len();
        // Offset zero alone is not a crossing. The bank notice identifies a
        // wrap within this access unit, independently of frame completion.
        let mut flags = 1 << 12;
        while remaining > 0 {
            if !available[bank] {
                return Err("H.264 bitstream bank has not been released".into());
            }
            let n = remaining.min(sizes[bank] - cursor);
            writes.push((bases[bank] + cursor as u64, n));
            remaining -= n;
            cursor += n;
            if cursor == sizes[bank] {
                available[bank] = false;
                bank ^= 1;
                cursor = 0;
                flags |= 1 << 9;
            }
        }
        // Read both destination spans now to refuse an unmapped DMA before
        // accepting work; the runtime independently validates every due write.
        for (address, length) in &writes {
            cx.ram_bytes(*address, *length)?;
        }
        self.state.pending = Some(Pending {
            deadline: due(cx.now(), (config.width * config.height).div_ceil(64) as u64)?,
            writes,
            cursor: cursor as u32,
            bank: bank as u32,
            bits,
            flags,
            mode: 1,
            decoded: None,
            decode_input: None,
        });
        self.state.parameter_sets = parameter_sets;
        self.state.bank_available = available;
        self.payload = payload.into();
        self.history.push(planar);
        self.state.config = Some(config);
        self.state.payload_length = self.payload.len();
        self.state.history_lengths = self.history.iter().map(|b| b.len()).collect();
        Ok(())
    }
    fn command(&mut self, cx: &Cx<'_>, value: u32) -> Result<(), String> {
        match value {
            0 => Ok(()),
            0x800 => {
                self.state.pending = None;
                self.state.finish_requested = false;
                self.state.config = None;
                self.state.parameter_sets.clear();
                self.codec = None;
                self.history.clear();
                self.payload = Arc::from([]);
                self.state.history_lengths.clear();
                self.state.payload_length = 0;
                self.set(0x4ac, 0);
                self.set(0xc20, 0);
                self.set(0xc24, 0);
                self.set(0x4a4, self.r(0x4a4) & !32);
                self.state.bank_available = [self.r(0x4a4) & 4 != 0, self.r(0x4a4) & 8 != 0];
                self.state.mode = 1;
                self.state.decode.sequence = None;
                self.state.decode.parameter_bytes = 0;
                self.state.decode.history_lengths.clear();
                self.state.decode.next_slot = 0;
                self.decode_runtime.parameters = Arc::from([]);
                self.decode_runtime.history.clear();
                self.decode_runtime.codec = None;
                self.decode_runtime.pending_input = Arc::from([]);
                self.state.decode.pending_input_bytes = 0;
                Ok(())
            }
            0x8000 => self.encode(cx),
            0x4000 => self.decode_frame(cx),
            0x2000 => {
                if self.state.pending.is_some() {
                    self.state.finish_requested = true;
                    Ok(())
                } else {
                    self.notice(cx.now(), 1 << if self.state.mode == 2 { 13 } else { 15 })
                }
            }
            0x1000 => self.notice(cx.now(), 1 << 14),
            _ => Err(format!("H.264 unsupported command {value:#x}")),
        }
    }
    fn publish_irq(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let high = self.r(0xc24) != 0;
        if high != self.state.irq_level {
            cx.set_core_irq(self.irq.core, self.irq.line, high)?;
            self.state.irq_level = high;
        }
        Ok(())
    }
    fn decoded(&self, saved: &Checkpoint) -> Result<State, String> {
        let s: State = saved.decode(self.name())?;
        if s.backend != self.state.backend {
            return Err("H.264 checkpoint backend differs".into());
        }
        if s.backend == Backend::Videotoolbox && !s.history_lengths.is_empty() {
            return Err(
                "Active VideoToolbox sequences cannot replay identically; select software".into(),
            );
        }
        if s.parameter_sets.len() > 88
            || s.r.len() != 0x1000 / 4
            || s.irq_level != (s.r[0xc24 / 4] != 0)
            || saved.blobs().len() != s.history_lengths.len() + s.decode.history_lengths.len() + 4
            || saved.blobs()[0].len() != s.payload_length
            || saved.blobs()[1..1 + s.history_lengths.len()]
                .iter()
                .zip(&s.history_lengths)
                .any(|(b, n)| b.len() != *n)
            || s.config.is_none() && (!s.history_lengths.is_empty() || !s.parameter_sets.is_empty())
            || s.config.is_some() && s.parameter_sets.is_empty()
        {
            return Err("H.264 checkpoint sizes differ".into());
        }
        let decode_offset = 1 + s.history_lengths.len();
        if !(1..=2).contains(&s.mode)
            || s.decode.parameter_bytes > 128
            || s.decode.input_bytes > 2 * 1024 * 1024
            || s.decode.pending_input_bytes > 2 * 1024 * 1024
            || s.decode.history_lengths.len() > 120
            || s.decode.history_lengths.iter().sum::<usize>() > 16 * 1024 * 1024
            || saved.blobs()[decode_offset].len() != s.decode.parameter_bytes
            || saved.blobs()[decode_offset + 1].len() != s.decode.input_bytes
            || saved.blobs()[decode_offset + 2].len() != s.decode.pending_input_bytes
            || saved.blobs()[decode_offset + 3..]
                .iter()
                .zip(&s.decode.history_lengths)
                .any(|(b, n)| b.len() != *n)
        {
            return Err("AVC checkpoint decode sizes differ".into());
        }
        if let Some(pending) = &s.pending {
            let total = pending
                .writes
                .iter()
                .try_fold(0usize, |n, (_, size)| n.checked_add(*size));
            if pending.writes.len() > 2
                || pending.bank > 1
                || !(1..=2).contains(&pending.mode)
                || pending.flags & !0xfe00 != 0
                || !pending.writes.is_empty() && total != Some(s.payload_length)
            {
                return Err("H.264 checkpoint pending DMA differs".into());
            }
            for (address, size) in &pending.writes {
                physical(*address, *size)?;
            }
            if let Some(output) = &pending.decoded
                && (pending.mode != 2
                    || output.packing != "i420"
                    || output.width < 16
                    || output.width > 1920
                    || output.height < 16
                    || output.height > 1088
                    || output.stride != output.width
                    || output.bytes != output.stride * output.height * 3 / 2
                    || output.completed_tick != pending.deadline
                    || output.slot >= 3
                    || pending.writes
                        != [(physical(output.destination, output.bytes)?, output.bytes)]
                    || fujimod_emulator_devices::fingerprint::bytes(&saved.blobs()[0])["sha256"]
                        .as_str()
                        != Some(&output.sha256)
                    || fujimod_emulator_devices::fingerprint::bytes(
                        &saved.blobs()[decode_offset + 2],
                    )["sha256"]
                        .as_str()
                        != Some(&output.input_sha256)
                    || pending
                        .decode_input
                        .as_ref()
                        .is_none_or(|input| input.sha256 != output.input_sha256))
            {
                return Err("AVC checkpoint decoded output differs".into());
            }
        }
        if let Some(c) = s.config {
            c.validate()?;
            if s.history_lengths.len() > c.gop
                || s.history_lengths.iter().any(|n| *n != c.frame_bytes())
            {
                return Err("H.264 checkpoint GOP differs".into());
            }
        }
        Ok(s)
    }
}
impl Device for AvcCodec {
    fn name(&self) -> &'static str {
        "exr2-isp-avc-codec"
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        if self.state.backend == Backend::Videotoolbox && !self.history.is_empty() {
            return Err(
                "Active VideoToolbox sequences cannot replay identically; select software".into(),
            );
        }
        let mut blobs = vec![self.payload.clone()];
        blobs.extend(self.history.iter().cloned());
        blobs.push(self.decode_runtime.parameters.clone());
        blobs.push(self.decode_runtime.input.clone());
        blobs.push(self.decode_runtime.pending_input.clone());
        blobs.extend(self.decode_runtime.history.iter().cloned());
        Ok(Checkpoint::encode(self.name(), &self.state)?.with_blobs(blobs))
    }
    fn validate_checkpoint(&self, s: &Checkpoint) -> Result<(), String> {
        self.decoded(s).map(|_| ())
    }
    fn restore_checkpoint(&mut self, s: &Checkpoint) -> Result<(), String> {
        self.state = self.decoded(s)?;
        self.payload = s.blobs()[0].clone();
        let offset = 1 + self.state.history_lengths.len();
        self.history = s.blobs()[1..offset].to_vec();
        self.decode_runtime.parameters = s.blobs()[offset].clone();
        self.decode_runtime.input = s.blobs()[offset + 1].clone();
        self.decode_runtime.pending_input = s.blobs()[offset + 2].clone();
        self.decode_runtime.history = s.blobs()[offset + 3..].to_vec();
        self.decode_runtime.codec = None;
        self.codec = None;
        Ok(())
    }
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        if self.state.backend == Backend::Videotoolbox && !self.history.is_empty() {
            return Err("Active VideoToolbox sequences have no identical replay fingerprint; select software".into());
        }
        Ok(
            serde_json::json!({"state":self.state,"payload":fujimod_emulator_devices::fingerprint::bytes(&self.payload),
            "history":self.history.iter().map(|b|fujimod_emulator_devices::fingerprint::bytes(b)).collect::<Vec<_>>(),
            "decode_input":fujimod_emulator_devices::fingerprint::bytes(&self.decode_runtime.input),
            "decode_pending_input":fujimod_emulator_devices::fingerprint::bytes(&self.decode_runtime.pending_input),
            "decode_parameters":fujimod_emulator_devices::fingerprint::bytes(&self.decode_runtime.parameters),
            "decode_history":self.decode_runtime.history.iter().map(|b|fujimod_emulator_devices::fingerprint::bytes(b)).collect::<Vec<_>>()}),
        )
    }
    fn read(&mut self, cx: &mut Cx<'_>, a: u64, s: u8) -> u64 {
        let Some(o) = word(cx, a, s, 0xfff60000, 0x1000) else {
            return 0;
        };
        if !Self::valid(o) {
            cx.unmodeled();
            return 0;
        }
        self.r(o) as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, a: u64, s: u8, v: u64) {
        let Some(o) = word(cx, a, s, 0xfff60000, 0x1000) else {
            return;
        };
        if !Self::valid(o) || v > u32::MAX as u64 {
            cx.unmodeled();
            return;
        }
        let v = v as u32;
        if o == 0xc24 {
            self.set(o, self.r(o) & !v);
            if let Err(error) = self.publish_irq(cx) {
                cx.fail(error);
            }
            return;
        }
        if o == 0xc00 {
            if let Err(error) = self.command(cx, v).and_then(|()| self.publish_irq(cx)) {
                cx.fail(error);
            }
            return;
        }
        if o == 0x4a4 {
            if v & !0x3d != 0 {
                cx.fail("H.264 unsupported bitstream bank control bits");
                return;
            }
            // Each set bit returns that bank to the encoder. A zero does
            // not revoke the other bank while an access unit is in flight.
            for (index, available) in self.state.bank_available.iter_mut().enumerate() {
                *available |= v & (4 << index) != 0;
            }
            self.set(o, (v & !32) | (self.r(o) & 32));
            return;
        }
        self.set(o, v);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        if (self.r(0xc24) != 0) != self.state.irq_level {
            return Some(now);
        }
        self.state.pending.as_ref().map(|p| p.deadline)
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self
            .state
            .pending
            .as_ref()
            .is_some_and(|p| cx.now() >= p.deadline)
        {
            let p = self.state.pending.take().unwrap();
            let mut offset = 0;
            for (address, length) in &p.writes {
                if let Err(e) = cx.ram_write(*address, &self.payload[offset..offset + length]) {
                    cx.fail(e);
                    return;
                }
                offset += length;
            }
            if !p.writes.is_empty() {
                self.set(0x4ac, p.cursor);
                self.set(0x4a4, (self.r(0x4a4) & !32) | (p.bank << 5));
                if let Some(output) = p.decoded {
                    self.state.decode.last_input = p.decode_input;
                    self.decode_runtime.input =
                        std::mem::replace(&mut self.decode_runtime.pending_input, Arc::from([]));
                    self.state.decode.input_bytes = self.decode_runtime.input.len();
                    self.state.decode.pending_input_bytes = 0;
                    self.state.decode.frames += 1;
                    self.state.decode.next_slot = (output.slot + 1) % 3;
                    self.set(0xfc0, output.slot);
                    self.set(0xf08, output.width as u32);
                    self.set(0xf0c, output.height as u32);
                    self.state.decode.last_output = Some(output);
                } else {
                    self.state.frames += 1;
                    self.state.output_bytes += u64::from(p.bits.div_ceil(8));
                    self.set(0xd88, p.bits & 65535);
                    self.set(0xd8c, p.bits >> 16);
                }
            }
            if p.flags & (1 << 15) != 0 {
                self.codec = None;
                self.history.clear();
                self.state.history_lengths.clear();
                self.state.config = None;
                self.state.parameter_sets.clear();
            }
            self.set(0xc20, p.mode << 2);
            self.set(0xc24, self.r(0xc24) | p.flags);
            if self.state.finish_requested {
                self.state.finish_requested = false;
                if let Err(e) =
                    self.notice(cx.now(), 1 << if self.state.mode == 2 { 13 } else { 15 })
                {
                    cx.fail(e);
                    return;
                }
            }
        }
        if let Err(error) = self.publish_irq(cx) {
            cx.fail(error);
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx)
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!({"backend":self.state.backend,"frames":self.state.frames,"output_bytes":self.state.output_bytes,
            "registers":self.state.r,"pending":self.state.pending,"gop_frames":self.history.len(),"decode":self.state.decode})
    }
    fn diagnostic_buffer(&self, name: &str) -> Result<&[u8], String> {
        match name {
            "decode-input" => Ok(&self.decode_runtime.input),
            _ => Err(format!("AVC codec has no diagnostic buffer {name}")),
        }
    }
}
