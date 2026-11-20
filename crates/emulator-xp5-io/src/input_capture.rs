//! XP5 input-capture banks.
//! Without a declared sequencer only disabled/reset and idle readback are
//! supported: active capture configuration refuses and no samples, cause bits
//! or deadlines are invented.
//!
//! With a declared sequencer, one channel runs the companion's position-sensor
//! word stream (X-T5 4.31: capture channel 2, `fa234adc -> ff928044`). The
//! companion core DMAs a 41-word table to the channel's transmit register on
//! every IBIS servo tick (M-core `0x3cd4`, DMA handle `0x4000d`), takes NVIC
//! line 62 (descriptor `0x31fb4`), acknowledges `+0xc & 0x10001` and reads one
//! 32-bit capture from `+0x10` per interrupt (handler `0x31e8`). The first
//! capture carries two 16-bit samples (high, low), the second carries one in
//! its low half; the servo (`0x284cc`) uses them as the three raw stage
//! position samples. Word meanings below are inferred from that table, not
//! from a datasheet:
//! - `0xc6c1xxxx` sends one 16-bit word to the sensor (starts a sample frame),
//! - `0xc6c2xxxx` clocks one 16-bit sample in (shifted into the capture word);
//!   the first one after a sent word reads axis 0, then axes 1 and 2,
//! - `0x0007xxxx` ends a segment: the capture word is latched after
//!   `latency_ns` and the channel interrupt is raised,
//! - every other word (pin and delay steps) is accepted without effect.
//!
//! The sensor is a stage-position model: each axis follows its commanded
//! position through a first-order lag (`position += (command - position) >>
//! lag_shift` per sample frame, at least one count) and reads back as `center - position`,
//! clamped to 16 bits. The command is the rest position from the wiring plus,
//! when `drive_channels` is declared, `drive_gain` times each axis coil's PWM
//! drive: the extended timer channel's second compare minus its first (X-T5
//! 4.31: period 1744, first compare 872, so 872 is zero drive). This is a
//! spring-like stage: a constant coil drive holds a constant displacement.
use fujimod_emulator_devices::bus::InterruptLine;
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::{dma::{self, Direction}, Cx, Device};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Most captures kept latched or in flight; a guest that never reads them refuses.
const CAPTURE_DEPTH: usize = 16;
/// A sample frame this long after the previous one starts a new settle window
/// (`settle_frames`). The X-T5 4.31 A53 reloads and restarts the companion core
/// at about 24.9 s; its servo then starts again after a pause far longer than
/// this, while the running servo samples every 62.5 us.
const SETTLE_GAP_NS: u64 = 50_000_000;

/// Immediate-acceptance transmit port for a channel without a sequencer.
pub struct TransmitSink;
impl dma::Port for TransmitSink {
    fn request(&self, direction: Direction, _burst: bool) -> Option<u64> {
        (direction == Direction::Transmit).then_some(0)
    }
    fn receive(&mut self, _now: u64, _bytes: u8) -> Result<u64, String> {
        Err("input-capture transmit sink cannot receive".into())
    }
    fn transmit(&mut self, _now: u64, _bytes: u8, _value: u64) -> Result<(), String> {
        // Immediate acceptance with no downstream effect is a session assumption.
        Ok(())
    }
}

/// Wiring of a position-sensor sequencer channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SequencerOptions {
    /// Capture channel whose transmit register receives the word stream.
    pub channel: u8,
    /// Companion NVIC line raised while a capture is latched.
    pub companion_irq_line: u32,
    /// Delay from the segment-end word to the latched capture.
    pub latency_ns: u64,
    /// Raw sample of each axis at stage position zero.
    pub center: [u16; 3],
    /// Commanded stage position of each axis, in raw counts from center.
    pub rest: [i32; 3],
    /// First-order lag of the stage: `position += (command - position) >> lag_shift`.
    pub lag_shift: u8,
    /// Extended-timer general channel whose PWM drives each axis's coil.
    #[serde(default)]
    pub drive_channels: Option<[u8; 3]>,
    /// Commanded displacement (raw counts) per PWM count of drive, per axis.
    #[serde(default)]
    pub drive_gain: [i32; 3],
    /// Host-time saving scenario: after this many sample frames the sensor stops
    /// answering (segment ends latch no capture) until a pause of at least 50 ms
    /// in the frames (a companion restart) opens a new window. The companion servo task then
    /// waits on its sample semaphore with its last, settled errors, as it did
    /// before this model existed. Not a hardware behavior; absent = always answer.
    #[serde(default)]
    pub settle_frames: Option<u64>,
}
/// The settle limit in force: the manifest option, or the host override
/// `XT5_IBIS_SETTLE_FRAMES=N` for quick development runs (read once). The override
/// is a host-time saving only, never hardware behavior; strict records leave it unset.
fn settle_limit(configured: Option<u64>) -> Option<u64> {
    static OVERRIDE: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    configured.or(*OVERRIDE.get_or_init(|| {
        std::env::var("XT5_IBIS_SETTLE_FRAMES").ok().and_then(|value| value.parse().ok())
    }))
}

impl SequencerOptions {
    fn validate(&self, channels: u64) -> Result<(), String> {
        if u64::from(self.channel) >= channels
            || self.latency_ns == 0
            || self.lag_shift > 15
            || self.drive_channels.is_some_and(|c| c.iter().any(|&n| n >= 6))
            || self.drive_channels.is_none() && self.drive_gain != [0; 3]
            || self.drive_gain.iter().any(|g| g.unsigned_abs() > 0x100)
        {
            return Err("invalid input-capture sequencer wiring".into());
        }
        if self.rest.iter().any(|r| r.unsigned_abs() > 0x8000) {
            return Err("input-capture sequencer rest position exceeds the sample range".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SequencerState {
    /// Samples shifted in since the last segment end.
    shift: u32,
    /// Samples read since the last command word (selects the axis).
    reads: u8,
    /// Last 16-bit word sent to the sensor.
    command: Option<u16>,
    /// Latched captures waiting for their latency: (due tick, value).
    in_flight: VecDeque<(u64, u32)>,
    /// Captures visible at `+0x10`, oldest first.
    ready: VecDeque<u32>,
    /// Stage position of each axis in raw counts from center.
    position: [i32; 3],
    irq_published: bool,
    words: u64,
    frames: u64,
    captures: u64,
    /// Frames since the current settle window opened, and the last frame's tick.
    #[serde(default)]
    window_frames: u64,
    #[serde(default)]
    last_frame: Option<u64>,
}

/// The sequencer state shared by the channel registers and its DMA port.
pub struct Sequencer {
    options: SequencerOptions,
    state: SequencerState,
    pwm: Option<crate::timer::PwmOutputs>,
}
impl Sequencer {
    pub fn new(
        options: SequencerOptions,
        channels: u64,
        pwm: Option<crate::timer::PwmOutputs>,
    ) -> Result<Self, String> {
        options.validate(channels)?;
        if options.drive_channels.is_some() && pwm.is_none() {
            return Err("input-capture sequencer drive needs the extended timer outputs".into());
        }
        Ok(Self {
            state: SequencerState { position: options.rest, ..Default::default() },
            options,
            pwm,
        })
    }
    fn command(&self, axis: usize) -> i32 {
        let drive = match (&self.options.drive_channels, &self.pwm) {
            (Some(channels), Some(pwm)) => {
                let words = pwm.lock().unwrap()[usize::from(channels[axis])];
                words[3] as i64 - words[2] as i64
            }
            _ => 0,
        };
        (self.options.rest[axis] as i64 + self.options.drive_gain[axis] as i64 * drive)
            .clamp(-0x8000, 0x8000) as i32
    }
    fn sample(&self, axis: usize) -> u16 {
        (i32::from(self.options.center[axis]) - self.state.position[axis]).clamp(0, 0xffff) as u16
    }
    fn step_stage(&mut self) {
        for axis in 0..3 {
            let error = self.command(axis) - self.state.position[axis];
            // Round away from zero so the stage settles exactly on its command.
            let step = error >> self.options.lag_shift;
            self.state.position[axis] += if step == 0 { error.signum() } else { step };
        }
    }
    /// One word from the companion's DMA stream.
    fn word(&mut self, now: u64, word: u32) -> Result<(), String> {
        self.state.words += 1;
        match word >> 16 {
            0xc6c1 => {
                self.state.command = Some(word as u16);
                self.state.reads = 0;
            }
            0xc6c2 => {
                let axis = usize::from(self.state.reads);
                if axis >= 3 {
                    return Err("position sensor read past its three axes".into());
                }
                if axis == 0 {
                    // One stage step per sample frame, before its first sample.
                    self.step_stage();
                    self.state.frames += 1;
                    if self.state.last_frame.is_some_and(|at| now.saturating_sub(at) >= SETTLE_GAP_NS) {
                        self.state.window_frames = 0;
                    }
                    self.state.window_frames += 1;
                    self.state.last_frame = Some(now);
                }
                self.state.reads += 1;
                self.state.shift = (self.state.shift << 16) | u32::from(self.sample(axis));
            }
            0x0007 if settle_limit(self.options.settle_frames).is_some_and(|limit| self.state.window_frames > limit) => {
                self.state.shift = 0;
            }
            0x0007 => {
                if self.state.in_flight.len() + self.state.ready.len() >= CAPTURE_DEPTH {
                    return Err("input-capture sequencer captures were not read".into());
                }
                let due = now
                    .checked_add(self.options.latency_ns)
                    .ok_or("input-capture sequencer deadline overflow")?;
                self.state.in_flight.push_back((due, std::mem::take(&mut self.state.shift)));
            }
            _ => {}
        }
        Ok(())
    }
    fn advance(&mut self, now: u64) {
        while self.state.in_flight.front().is_some_and(|&(due, _)| due <= now) {
            let (_, value) = self.state.in_flight.pop_front().unwrap();
            self.state.ready.push_back(value);
            self.state.captures += 1;
        }
    }
    fn next(&self) -> Option<u64> {
        self.state.in_flight.front().map(|&(due, _)| due)
    }
    fn validate_state(&self, state: &SequencerState) -> Result<(), String> {
        if state.reads > 3
            || state.in_flight.len() + state.ready.len() > CAPTURE_DEPTH
            || state.in_flight.iter().zip(state.in_flight.iter().skip(1)).any(|(a, b)| a.0 > b.0)
            || state.irq_published != (!state.ready.is_empty() && state.irq_published)
            || state.position.iter().any(|p| p.unsigned_abs() > 0x8000)
        {
            return Err("invalid input-capture sequencer checkpoint".into());
        }
        Ok(())
    }
}

/// DMA transmit port of a sequencer channel.
struct SequencerPort(Arc<Mutex<Sequencer>>);
impl dma::Port for SequencerPort {
    fn request(&self, direction: Direction, _burst: bool) -> Option<u64> {
        (direction == Direction::Transmit).then_some(0)
    }
    fn receive(&mut self, _now: u64, _bytes: u8) -> Result<u64, String> {
        Err("input-capture sequencer cannot receive".into())
    }
    fn transmit(&mut self, now: u64, bytes: u8, value: u64) -> Result<(), String> {
        if bytes != 4 || value >> 32 != 0 {
            return Err("input-capture sequencer takes 32-bit words".into());
        }
        self.0
            .lock()
            .map_err(|_| "input-capture sequencer poisoned")?
            .word(now, value as u32)
    }
}

struct Channel {
    options: SequencerOptions,
    engine: Arc<Mutex<Sequencer>>,
    interrupt: InterruptLine,
}

/// Bank control word; the firmware addresses bits 0..7 of it by single-bit
/// read-modify-write. The model stores and returns them without attaching any effect.
const CONTROL: u64 = 0xa0;
const CONTROL_BITS: u32 = 0xff;

pub struct InputCapture {
    base: u64,
    channels: u64,
    /// +0xa0. Reset value zero is assumed, like the per-channel words.
    control: u32,
    sequencer: Option<Channel>,
}
impl InputCapture {
    pub fn new(base: u64) -> Result<Self, String> {
        let channels = Self::channel_count(base)?;
        Ok(Self { base, channels, control: 0, sequencer: None })
    }
    pub fn channel_count(base: u64) -> Result<u64, String> {
        match base {
            0xff928000 => Ok(5),
            0xff929000 => Ok(3),
            _ => Err("invalid input-capture bank".into()),
        }
    }
    /// A bank whose `options.channel` runs the position-sensor sequencer on companion `core`.
    pub fn with_sequencer(
        base: u64,
        options: SequencerOptions,
        core: usize,
        pwm: Option<crate::timer::PwmOutputs>,
    ) -> Result<Self, String> {
        let channels = Self::channel_count(base)?;
        let engine = Arc::new(Mutex::new(Sequencer::new(options.clone(), channels, pwm)?));
        Ok(Self {
            base,
            channels,
            control: 0,
            sequencer: Some(Channel {
                interrupt: InterruptLine::Core { core, line: options.companion_irq_line },
                options,
                engine,
            }),
        })
    }
    /// The DMA transmit port of `channel`: the sequencer for its channel, otherwise a sink.
    pub fn transmit_port(&self, channel: u8) -> dma::SharedPort {
        match &self.sequencer {
            Some(s) if s.options.channel == channel => Arc::new(Mutex::new(SequencerPort(s.engine.clone()))),
            _ => Arc::new(Mutex::new(TransmitSink)),
        }
    }
    fn sequencer_offset(&self, address: u64) -> Option<u64> {
        let s = self.sequencer.as_ref()?;
        let offset = address.wrapping_sub(self.base);
        (offset / 0x20 == u64::from(s.options.channel)).then_some(offset % 0x20)
    }
    fn readable(&self, address: u64, size: u8) -> bool {
        let offset = address.wrapping_sub(self.base);
        size == 4
            && (offset == CONTROL
                || offset / 0x20 < self.channels && matches!(offset % 0x20, 0 | 4 | 0xc | 0x10))
    }
    fn validate_control(control: u32) -> Result<(), String> {
        if control & !CONTROL_BITS != 0 {
            return Err("input-capture control has bits above 7".into());
        }
        Ok(())
    }
    fn publish(&self, cx: &mut Cx<'_>) {
        let Some(s) = &self.sequencer else { return };
        let mut e = s.engine.lock().unwrap();
        e.advance(cx.now());
        let high = !e.state.ready.is_empty();
        if high != e.state.irq_published {
            e.state.irq_published = high;
            if let Err(error) = s.interrupt.drive(cx, high) {
                cx.fail(error);
            }
        }
    }
}
impl Device for InputCapture {
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        match &self.sequencer {
            None => Checkpoint::encode(self.name(), &self.control),
            Some(s) => {
                let e = s.engine.lock().map_err(|_| "input-capture sequencer poisoned")?;
                Checkpoint::encode(self.name(), &(&s.options, &e.state, self.control))
            }
        }
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        match &self.sequencer {
            None => Self::validate_control(saved.decode(self.name())?),
            Some(s) => {
                let (options, state, control): (SequencerOptions, SequencerState, u32) =
                    saved.decode(self.name())?;
                if options != s.options {
                    return Err("input-capture sequencer wiring differs".into());
                }
                Self::validate_control(control)?;
                s.engine.lock().map_err(|_| "input-capture sequencer poisoned")?.validate_state(&state)
            }
        }
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        match &self.sequencer {
            None => self.control = saved.decode(self.name())?,
            Some(s) => {
                let (_, state, control): (SequencerOptions, SequencerState, u32) =
                    saved.decode(self.name())?;
                s.engine.lock().map_err(|_| "input-capture sequencer poisoned")?.state = state;
                self.control = control;
            }
        }
        Ok(())
    }
    fn name(&self) -> &'static str {
        "input-capture"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if !self.readable(address, size) {
            cx.unmodeled();
            return 0;
        }
        if address.wrapping_sub(self.base) == CONTROL {
            return u64::from(self.control);
        }
        self.publish(cx);
        let value = match (self.sequencer_offset(address), &self.sequencer) {
            (Some(0xc), Some(s)) => u64::from(!s.engine.lock().unwrap().state.ready.is_empty()),
            // Reading the capture consumes it; the next one (if any) keeps the line high.
            (Some(0x10), Some(s)) => u64::from(s.engine.lock().unwrap().state.ready.pop_front().unwrap_or(0)),
            _ => 0,
        };
        self.publish(cx);
        value
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(self.base);
        if size == 4 && offset == CONTROL && value & !u64::from(CONTROL_BITS) == 0 {
            self.control = value as u32;
            return;
        }
        if size != 4 || offset / 0x20 >= self.channels {
            cx.unmodeled();
            return;
        }
        match offset % 0x20 {
            0 if value == 0 => {}
            // Status acknowledge: the capture FIFO, not this write, clears the line.
            0xc if value & !0x10001 == 0 => {}
            _ => cx.unmodeled(),
        }
        self.publish(cx);
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        self.publish(cx);
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.sequencer.as_ref().and_then(|s| s.engine.lock().unwrap().next())
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        self.readable(address, size) && self.sequencer_offset(address) != Some(0x10)
    }
    fn save(&self) -> serde_json::Value {
        match &self.sequencer {
            None => serde_json::json!({"control": self.control}),
            Some(s) => {
                let e = s.engine.lock().unwrap();
                serde_json::json!({"options": s.options, "state": e.state, "control": self.control})
            }
        }
    }
    fn restore(&mut self, state: &serde_json::Value) -> Result<(), String> {
        let control = state
            .get("control")
            .and_then(serde_json::Value::as_u64)
            .and_then(|control| u32::try_from(control).ok())
            .ok_or("invalid input-capture state")?;
        Self::validate_control(control)?;
        self.control = control;
        match &self.sequencer {
            None => Ok(()),
            Some(s) => {
                if state["options"] != serde_json::json!(s.options) {
                    return Err("input-capture sequencer wiring differs".into());
                }
                let saved: SequencerState =
                    serde_json::from_value(state["state"].clone()).map_err(|e| e.to_string())?;
                let mut e = s.engine.lock().map_err(|_| "input-capture sequencer poisoned")?;
                e.validate_state(&saved)?;
                e.state = saved;
                Ok(())
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::dma::Port;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    #[test]
    fn bank_bounds_idle_readback_and_active_capture_refusal() {
        let mut host = RecordingHost::default();
        for (base, channels) in [(0xff928000, 5), (0xff929000, 3)] {
            let mut capture = InputCapture::new(base).unwrap();
            for channel in 0..channels {
                let mut cx = Cx::new(&mut host, 4, 0, 0);
                let at = base + channel * 0x20;
                capture.write(&mut cx, at, 4, 0);
                capture.write(&mut cx, at + 0xc, 4, 0x10001);
                for offset in [0, 4, 0xc, 0x10] {
                    assert_eq!(capture.read(&mut cx, at + offset, 4), 0);
                }
                assert_eq!(cx.refusal(), None);
            }
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            // Bank 0's first word past its channels is the control word.
            capture.read(&mut cx, base + channels * 0x20 + 4, 4);
            assert!(cx.is_unmodeled());
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            capture.write(&mut cx, base, 4, 1);
            assert!(cx.is_unmodeled());
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            capture.read(&mut cx, base, 8);
            assert!(cx.is_unmodeled());
            assert_eq!(capture.next_deadline(0), None);
            assert_eq!(capture.save(), serde_json::json!({ "control": 0 }));
        }
        assert!(host.events.is_empty());
        assert!(host.irqs.is_empty());
    }

    #[test]
    fn bank_control_bits_read_back_and_checkpoint() {
        let mut host = RecordingHost::default();
        for base in [0xff928000, 0xff929000] {
            let mut capture = InputCapture::new(base).unwrap();
            let control = base + CONTROL;
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            assert_eq!(capture.read(&mut cx, control, 4), 0);
            // Set bit 7 and bit 0, then clear bit 7, as single-bit
            // read-modify-writes.
            for bit in [7, 0] {
                let value = capture.read(&mut cx, control, 4) | 1 << bit;
                capture.write(&mut cx, control, 4, value);
            }
            let value = capture.read(&mut cx, control, 4) & !(1 << 7);
            capture.write(&mut cx, control, 4, value);
            assert_eq!(capture.read(&mut cx, control, 4), 1);
            assert!(capture.pollable(control, 4));
            assert_eq!(cx.refusal(), None);
            assert!(!cx.is_unmodeled());

            let saved = capture.checkpoint().unwrap();
            let mut restored = InputCapture::new(base).unwrap();
            restored.restore_checkpoint(&saved).unwrap();
            assert_eq!(restored.read(&mut cx, control, 4), 1);
            assert_eq!(capture.save(), serde_json::json!({ "control": 1 }));
            restored.restore(&serde_json::json!({ "control": 0x80 })).unwrap();
            assert_eq!(restored.read(&mut cx, control, 4), 0x80);
            assert!(restored.restore(&serde_json::json!({ "control": 0x100 })).is_err());

            for (address, size, value) in
                [(control, 4, 0x100), (control, 2, 1), (control + 4, 4, 0), (control - 4, 4, 0)]
            {
                let mut cx = Cx::new(&mut host, 0, 0, 0);
                capture.write(&mut cx, address, size, value);
                assert!(cx.is_unmodeled(), "{address:#x}/{size}={value:#x}");
            }
            for (address, size) in [(control, 8), (control + 4, 4)] {
                let mut cx = Cx::new(&mut host, 0, 0, 0);
                capture.read(&mut cx, address, size);
                assert!(cx.is_unmodeled(), "{address:#x}/{size}");
            }
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            assert_eq!(capture.read(&mut cx, control, 4), 1);
        }
        assert!(host.events.is_empty());
        assert!(host.irqs.is_empty());
    }

    /// The X-T5 4.31 companion's per-tick table (M-core RAM `0x34adc`, 41 words).
    const TABLE: [u32; 41] = [
        0xc00e0000, 0x0004002a, 0xc0060000, 0xe0060000, 0x00040027, 0xc6c1e5dc, 0xe00e0000,
        0x00040001, 0xc00e0000, 0x0004002a, 0xc0060000, 0x00040027, 0xc6c1ffff, 0x00040001,
        0xc00e0000, 0x0004002a, 0xc0060000, 0x00040027, 0xc6c2ffff, 0x00040001, 0xc00e0000,
        0x0004002a, 0xc0060000, 0x00040027, 0xc6c2ffff, 0x00070000, 0xc0060000, 0xc0060000,
        0xc0060000, 0xc0060000, 0xc0060000, 0xc0060000, 0xc0060000, 0xc0060000, 0xc0060000,
        0x00040018, 0x00040027, 0xc6c2ffff, 0x00040001, 0xc00e0000, 0x00070000,
    ];
    const BASE: u64 = 0xff928000;
    const CHANNEL: u64 = BASE + 2 * 0x20;

    fn options(rest: [i32; 3], lag_shift: u8) -> SequencerOptions {
        SequencerOptions {
            channel: 2,
            companion_irq_line: 62,
            latency_ns: 1000,
            center: [0x8000, 0x8000, 0x8000],
            rest,
            lag_shift,
            drive_channels: None,
            drive_gain: [0; 3],
            settle_frames: None,
        }
    }

    fn stream(port: &dma::SharedPort, now: u64) {
        let mut port = port.lock().unwrap();
        for word in TABLE {
            port.transmit(now, 4, word.into()).unwrap();
        }
    }

    /// The companion handler: acknowledge, then read one capture.
    fn service(capture: &mut InputCapture, host: &mut RecordingHost) -> u32 {
        let mut cx = Cx::new(host, 4, 0, 0);
        let status = capture.read(&mut cx, CHANNEL + 0xc, 4);
        capture.write(&mut cx, CHANNEL + 0xc, 4, status & 0x10001);
        let value = capture.read(&mut cx, CHANNEL + 0x10, 4) as u32;
        capture.write(&mut cx, CHANNEL, 4, 0);
        assert_eq!(cx.refusal(), None);
        assert!(!cx.is_unmodeled());
        value
    }

    #[test]
    fn table_yields_two_captures_with_three_centered_samples() {
        let mut host = RecordingHost::default();
        let mut capture = InputCapture::with_sequencer(BASE, options([0; 3], 2), 4, None).unwrap();
        let port = capture.transmit_port(2);
        stream(&port, 100);
        assert_eq!(capture.next_deadline(100), Some(1100));
        host.now = 1100;
        capture.advance(&mut Cx::new(&mut host, 4, 0, 0));
        assert_eq!(host.core_irqs, vec![(4, 62, true)]);
        assert_eq!(service(&mut capture, &mut host), 0x8000_8000);
        // The second capture keeps the line high.
        assert_eq!(host.core_irqs, vec![(4, 62, true)]);
        assert_eq!(service(&mut capture, &mut host) & 0xffff, 0x8000);
        assert_eq!(host.core_irqs, vec![(4, 62, true), (4, 62, false)]);
        assert_eq!(capture.next_deadline(1100), None);
    }

    #[test]
    fn stage_follows_its_command_through_the_lag() {
        let mut host = RecordingHost::default();
        let mut sequencer = Sequencer::new(options([800, -400, 0], 1), 5, None).unwrap();
        sequencer.state.position = [0; 3];
        let mut seen = vec![];
        for tick in 0..20u64 {
            for word in TABLE {
                sequencer.word(tick * 10, word).unwrap();
            }
            sequencer.advance(tick * 10 + 1000);
            let first = sequencer.state.ready.pop_front().unwrap();
            sequencer.state.ready.pop_front().unwrap();
            seen.push(((first >> 16) as i32 - 0x8000, (first & 0xffff) as i32 - 0x8000));
        }
        // Samples read back as center - position and converge on the command.
        assert_eq!(seen[0], (-400, 200));
        assert!(seen.windows(2).all(|w| w[1].0 <= w[0].0 && w[1].1 >= w[0].1));
        assert_eq!(*seen.last().unwrap(), (-800, 400));
        let _ = &mut host;
    }

    #[test]
    fn stage_displacement_follows_the_coil_pwm_drive() {
        let pwm: crate::timer::PwmOutputs = Default::default();
        let mut wiring = options([0; 3], 0);
        wiring.drive_channels = Some([2, 4, 0]);
        wiring.drive_gain = [16, -8, 4];
        assert!(Sequencer::new(wiring.clone(), 5, None).is_err());
        let mut sequencer = Sequencer::new(wiring, 5, Some(pwm.clone())).unwrap();
        let frame = |sequencer: &mut Sequencer| {
            for word in TABLE {
                sequencer.word(0, word).unwrap();
            }
            sequencer.advance(1000);
            let first = sequencer.state.ready.pop_front().unwrap();
            let second = sequencer.state.ready.pop_front().unwrap();
            [(first >> 16) as u16, first as u16, second as u16]
        };
        // Zero drive: second compare equals the first.
        for n in [0, 2, 4] {
            pwm.lock().unwrap()[n] = [0, 1744, 872, 872];
        }
        assert_eq!(frame(&mut sequencer), [0x8000; 3]);
        pwm.lock().unwrap()[2][3] = 882; // +10 counts on axis 0
        pwm.lock().unwrap()[4][3] = 862; // -10 counts on axis 1
        pwm.lock().unwrap()[0][3] = 1743;
        assert_eq!(frame(&mut sequencer), [0x8000 - 160, 0x8000 - 80, 0x8000 - 3484]);
    }

    #[test]
    fn settled_sensor_stops_answering_after_its_frame_limit() {
        let mut wiring = options([0; 3], 2);
        wiring.settle_frames = Some(2);
        let mut sequencer = Sequencer::new(wiring, 5, None).unwrap();
        let mut captures = vec![];
        for tick in 0..4u64 {
            for word in TABLE {
                sequencer.word(tick * 100, word).unwrap();
            }
            sequencer.advance(tick * 100 + 1000);
            captures.push(sequencer.state.ready.drain(..).count());
        }
        assert_eq!(captures, [2, 2, 0, 0]);
        assert_eq!(sequencer.next(), None);
        // After a companion restart (a long pause) the sensor answers again.
        for tick in 0..3u64 {
            let at = SETTLE_GAP_NS + 400 + tick * 100;
            for word in TABLE {
                sequencer.word(at, word).unwrap();
            }
            sequencer.advance(at + 1000);
            captures.push(sequencer.state.ready.drain(..).count());
        }
        assert_eq!(captures, [2, 2, 0, 0, 2, 2, 0]);
    }

    #[test]
    fn checkpoint_round_trips_in_flight_and_ready_captures() {
        let mut host = RecordingHost::default();
        let mut capture = InputCapture::with_sequencer(BASE, options([0; 3], 2), 4, None).unwrap();
        stream(&capture.transmit_port(2), 0);
        host.now = 1000;
        capture.advance(&mut Cx::new(&mut host, 4, 0, 0));
        stream(&capture.transmit_port(2), 1000);
        let saved = capture.checkpoint().unwrap();
        let mut fresh = InputCapture::with_sequencer(BASE, options([0; 3], 2), 4, None).unwrap();
        fresh.restore_checkpoint(&saved).unwrap();
        assert_eq!(fresh.checkpoint().unwrap(), saved);
        assert_eq!(fresh.next_deadline(1000), Some(2000));
        let other = InputCapture::with_sequencer(BASE, options([1, 0, 0], 2), 4, None).unwrap();
        assert!(other.validate_checkpoint(&saved).is_err());
        assert!(InputCapture::new(BASE).unwrap().validate_checkpoint(&saved).is_err());
    }

    #[test]
    fn unread_captures_and_bad_words_refuse() {
        let mut sequencer = Sequencer::new(options([0; 3], 2), 5, None).unwrap();
        let mut port = SequencerPort(Arc::new(Mutex::new(Sequencer::new(options([0; 3], 2), 5, None).unwrap())));
        assert!(port.transmit(0, 2, 0xc6c2).is_err());
        for _ in 0..CAPTURE_DEPTH {
            sequencer.word(0, 0x0007_0000).unwrap();
        }
        assert!(sequencer.word(0, 0x0007_0000).is_err());
        let mut reads = Sequencer::new(options([0; 3], 2), 5, None).unwrap();
        for _ in 0..3 {
            reads.word(0, 0xc6c2_ffff).unwrap();
        }
        assert!(reads.word(0, 0xc6c2_ffff).is_err());
        assert!(Sequencer::new(SequencerOptions { channel: 5, ..options([0; 3], 2) }, 5, None).is_err());
        assert!(InputCapture::new(BASE).unwrap().transmit_port(2).lock().unwrap().transmit(0, 4, 1).is_ok());
    }
}
