//! X-Processor Pro display frame interrupts.
//!
//! An interrupt law, not a register model of the display controller: each
//! declared line receives one edge per frame period, counted from tick zero.
//! The X-T20 2.11 frame handlers (0xc9d854, 0xc9d994) keep their callback
//! state in RAM and read no controller register, so nothing is acknowledged.
//! Periods that pass between device calls raise one edge per line. The period
//! is a declared input; the panel rate is not measured.
//!
//! [`Output`] is the register window of one display output: word registers
//! read back their last write, except for declared status bits that read set
//! (a request the controller completes at once) or clear (a busy bit the
//! controller clears at once). No frame timing is attached to the status bits.
//!
//! An output with a declared [`Scanout`] captures its OSD layer group once per
//! declared period, as the panel reads the buffers every frame. The X-T20
//! 2.11 commit routine 0xca5a0c copies its shadow block to the group: +0x200
//! enables the four layers by bit, then four words each from shadow +0x1c4,
//! +0x1c8 and +0x1cc to the origins at +0x210, the sizes at +0x220 and the
//! buffers at +0x230. Sizes, like the output size at +0x014, hold
//! `(width - 1) << 16 | (height - 1)`. The firmware writes no pitch; the
//! camera's OSD buffers (0x354d000, 0x384d000) are 720 x 480 premultiplied
//! RGBA words with a 2880-byte pitch, so rows are packed. Layers compose in
//! index order over the video layer, or over black without it.
//!
//! The same routine copies the video layer's words: shadow +0x10 to the
//! format at +0x010, +0x3c to the size at +0x02c and +0x40 to the buffer at
//! +0x030. The camera holds format 0x333, size 720 x 480 and the live-view
//! buffer 0xa7d6000 there in REC, with +0x000 = 0x1010105.
//!
//! +0x000 bit 0 enables the video plane and bit 2 the OSD layer group. The
//! firmware update page shows the OSD alone: it writes 0x1010104 (bit 0
//! clear, bit 2 set) with the video size and buffer still zero. The playback information-off page shows why: the stock
//! firmware writes 0x1010101 there (bit 2 clear) while the video words still
//! name the decoded photo (0x5958000, format 0x333, 720 x 480) and the OSD
//! layer buffer (0x384d000) is all zero, and that page's own sub-screen is
//! UILIB's PLAYSTART_OSDOFF: the page is the photo with the OSD group off.
//! Reading bit 2 as the video enable instead made that page black (OSD only),
//! which no camera observation supports; the REC value 0x1010105 has both
//! bits set, so it does not distinguish the two readings.
//!
//! A video layer whose words are all zero (format, size and buffer) is not
//! yet programmed: the output shows only its OSD layers, or nothing (no
//! frame) without them. Any other unsupported video word refuses. A plane
//! larger than the output size shows its top-left part, clipped to the
//! output's active area (VIEW MODE switches between the EVF and the rear
//! screen scan one frame with the new timing and the old plane sizes).
//! Format 0x333 is packed 4:2:2 `Y0 Y1 Cb Cr` with rows of two bytes per
//! pixel, as in the camera's live-view buffers. The conversion to RGB
//! (BT.601, limited range) is the shared raster law, not a measured one.
//! Frames keep the byte order of the OSD buffers (blue, green, red, alpha:
//! the camera's red battery icon reads blue as RGBA), so video pixels are
//! stored in that order too.
//!
//! A declared handshake clock makes +0 a bit-16 status: a falling request
//! sets status after that delay; a set request clears it. Other words retain
//! writes. This is the same output controller with a timed completion law.
use fujimod_emulator_devices::graphics::scanout::Frame;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub struct FrameClock {
    irqs: Vec<u32>,
    period: u64,
    next: u64,
}
impl FrameClock {
    pub fn new(irqs: Vec<u32>, period: u64) -> Result<Self, String> {
        let mut unique = irqs.clone();
        unique.sort_unstable();
        unique.dedup();
        if irqs.is_empty()
            || unique.len() != irqs.len()
            || irqs.iter().any(|irq| !(32..512).contains(irq))
            || period == 0
        {
            return Err("Invalid X-Processor Pro display frame interrupts".into());
        }
        Ok(Self {
            irqs,
            period,
            next: period,
        })
    }
}
impl Device for FrameClock {
    fn name(&self) -> &'static str {
        "xpp-display-frames"
    }
    fn read(&mut self, cx: &mut Cx<'_>, _address: u64, _size: u8) -> u64 {
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, _address: u64, _size: u8, _value: u64) {
        cx.unmodeled();
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let now = cx.now();
        if now < self.next {
            return;
        }
        for &irq in &self.irqs {
            cx.set_irq(irq, true);
            cx.set_irq(irq, false);
        }
        let frames = (now - self.next) / self.period + 1;
        match frames
            .checked_mul(self.period)
            .and_then(|step| self.next.checked_add(step))
        {
            Some(next) => self.next = next,
            None => cx.fail("Display frame deadline overflow"),
        }
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        Some(self.next)
    }
    fn save(&self) -> Value {
        json!({ "next": self.next })
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let next = value["next"]
            .as_u64()
            .filter(|next| next % self.period == 0 && *next > 0)
            .ok_or("Invalid display frame checkpoint")?;
        self.next = next;
        Ok(())
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(self.name(), &self.next)
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        if !saved.children().is_empty() || !saved.blobs().is_empty() {
            return Err("Display frame checkpoint has unexpected parts".into());
        }
        let next: u64 = saved.decode(self.name())?;
        if next == 0 || !next.is_multiple_of(self.period) {
            return Err("Invalid display frame checkpoint".into());
        }
        Ok(())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        self.next = saved.decode(self.name())?;
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.checkpoint()?.state())
    }
}

use fujimod_framebuffer_xpp::{CONTROL, VIDEO_ENABLE, LAYER_ENABLE, LAYER_BUFFER, VIDEO_SIZE, VIDEO_BUFFER, LAYER_SIZE};
#[cfg(test)]
use fujimod_framebuffer_xpp::{LAYER_ORIGIN, OSD_ENABLE, OUTPUT_SIZE, PACKED_422, VIDEO_FORMAT};

/// Host output index and capture period of a scanned-out display output.
#[derive(Clone, Copy, Debug)]
pub struct Scanout {
    pub output: usize,
    pub period: u64,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Capture {
    next: u64,
    sequence: u64,
    hash: Option<u64>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Handshake {
    status: bool,
    due: Option<u64>,
}
pub struct Output {
    base: u64,
    size: u64,
    set: BTreeMap<u64, u32>,
    clear: BTreeMap<u64, u32>,
    words: BTreeMap<u64, u32>,
    scanout: Option<Scanout>,
    capture: Capture,
    /// The last composed frame and its hash, keyed by the inputs `compose` read.
    last: Option<(Vec<u8>, (u32, u32, Vec<u8>, u64), u64)>,
    handshake_ticks: Option<u64>,
    handshake: Handshake,
}
impl Output {
    /// `set` and `clear` map word offsets to the status bits that read as
    /// set or clear regardless of the last write.
    pub fn new(
        base: u64,
        size: u64,
        set: &[(u64, u32)],
        clear: &[(u64, u32)],
        scanout: Option<Scanout>,
    ) -> Result<Self, String> {
        let mut laws = [BTreeMap::new(), BTreeMap::new()];
        for (law, bits) in laws.iter_mut().zip([set, clear]) {
            for &(offset, mask) in bits {
                if offset & 3 != 0 || offset >= size || mask == 0 {
                    return Err("Invalid display output status bits".into());
                }
                *law.entry(offset).or_insert(0) |= mask;
            }
        }
        let [set, clear] = laws;
        if base & 0xfff != 0
            || size == 0
            || size & 0xfff != 0
            || set
                .iter()
                .any(|(offset, mask)| clear.get(offset).is_some_and(|c| c & mask != 0))
        {
            return Err("Invalid display output window".into());
        }
        if scanout.is_some_and(|s| s.period == 0 || size <= LAYER_BUFFER + 12) {
            return Err("Invalid display output scanout".into());
        }
        Ok(Self {
            base,
            size,
            set,
            clear,
            words: BTreeMap::new(),
            scanout,
            handshake_ticks: None,
            handshake: Handshake::default(),
            last: None,
            capture: Capture {
                next: scanout.map_or(0, |s| s.period),
                ..Capture::default()
            },
        })
    }
    /// Use timed bit-16 request completion in place of fixed bits at +0.
    pub fn with_handshake(mut self, ticks: u64) -> Result<Self, String> {
        if ticks == 0 || self.set.contains_key(&0) || self.clear.contains_key(&0) {
            return Err("Invalid display output handshake clock/status".into());
        }
        self.handshake_ticks = Some(ticks);
        Ok(self)
    }
    fn word(&self, offset: u64) -> u32 {
        self.words.get(&offset).copied().unwrap_or(0)
    }
    fn compose(&self,cx:&mut Cx<'_>)->Result<Option<(u32,u32,Vec<u8>,u64)>,()> {
        fujimod_framebuffer_xpp::Decoder{words:&self.words}.compose(|a,n|cx.ram_bytes(a,n))
    }
    /// The registers and source bytes `compose` reads; equal inputs compose
    /// the same frame. `None` when a layer cannot be read.
    fn inputs(&self, cx: &mut Cx<'_>) -> Option<Vec<u8>> {
        let mut key: Vec<u8> = self
            .words
            .iter()
            .flat_map(|(offset, value)| offset.to_le_bytes().into_iter().chain(value.to_le_bytes()))
            .collect();
        if self.word(CONTROL) & VIDEO_ENABLE != 0 {
            let (w, h) = fujimod_framebuffer_xpp::Decoder::extent(self.word(VIDEO_SIZE))?;
            let buffer = u64::from(self.word(VIDEO_BUFFER));
            key.extend(cx.ram_bytes(buffer, w as usize * h as usize * 2).ok()?);
        }
        for layer in (0..4u64).filter(|n| self.word(LAYER_ENABLE) & 1 << n != 0) {
            let (w, h) = fujimod_framebuffer_xpp::Decoder::extent(self.word(LAYER_SIZE + 4 * layer))?;
            let buffer = u64::from(self.word(LAYER_BUFFER + 4 * layer));
            key.extend(cx.ram_bytes(buffer, w as usize * h as usize * 4).ok()?);
        }
        Some(key)
    }
    fn scan(&mut self, cx: &mut Cx<'_>, scanout: Scanout) {
        let inputs = self.inputs(cx);
        let cached = match (&inputs, &self.last) {
            (Some(inputs), Some((key, frame, hash))) if inputs == key => {
                Some((frame.clone(), *hash))
            }
            _ => None,
        };
        let ((width, height, rgba, source), hash) = if let Some(cached) = cached {
            cached
        } else {
            self.last = None;
            let frame = match self.compose(cx) {
                Ok(Some(frame)) => frame,
                Ok(None) => {
                    let _ = cx.output_status(
                        scanout.output,
                        json!({ "active": false, "tick": cx.now() }),
                    );
                    return;
                }
                Err(()) => {
                    cx.unmodeled();
                    return;
                }
            };
            // FNV-1a: a stable digest for the image-change sequence.
            let hash = [frame.0, frame.1]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .chain(frame.2.iter().copied())
                .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
                    (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
                });
            if let Some(inputs) = inputs {
                self.last = Some((inputs, frame.clone(), hash));
            }
            (frame, hash)
        };
        if self.capture.hash != Some(hash) {
            self.capture.sequence += 1;
            self.capture.hash = Some(hash);
        }
        let sequence = self.capture.sequence;
        let decoder = fujimod_framebuffer_xpp::Decoder { words: &self.words };
        let video = decoder.video_shown();
        let layers = decoder.layers_shown();
        let status = json!({
            "active": true, "format": "rgba8888-premultiplied", "layers": layers,
            "source": format!("0x{source:x}"), "sequence": sequence, "width": width,
            "height": height, "tick": cx.now(), "capture": if video {
                "osd-layers-over-video"
            } else {
                "osd-layers-over-black"
            },
        });
        let delivered = cx.output_status(scanout.output, status).and_then(|()| {
            cx.scanout(Frame {
                output: scanout.output,
                sequence,
                width,
                height,
                rgba,
            })
        });
        if let Err(error) = delivered {
            cx.fail(error);
        }
    }
    fn offset(&self, cx: &mut Cx<'_>, address: u64, size: u8) -> Option<u64> {
        let offset = address.wrapping_sub(self.base);
        if size != 4 || offset & 3 != 0 || offset >= self.size {
            cx.unmodeled();
            return None;
        }
        Some(offset)
    }
}
impl Device for Output {
    fn name(&self) -> &'static str {
        "xpp-display-output"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(offset) = self.offset(cx, address, size) else {
            return 0;
        };
        if offset == 0 && self.handshake_ticks.is_some() {
            return if self.handshake.status { 1 << 16 } else { 0 };
        }
        let word = self.words.get(&offset).copied().unwrap_or(0);
        let set = self.set.get(&offset).copied().unwrap_or(0);
        let clear = self.clear.get(&offset).copied().unwrap_or(0);
        u64::from((word | set) & !clear)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = self.offset(cx, address, size) else {
            return;
        };
        if offset == 0
            && value <= u32::MAX.into()
            && let Some(ticks) = self.handshake_ticks
        {
            if value & (1 << 16) != 0 {
                self.handshake.status = false;
                self.handshake.due = None;
            } else if self.word(0) & (1 << 16) != 0 {
                self.handshake.due = cx.now().checked_add(ticks);
                if self.handshake.due.is_none() {
                    cx.fail("Display handshake deadline overflow");
                }
            }
        }
        match u32::try_from(value) {
            Ok(0) => {
                self.words.remove(&offset);
            }
            Ok(value) => {
                self.words.insert(offset, value);
            }
            Err(_) => cx.unmodeled(),
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.handshake.due.is_some_and(|at| at <= cx.now()) {
            self.handshake.due = None;
            self.handshake.status = true;
        }

        let Some(scanout) = self.scanout else {
            return;
        };
        let now = cx.now();
        if now < self.capture.next {
            return;
        }
        self.scan(cx, scanout);
        let frames = (now - self.capture.next) / scanout.period + 1;
        match frames
            .checked_mul(scanout.period)
            .and_then(|step| self.capture.next.checked_add(step))
        {
            Some(next) => self.capture.next = next,
            None => cx.fail("Display scanout deadline overflow"),
        }
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.scanout
            .map(|_| self.capture.next)
            .into_iter()
            .chain(self.handshake.due)
            .min()
    }
    fn save(&self) -> Value {
        let words = self
            .words
            .iter()
            .map(|(o, v)| [*o, u64::from(*v)])
            .collect::<Vec<_>>();
        let mut value = json!({ "words": words });
        if self.scanout.is_some() {
            value["scanout"] = json!(self.capture);
        }
        if self.handshake_ticks.is_some() {
            value["handshake"] = json!(self.handshake);
        }
        value
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let rows: Vec<(u64, u32)> = serde_json::from_value(value["words"].clone())
            .map_err(|e| format!("Invalid display output checkpoint: {e}"))?;
        fn field<T: serde::de::DeserializeOwned>(value: &Value, name: &str) -> Result<T, String> {
            serde_json::from_value(value[name].clone())
                .map_err(|e| format!("Invalid display output checkpoint: {e}"))
        }
        let state = OutputState {
            words: rows.into_iter().collect(),
            capture: if self.scanout.is_some() {
                field(value, "scanout")?
            } else {
                Capture::default()
            },
            handshake: if self.handshake_ticks.is_some() {
                field(value, "handshake")?
            } else {
                Handshake::default()
            },
        };
        self.check(&state)?;
        self.apply(state);
        Ok(())
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &OutputState {
                words: self.words.clone(),
                capture: self.capture.clone(),
                handshake: self.handshake.clone(),
            },
        )
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        if !saved.children().is_empty() || !saved.blobs().is_empty() {
            return Err("Display output checkpoint has unexpected parts".into());
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

/// Mutable display output state: words, capture clock and handshake.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputState {
    words: BTreeMap<u64, u32>,
    capture: Capture,
    handshake: Handshake,
}
impl Output {
    fn check(&self, state: &OutputState) -> Result<(), String> {
        if state
            .words
            .iter()
            .any(|(offset, v)| offset & 3 != 0 || *offset >= self.size || *v == 0)
        {
            return Err("Invalid display output checkpoint".into());
        }
        match self.scanout {
            Some(scanout) => {
                if state.capture.next == 0 || !state.capture.next.is_multiple_of(scanout.period) {
                    return Err("Invalid display output checkpoint".into());
                }
            }
            None => {
                if state.capture.next != 0
                    || state.capture.sequence != 0
                    || state.capture.hash.is_some()
                {
                    return Err("Invalid display output checkpoint".into());
                }
            }
        }
        if state.handshake.status && state.handshake.due.is_some()
            || self.handshake_ticks.is_none()
                && (state.handshake.status || state.handshake.due.is_some())
        {
            return Err("Invalid display handshake checkpoint".into());
        }
        Ok(())
    }
    fn apply(&mut self, state: OutputState) {
        self.words = state.words;
        self.capture = state.capture;
        self.handshake = state.handshake;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{Host, Record};

    #[derive(Default)]
    struct Clock {
        now: u64,
        edges: Vec<(u64, u32)>,
    }
    impl Host for Clock {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            unreachable!()
        }
        fn set_irq(&mut self, irq: u32, level: bool) {
            if level {
                self.edges.push((self.now, irq));
            }
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            unreachable!()
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            unreachable!()
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            unreachable!()
        }
        fn record(&mut self, _: Record) {}
    }
    fn advance(clock: &mut FrameClock, host: &mut Clock, now: u64) {
        host.now = now;
        clock.advance(&mut Cx::new(host, 0, 0, 0));
    }

    #[test]
    fn each_period_raises_one_edge_per_line_and_skips_missed_frames() {
        assert!(FrameClock::new(vec![], 10).is_err());
        assert!(FrameClock::new(vec![112, 112], 10).is_err());
        assert!(FrameClock::new(vec![112], 0).is_err());
        let mut clock = FrameClock::new(vec![112, 117], 10).unwrap();
        assert_eq!(clock.next_deadline(0), Some(10));
        let mut host = Clock::default();
        advance(&mut clock, &mut host, 9);
        assert!(host.edges.is_empty());
        advance(&mut clock, &mut host, 35);
        assert_eq!(host.edges, vec![(35, 112), (35, 117)]);
        assert_eq!(clock.next_deadline(35), Some(40));
        let saved = clock.save();
        advance(&mut clock, &mut host, 40);
        assert_eq!(clock.next_deadline(40), Some(50));
        clock.restore(&saved).unwrap();
        assert_eq!(clock.next_deadline(40), Some(40));
        assert!(clock.restore(&json!({ "next": 41 })).is_err());
    }

    #[derive(Default)]
    struct Screen {
        now: u64,
        ram: Vec<u8>,
        frames: Vec<Frame>,
        active: Vec<bool>,
    }
    impl Host for Screen {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            unreachable!()
        }
        fn set_irq(&mut self, _: u32, _: bool) {}
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            unreachable!()
        }
        fn ram_read(&self, address: u64, out: &mut [u8]) -> Result<(), String> {
            let start = address as usize;
            out.copy_from_slice(
                self.ram
                    .get(start..start + out.len())
                    .ok_or("outside RAM")?,
            );
            Ok(())
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            unreachable!()
        }
        fn scanout(&mut self, frame: Frame) -> Result<(), String> {
            self.frames.push(frame);
            Ok(())
        }
        fn output_status(&mut self, _: usize, state: Value) -> Result<(), String> {
            self.active.push(state["active"] == true);
            Ok(())
        }
        fn record(&mut self, _: Record) {}
    }

    #[test]
    fn scanout_composes_premultiplied_osd_layers_over_black() {
        const BASE: u64 = 0xfff64000;
        let scanout = Some(Scanout {
            output: 0,
            period: 10,
        });
        assert!(
            Output::new(
                BASE,
                0x1000,
                &[],
                &[],
                Some(Scanout {
                    output: 0,
                    period: 0
                })
            )
            .is_err()
        );
        let mut out = Output::new(BASE, 0x3000, &[], &[], scanout).unwrap();
        assert_eq!(out.next_deadline(0), Some(10));
        let mut host = Screen {
            ram: vec![0; 0x100],
            ..Screen::default()
        };
        // Two 2 x 1 layers: opaque red then half-covering premultiplied white.
        host.ram[0x40..0x48].copy_from_slice(&[255, 0, 0, 255, 0, 0, 0, 0]);
        host.ram[0x80..0x88].copy_from_slice(&[128, 128, 128, 128, 0, 0, 0, 0]);
        let write = |out: &mut Output, host: &mut Screen, offset: u64, value: u32| {
            out.write(
                &mut Cx::new(host, 0, 0, 0),
                BASE + offset,
                4,
                u64::from(value),
            );
        };
        host.now = 10;
        out.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(
            (host.frames.len(), host.active.as_slice()),
            (0, &[false][..])
        );
        write(&mut out, &mut host, CONTROL, OSD_ENABLE);
        write(&mut out, &mut host, OUTPUT_SIZE, 0x1_0000);
        write(&mut out, &mut host, LAYER_ENABLE, 3);
        for (layer, buffer) in [(0, 0x40), (4, 0x80)] {
            write(&mut out, &mut host, LAYER_SIZE + layer, 0x1_0000);
            write(&mut out, &mut host, LAYER_BUFFER + layer, buffer);
        }
        host.now = 25;
        out.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(out.next_deadline(25), Some(30));
        let frame = host.frames.last().unwrap();
        assert_eq!((frame.width, frame.height, frame.sequence), (2, 1, 1));
        assert_eq!(frame.rgba, [255, 128, 128, 255, 0, 0, 0, 255]);
        let saved = out.save();
        host.now = 30;
        out.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(host.frames.last().unwrap().sequence, 1);
        out.restore(&saved).unwrap();
        assert_eq!(out.next_deadline(30), Some(30));
        // A layer origin away from the corner is not modeled.
        write(&mut out, &mut host, LAYER_ORIGIN, 1);
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        out.advance(&mut cx);
        assert!(cx.is_unmodeled());
    }

    #[test]
    fn scanout_composes_osd_layers_over_the_packed_video_layer() {
        const BASE: u64 = 0xfff64000;
        let scanout = Some(Scanout {
            output: 0,
            period: 10,
        });
        let mut out = Output::new(BASE, 0x3000, &[], &[], scanout).unwrap();
        let mut host = Screen {
            ram: vec![0; 0x100],
            ..Screen::default()
        };
        // A red 2 x 1 video pixel pair, stored blue first like the OSD, under
        // one 2 x 1 OSD layer that is clear over pixel 0 and half white over
        // pixel 1.
        host.ram[0x20..0x24].copy_from_slice(&[81, 81, 90, 240]);
        host.ram[0x80..0x88].copy_from_slice(&[0, 0, 0, 0, 128, 128, 128, 128]);
        let write = |out: &mut Output, host: &mut Screen, offset: u64, value: u32| {
            out.write(
                &mut Cx::new(host, 0, 0, 0),
                BASE + offset,
                4,
                u64::from(value),
            );
        };
        for (offset, value) in [
            (CONTROL, VIDEO_ENABLE | OSD_ENABLE),
            (OUTPUT_SIZE, 0x1_0000),
            (VIDEO_FORMAT, PACKED_422),
            (VIDEO_SIZE, 0x1_0000),
            (VIDEO_BUFFER, 0x20),
        ] {
            write(&mut out, &mut host, offset, value);
        }
        host.now = 10;
        out.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(
            host.frames.last().unwrap().rgba,
            [0, 0, 255, 255, 0, 0, 255, 255]
        );
        write(&mut out, &mut host, LAYER_ENABLE, 1);
        write(&mut out, &mut host, LAYER_SIZE, 0x1_0000);
        write(&mut out, &mut host, LAYER_BUFFER, 0x80);
        host.now = 20;
        out.advance(&mut Cx::new(&mut host, 0, 0, 0));
        let frame = host.frames.last().unwrap();
        assert_eq!(frame.rgba, [0, 0, 255, 255, 128, 128, 255, 255]);
        assert_eq!(frame.sequence, 2);
        // With the OSD group disabled (+0x000 bit 2 clear) the video layer
        // stays: the playback information-off page (PLAYSTART_OSDOFF) writes
        // 0x1010101 there with the photo still programmed as the video layer.
        write(&mut out, &mut host, CONTROL, VIDEO_ENABLE);
        host.now = 30;
        out.advance(&mut Cx::new(&mut host, 0, 0, 0));
        let frame = host.frames.last().unwrap();
        assert_eq!(frame.rgba, [0, 0, 255, 255, 0, 0, 255, 255]);
        // Only the packed 4:2:2 format is modeled.
        write(&mut out, &mut host, CONTROL, VIDEO_ENABLE | OSD_ENABLE);
        write(&mut out, &mut host, VIDEO_FORMAT, 0x222);
        host.now = 40;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        out.advance(&mut cx);
        assert!(cx.is_unmodeled());
    }

    #[test]
    fn an_enabled_video_path_without_a_committed_plane_scans_the_osd_layers() {
        const BASE: u64 = 0xfff64000;
        let scanout = Some(Scanout {
            output: 0,
            period: 10,
        });
        let mut out = Output::new(BASE, 0x3000, &[], &[], scanout).unwrap();
        let mut host = Screen {
            ram: vec![0; 0x100],
            ..Screen::default()
        };
        // One 2 x 1 opaque red OSD layer.
        host.ram[0x40..0x48].copy_from_slice(&[255, 0, 0, 255, 0, 0, 0, 0]);
        let write = |out: &mut Output, host: &mut Screen, offset: u64, value: u32| {
            out.write(
                &mut Cx::new(host, 0, 0, 0),
                BASE + offset,
                4,
                u64::from(value),
            );
        };
        // The update UI's commit: video on, size and buffer still zero.
        for (offset, value) in [
            (CONTROL, 0x101_0104),
            (OUTPUT_SIZE, 0x1_0000),
            (VIDEO_FORMAT, PACKED_422),
            (VIDEO_SIZE, 0),
            (VIDEO_BUFFER, 0),
            (LAYER_ENABLE, 1),
            (LAYER_SIZE, 0x1_0000),
            (LAYER_BUFFER, 0x40),
        ] {
            write(&mut out, &mut host, offset, value);
        }
        host.now = 10;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        out.advance(&mut cx);
        assert!(!cx.is_unmodeled());
        assert_eq!(
            host.frames.last().unwrap().rgba,
            [255, 0, 0, 255, 0, 0, 0, 255]
        );
    }

    #[test]
    fn planes_larger_than_the_output_show_their_top_left_part() {
        // The X-T20 2.11 VIEW MODE switch back from the EVF: the rear-screen
        // timing is written while both planes still hold the larger EVF sizes,
        // and a frame is scanned before the next commit shrinks them.
        const BASE: u64 = 0xfff64000;
        let scanout = Some(Scanout {
            output: 0,
            period: 10,
        });
        let mut out = Output::new(BASE, 0x3000, &[], &[], scanout).unwrap();
        let mut host = Screen {
            ram: vec![0; 0x100],
            ..Screen::default()
        };
        // A 3 x 2 OSD layer: opaque red, transparent, then pixels outside 2 x 1.
        host.ram[0x40..0x4c].copy_from_slice(&[255, 0, 0, 255, 0, 0, 0, 0, 9, 9, 9, 255]);
        host.ram[0x4c..0x58].fill(9);
        // A 4 x 2 packed 4:2:2 video plane, every pair the same blue.
        for row in host.ram[0x80..0x90].as_chunks_mut::<4>().0 {
            row.copy_from_slice(&[81, 81, 90, 240]);
        }
        let write = |out: &mut Output, host: &mut Screen, offset: u64, value: u32| {
            out.write(
                &mut Cx::new(host, 0, 0, 0),
                BASE + offset,
                4,
                u64::from(value),
            );
        };
        for (offset, value) in [
            (CONTROL, VIDEO_ENABLE | OSD_ENABLE),
            (OUTPUT_SIZE, 0x1_0000),
            (VIDEO_FORMAT, PACKED_422),
            (VIDEO_SIZE, 0x3_0001),
            (VIDEO_BUFFER, 0x80),
            (LAYER_ENABLE, 1),
            (LAYER_SIZE, 0x2_0001),
            (LAYER_BUFFER, 0x40),
        ] {
            write(&mut out, &mut host, offset, value);
        }
        host.now = 10;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        out.advance(&mut cx);
        assert!(!cx.is_unmodeled());
        let frame = host.frames.last().unwrap();
        assert_eq!((frame.width, frame.height), (2, 1));
        assert_eq!(frame.rgba, [255, 0, 0, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn the_playback_information_off_output_composes_the_video_without_the_osd_group() {
        // The X-T20 2.11 playback information-off page: +0x000 = 0x1010101
        // (bit 0 output and video, bit 2 clear = OSD group off), the decoded
        // photo in the packed 4:2:2 video plane, and an OSD layer buffer that
        // still holds pixels from the previous page.
        const BASE: u64 = 0xfff64000;
        let scanout = Some(Scanout {
            output: 0,
            period: 10,
        });
        let mut out = Output::new(BASE, 0x3000, &[], &[], scanout).unwrap();
        let mut host = Screen {
            ram: vec![0; 0x100],
            ..Screen::default()
        };
        host.ram[0x20..0x24].copy_from_slice(&[81, 81, 90, 240]);
        host.ram[0x80..0x88].copy_from_slice(&[255, 0, 0, 255, 255, 0, 0, 255]);
        let write = |out: &mut Output, host: &mut Screen, offset: u64, value: u32| {
            out.write(
                &mut Cx::new(host, 0, 0, 0),
                BASE + offset,
                4,
                u64::from(value),
            );
        };
        for (offset, value) in [
            (CONTROL, 0x1010101 & 0xf),
            (OUTPUT_SIZE, 0x1_0000),
            (VIDEO_FORMAT, PACKED_422),
            (VIDEO_SIZE, 0x1_0000),
            (VIDEO_BUFFER, 0x20),
            (LAYER_ENABLE, 1),
            (LAYER_SIZE, 0x1_0000),
            (LAYER_BUFFER, 0x80),
        ] {
            write(&mut out, &mut host, offset, value);
        }
        host.now = 10;
        out.advance(&mut Cx::new(&mut host, 0, 0, 0));
        let frame = host.frames.last().unwrap();
        assert_eq!(frame.rgba, [0, 0, 255, 255, 0, 0, 255, 255]);
        // The same words with the OSD group on composite the stale layer.
        write(&mut out, &mut host, CONTROL, 0x1010105 & 0xf);
        host.now = 20;
        out.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(
            host.frames.last().unwrap().rgba,
            [255, 0, 0, 255, 255, 0, 0, 255]
        );
        // The firmware update page: OSD group on, video plane off and not
        // programmed; the layer composes over black.
        for (offset, value) in [(CONTROL, 0x1010104), (VIDEO_SIZE, 0), (VIDEO_BUFFER, 0)] {
            write(&mut out, &mut host, offset, value);
        }
        host.now = 30;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        out.advance(&mut cx);
        assert!(!cx.is_unmodeled());
        assert_eq!(
            host.frames.last().unwrap().rgba,
            [255, 0, 0, 255, 255, 0, 0, 255]
        );
    }

    #[test]
    fn outputs_read_back_words_with_declared_status_bits() {
        use fujimod_emulator_devices::test_host::{TestHost, read, write};
        const BASE: u64 = 0xfff64000;
        assert!(Output::new(BASE, 0x3000, &[(0, 0x10000)], &[(0, 0x10000)], None).is_err());
        assert!(Output::new(BASE, 0x3000, &[(0x3000, 1)], &[], None).is_err());
        let mut out =
            Output::new(BASE, 0x3000, &[(0, 0x10000)], &[(0x2000, 1 << 28)], None).unwrap();
        let mut h = TestHost::new();
        assert_eq!(read(&mut out, &mut h, BASE), 0x10000);
        write(&mut out, &mut h, BASE, 0x10001);
        write(&mut out, &mut h, BASE, 1);
        assert_eq!(read(&mut out, &mut h, BASE), 0x10001);
        write(&mut out, &mut h, BASE + 0x2000, 0x1002_5000);
        assert_eq!(read(&mut out, &mut h, BASE + 0x2000), 0x25000);
        write(&mut out, &mut h, BASE + 0x1004, 0x1c1044b);
        let saved = out.save();
        write(&mut out, &mut h, BASE + 0x1004, 0);
        assert_eq!(read(&mut out, &mut h, BASE + 0x1004), 0);
        out.restore(&saved).unwrap();
        assert_eq!(read(&mut out, &mut h, BASE + 0x1004), 0x1c1044b);
        let mut cx = h.cx();
        out.read(&mut cx, BASE + 2, 2);
        assert!(cx.is_unmodeled());
    }

    #[test]
    fn clearing_bit16_reports_status_after_delay_until_set_again() {
        use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};
        let mut h = TestHost::new();
        let mut d = Output::new(0xfff64000, 0x1000, &[], &[], None)
            .unwrap()
            .with_handshake(8)
            .unwrap();
        write(&mut d, &mut h, 0xfff64000, 0x10000);
        write(&mut d, &mut h, 0xfff64000, 0);
        assert_eq!(read(&mut d, &mut h, 0xfff64000), 0);
        let pending = d.save();
        assert_eq!(d.next_deadline(0), Some(8));
        advance(&mut d, &mut h, 7);
        assert_eq!(read(&mut d, &mut h, 0xfff64000), 0);
        advance(&mut d, &mut h, 8);
        assert_eq!(read(&mut d, &mut h, 0xfff64000), 0x10000);
        d.restore(&pending).unwrap();
        assert_eq!(read(&mut d, &mut h, 0xfff64000), 0);
        assert_eq!(d.next_deadline(8), Some(8));
        advance(&mut d, &mut h, 8);
        assert_eq!(read(&mut d, &mut h, 0xfff64000), 0x10000);
        write(&mut d, &mut h, 0xfff64010, 0x33);
        assert_eq!(read(&mut d, &mut h, 0xfff64010), 0x33);
        write(&mut d, &mut h, 0xfff64000, 0x10020);
        assert_eq!(read(&mut d, &mut h, 0xfff64000), 0);
    }
}
