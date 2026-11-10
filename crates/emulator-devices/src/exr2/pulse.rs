//! FFFA9000 pulse unit: twelve 0x100-byte channels at FFFA9100. The shared
//! enable and routing bank selects output, interval, counter, and capture
//! modes. Clock divisors are declared
//! virtual-clock laws, not measured oscillator timing. The capture channel
//! counts virtual ticks from its arm or last input edge; each input edge after
//! the first latches the elapsed count and drives the channel's IRQ output.
//! External edges never come from elapsed guest time or program inspection.
pub mod block86000;
pub use block86000::Block86000;

use crate::bus::InterruptLine;
use crate::{Cx, Device};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

pub const BASE: u64 = 0xfffa9000;
pub const CHANNEL_BASE: u64 = BASE + 0x100;
const DIVISORS: [u64; 4] = [1, 8, 32, 128];
/// Declared capture clock: one count per virtual tick (unmeasured scale).
const CAPTURE_TICKS_PER_COUNT: u64 = 1;
const CAPTURE_MODE: u32 = 0x438b;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub counter_channels: Vec<u8>,
    pub timer_channels: Vec<u8>,
    /// Channels that also accept a polled countdown: control 0x800x with
    /// reload 0 runs once without an IRQ. A channel listed here and in
    /// `timer_channels` chooses the form each time it is armed.
    #[serde(default)]
    pub one_shot_channels: Vec<u8>,
    /// Interval-timer channels whose +08 word reads the running down-count
    /// instead of the programmed period.
    #[serde(default)]
    pub counter_readback_channels: Vec<u8>,
    /// Additional per-core lines an interval timer raises with its primary
    /// output, for a timer wired to more than one interrupt controller (EXR
    /// Pro's kernel tick, channel 7, reaches both cores' line 27).
    #[serde(default)]
    pub mirrored_irqs: Vec<MirroredIrq>,
    pub quadrature: bool,
    pub initial_phase: u8,
    pub capture_channel: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirroredIrq {
    pub channel: u8,
    pub core: usize,
    pub line: u32,
}

/// IRQ outputs are physical wiring. Interrupt masks do not select a CPU.
pub struct Wiring {
    pub irqs: [Option<InterruptLine>; 12],
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct Channel {
    words: [u32; 5],
    due: Option<u64>,
    interval: u64,
    /// Tick when a one-shot countdown was armed; cleared on disable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start: Option<u64>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct State {
    channels: [Channel; 12],
    enabled: u16,
    route: u32,
    /// Input routes for channels 8..11 (+0x18, one nibble per channel), as EXR Pro uses for channel 11.
    #[serde(default)]
    route_high: u32,
    clock_route: u32,
    phase_position: i64,
    anchor: i64,
    preload: u16,
    capture_level: bool,
    /// Tick the running capture count starts from: the arm or the last edge.
    capture_anchor: u64,
    /// Tick of the last input edge since the capture was armed.
    capture_edge: Option<u64>,
}

/// The lens and dial receive this explicit pin connection, not access to a
/// board, a CPU, or firmware RAM. Shared state also exposes physical PWM pins.
#[derive(Clone)]
pub struct PulsePins {
    state: Arc<Mutex<State>>,
    counters: u16,
    timers: u16,
    one_shots: u16,
    readbacks: u16,
    quadrature: bool,
    capture: Option<u8>,
    irqs: [Option<InterruptLine>; 12],
    mirrored: Vec<MirroredIrq>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Pwm {
    pub period: u64,
    pub duty: u16,
    pub counts: u32,
}

impl PulsePins {
    pub fn counter_wired(&self, channel: u8) -> bool {
        channel < 12 && self.counters & (1 << channel) != 0
    }
    pub fn quadrature_wired(&self) -> bool {
        self.quadrature
    }
    pub fn phase(&self) -> u8 {
        self.state.lock().unwrap().phase_position.rem_euclid(4) as u8
    }
    pub fn capture_wired(&self) -> bool {
        self.capture.is_some()
    }
    pub fn counter_value(&self, channel: u8) -> Result<u16, String> {
        if channel >= 12 {
            return Err("invalid counter channel".into());
        }
        Ok(self.state.lock().unwrap().channels[channel as usize].words[2] as u16)
    }
    pub fn pwm(&self, channel: u8) -> Result<Option<Pwm>, String> {
        if channel >= 12 {
            return Err("invalid pulse output channel".into());
        }
        let s = self.state.lock().unwrap();
        let c = &s.channels[channel as usize];
        if s.enabled & (1 << channel) == 0 {
            return Ok(None);
        }
        if !(0x8800..=0x8803).contains(&c.words[0]) {
            return Err("attached motor requires a repetitive pulse output".into());
        }
        // +08 initializes the running counter; +0C reloads subsequent cycles.
        // A guest write may change the reload while PWM is armed. The two
        // values need not stay equal after the initial complete configuration.
        let counts = c.words[3] + 1;
        if c.words[4] == 0 {
            return Ok(None);
        }
        Ok(Some(Pwm {
            period: u64::from(counts) * DIVISORS[(c.words[0] & 3) as usize],
            duty: c.words[4].min(counts) as u16,
            counts,
        }))
    }

    pub fn accepts_edges(&self, channel: u8) -> bool {
        if channel >= 8 || self.counters & (1 << channel) == 0 {
            return false;
        }
        let s = self.state.lock().unwrap();
        s.enabled & (1 << channel) != 0
            && matches!(s.channels[channel as usize].words[0], 0x2089..=0x208b)
    }

    /// Disabled counters discard edges. Direction is applied by the driver.
    pub fn counter_edges(&self, channel: u8, count: u64) -> Result<(), String> {
        if channel >= 8 || self.counters & (1 << channel) == 0 {
            return Err("counter input is not wired".into());
        }
        let mut s = self.state.lock().unwrap();
        if s.enabled & (1 << channel) == 0 {
            return Ok(());
        }
        let c = &mut s.channels[channel as usize];
        if !matches!(c.words[0], 0x2089..=0x208b) {
            return Err("external edges require an armed counter mode".into());
        }
        c.words[2] = c.words[2].wrapping_add(count as u32) & 0xffff;
        Ok(())
    }

    /// Phase is an ordinal around the Gray ring 11,10,00,01. The endpoint
    /// delivering electrical levels owns their polarity and pin assignment.
    pub fn quadrature_phase(&self, cx: &mut Cx<'_>, phase: u8) -> Result<(), String> {
        if !self.quadrature || phase > 3 {
            return Err("quadrature input is not wired".into());
        }
        let mut s = self.state.lock().unwrap();
        let old = s.phase_position;
        let delta = (i64::from(phase) - old).rem_euclid(4);
        if delta == 2 {
            return Err("quadrature input skipped an electrical phase".into());
        }
        s.phase_position += if delta == 3 { -1 } else { delta };
        if s.enabled & 1 != 0 && s.channels[0].words[0] == 0x870c {
            s.channels[0].words[2] = (u64::from(s.preload)
                + (s.phase_position.div_euclid(4) - s.anchor.div_euclid(4)).unsigned_abs())
                as u32
                & 0xffff;
        }
        if s.enabled & 2 != 0 && matches!(s.channels[1].words[0], 0x2089 | 0x208a) {
            let levels = [3, 2, 0, 1];
            let before = levels[old.rem_euclid(4) as usize] & 1 != 0;
            let after = levels[phase as usize] & 1 != 0;
            let edge = s.clock_route >> 8 & 15;
            if before != after && (edge == 1 && after || edge == 2 && before) {
                let count = s.channels[1].words[2] + 1;
                if count > s.channels[1].words[4] {
                    s.channels[1].words[3] = count & 0xffff;
                    s.channels[1].words[2] = 0;
                    if let Some(line) = self.irqs[1] {
                        match line {
                            InterruptLine::Core { core, line } => cx.raise_irq(core, line)?,
                            InterruptLine::Shared(line) => cx.set_irq(line, true),
                        }
                    }
                } else {
                    s.channels[1].words[2] = count;
                }
            }
        }
        Ok(())
    }

    /// Input level change at virtual tick `at`. While the capture mode is
    /// armed, every edge after the first latches the ticks since the previous
    /// edge into +10 and drives the channel IRQ output; +08 restarts at zero.
    pub fn capture_level(&self, cx: &mut Cx<'_>, level: bool, at: u64) -> Result<(), String> {
        let Some(channel) = self.capture else {
            return Err("capture input is not wired".into());
        };
        let ch = channel as usize;
        let mut s = self.state.lock().unwrap();
        if s.capture_level == level {
            return Ok(());
        }
        if s.capture_edge.is_some_and(|edge| at < edge) {
            return Err("capture input edge precedes the previous capture event".into());
        }
        s.capture_level = level;
        if s.enabled & (1 << ch) == 0 || s.channels[ch].words[0] != CAPTURE_MODE {
            return Ok(());
        }
        if let Some(edge) = s.capture_edge {
            let elapsed = at - edge;
            if elapsed < CAPTURE_TICKS_PER_COUNT {
                return Err("capture edge is faster than the declared capture clock".into());
            }
            s.channels[ch].words[4] = (elapsed / CAPTURE_TICKS_PER_COUNT) as u32 & 0xffff;
            if let Some(line) = self.irqs[ch] {
                match line {
                    InterruptLine::Core { core, line } => cx.raise_irq(core, line)?,
                    InterruptLine::Shared(line) => cx.set_irq(line, true),
                }
            }
        }
        s.capture_edge = Some(at);
        s.capture_anchor = at;
        Ok(())
    }
}

pub struct PulseUnit {
    pins: PulsePins,
}

/// Input route nibble of `channel`: +0x10 holds channels 0..7, +0x18 channels 8..11.
fn route_nibble(s: &State, channel: usize) -> u32 {
    if channel < 8 { s.route >> (channel * 4) & 15 } else { s.route_high >> ((channel - 8) * 4) & 15 }
}

impl PulseUnit {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        let mut counters = 0u16;
        for channel in options.counter_channels {
            if channel >= 12 || counters & (1 << channel) != 0 {
                return Err("counter inputs require unique channels below twelve".into());
            }
            counters |= 1 << channel;
        }
        let mut timers = 0u16;
        for channel in options.timer_channels {
            if channel >= 12
                || timers & (1 << channel) != 0
                || wiring.irqs[channel as usize].is_none()
            {
                return Err(
                    "timer channels require unique channels and physical IRQ wiring".into(),
                );
            }
            timers |= 1 << channel;
        }
        let mut one_shots = 0u16;
        for channel in options.one_shot_channels {
            if channel >= 12 || one_shots & (1 << channel) != 0 {
                return Err("one-shot channels must be unique and below twelve".into());
            }
            one_shots |= 1 << channel;
        }
        let mut readbacks = 0u16;
        for channel in options.counter_readback_channels {
            if channel >= 12 || timers & (1 << channel) == 0 || readbacks & (1 << channel) != 0 {
                return Err("counter readback channels must be unique interval timers".into());
            }
            readbacks |= 1 << channel;
        }
        for m in &options.mirrored_irqs {
            if m.channel >= 12 || timers & (1 << m.channel) == 0 {
                return Err("mirrored IRQs require a declared interval-timer channel".into());
            }
        }
        if options.initial_phase > 3
            || options.capture_channel.is_some_and(|v| v != 1)
            || options.quadrature && options.capture_channel.is_some()
        {
            return Err("capture supports channel one and excludes quadrature wiring".into());
        }
        let state = State {
            phase_position: i64::from(options.initial_phase),
            ..State::default()
        };
        Ok(Self {
            pins: PulsePins {
                state: Arc::new(Mutex::new(state)),
                counters,
                timers,
                one_shots,
                readbacks,
                quadrature: options.quadrature,
                capture: options.capture_channel,
                irqs: wiring.irqs,
                mirrored: options.mirrored_irqs,
            },
        })
    }

    pub fn pins(&self) -> PulsePins {
        self.pins.clone()
    }

    fn locate(address: u64, size: u8) -> Option<(usize, usize)> {
        if size != 4 || address & 3 != 0 {
            return None;
        }
        let delta = address.checked_sub(CHANNEL_BASE)?;
        let channel = (delta / 0x100) as usize;
        let offset = delta % 0x100;
        (channel < 12 && offset <= 16).then_some((channel, offset as usize / 4))
    }

    /// Returns the deadline, the interval and whether this arming is a polled one-shot.
    fn arm(&self, s: &State, channel: usize, now: u64) -> Result<Option<(u64, u64, bool)>, String> {
        let c = &s.channels[channel];
        let [control, config, period, reload, duty] = c.words;
        if self.pins.one_shots & (1 << channel) != 0
            && (0x8000..=0x8003).contains(&(control & !0x5000))
            && (reload == 0 || self.pins.timers & (1 << channel) == 0)
        {
            if config != 0 || period == 0 || reload != 0 || duty != 0 {
                return Err("incomplete one-shot timer configuration".into());
            }
            let interval = u64::from(period) * DIVISORS[(control & 3) as usize];
            return Ok(Some((
                now.checked_add(interval).ok_or("pulse deadline overflow")?,
                interval,
                true,
            )));
        }
        if control & !0x5000 >= 0x8000 && control & !0x5000 <= 0x8003 {
            if self.pins.timers & (1 << channel) == 0
                || self.pins.irqs[channel].is_none()
                || config != 0
                || period == 0
                || reload != period - 1
                || duty != 0
            {
                return Err("incomplete interval timer configuration or IRQ wiring".into());
            }
            let interval = u64::from(period) * DIVISORS[(control & 3) as usize];
            return Ok(Some((
                now.checked_add(interval).ok_or("pulse deadline overflow")?,
                interval,
                false,
            )));
        }
        if (0x8800..=0x8803).contains(&control) {
            if period < 2 || reload != period - 1 || duty == 0 || duty > period {
                return Err("incomplete repetitive pulse configuration".into());
            }
        } else if channel == 0 && self.pins.quadrature && control == 0x870c {
            if config != 0 {
                return Err("unsupported quadrature clock configuration".into());
            }
        } else if channel == 1 && self.pins.quadrature && matches!(control, 0x2089 | 0x208a) {
            if config != 0 {
                return Err("unsupported quadrature edge clock".into());
            }
        } else if self.pins.capture == Some(channel as u8) && control == CAPTURE_MODE {
            if config != 0 || period != 0 || reload != 0 || duty != 0 || s.route & 0xf0 != 0x10 {
                return Err("incomplete capture configuration".into());
            }
        } else if self.pins.counters & (1 << channel) != 0 && matches!(control, 0x2089..=0x208b) {
            if config != 0
                || period != 0
                || reload != 0
                || duty != 0xffff
                || route_nibble(s, channel) != channel as u32
            {
                return Err("incomplete external counter configuration".into());
            }
        } else {
            return Err("unsupported pulse channel mode".into());
        }
        Ok(None)
    }

    fn put(&self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) -> Result<(), String> {
        if size != 4 || address & 3 != 0 || value > u32::MAX as u64 {
            return Err("pulse unit requires aligned unsigned word accesses".into());
        }
        let value = value as u32;
        let mut s = self.pins.state.lock().unwrap();
        match address {
            BASE => {
                let mask = (value >> 16) as u16;
                let levels = value as u16;
                if mask & !0xfff != 0 || levels & !mask != 0 {
                    return Err("unknown pulse enable mask".into());
                }
                let enabled = s.enabled & !mask | levels;
                let starting = enabled & !s.enabled;
                let mut timers = [None; 12];
                // Validate the entire mask before changing any channel.
                for (ch, timer) in timers.iter_mut().enumerate() {
                    if starting & (1 << ch) != 0 {
                        *timer = self.arm(&s, ch, cx.now())?;
                    }
                }
                for (ch, timer) in timers.into_iter().enumerate() {
                    if enabled & (1 << ch) == 0 {
                        s.channels[ch].due = None;
                        s.channels[ch].start = None;
                    }
                    if let Some((due, interval, once)) = timer {
                        s.channels[ch].due = Some(due);
                        s.channels[ch].interval = interval;
                        s.channels[ch].start = once.then(|| cx.now());
                    }
                }
                if starting & 1 != 0 && self.pins.quadrature && s.channels[0].words[0] == 0x870c {
                    s.anchor = s.phase_position;
                    s.preload = s.channels[0].words[2] as u16;
                }
                if let Some(ch) = self.pins.capture
                    && starting & (1 << ch) != 0
                    && s.channels[ch as usize].words[0] == CAPTURE_MODE
                {
                    s.capture_anchor = cx.now();
                    s.capture_edge = None;
                }
                s.enabled = enabled;
            }
            a if a == BASE + 0x10 => {
                for ch in 0..8 {
                    let nibble = value >> (ch * 4) & 15;
                    let wired = self.pins.counters & (1 << ch) != 0
                        || ch == 1 && (self.pins.quadrature || self.pins.capture.is_some());
                    if nibble != 0 && (!wired || nibble != ch as u32) {
                        return Err("unsupported pulse input route".into());
                    }
                    if s.enabled & (1 << ch) != 0 && (value ^ s.route) & (15 << (ch * 4)) != 0 {
                        return Err("changing an armed pulse input route".into());
                    }
                }
                s.route = value;
            }
            a if a == BASE + 0x18 => {
                for ch in 8..12 {
                    let nibble = value >> ((ch - 8) * 4) & 15;
                    if nibble != 0 && (self.pins.counters & (1 << ch) == 0 || nibble != ch as u32) {
                        return Err("unsupported pulse input route".into());
                    }
                    if s.enabled & (1 << ch) != 0 && (value ^ s.route_high) & (15 << ((ch - 8) * 4)) != 0 {
                        return Err("changing an armed pulse input route".into());
                    }
                }
                if value >> 16 != 0 {
                    return Err("unsupported pulse input route".into());
                }
                s.route_high = value;
            }
            a if a == BASE + 0x14 => {
                if value & !0xf0f != 0 {
                    return Err("unsupported pulse clock input selection".into());
                }
                s.clock_route = value;
            }
            _ => {
                let (ch, word) = Self::locate(address, size).ok_or("unknown pulse register")?;
                let c = &s.channels[ch];
                if word == 0 {
                    let timer = self.pins.timers & (1 << ch) != 0
                        && (0x8000..=0x8003).contains(&(value & !0x5000));
                    let one_shot = self.pins.one_shots & (1 << ch) != 0
                        && (0x8000..=0x8003).contains(&(value & !0x5000));
                    let counter =
                        self.pins.counters & (1 << ch) != 0 && matches!(value, 0x2089..=0x208b);
                    let quad = self.pins.quadrature
                        && (ch == 0 && matches!(value, 0 | 0x870c)
                            || ch == 1 && matches!(value, 0 | 0x2089 | 0x208a));
                    if !timer
                        && !one_shot
                        && !counter
                        && !quad
                        && !(0x8800..=0x8803).contains(&value)
                        && !(self.pins.capture == Some(ch as u8) && value == CAPTURE_MODE)
                    {
                        return Err("unsupported pulse control mode".into());
                    }
                } else if value > 0xffff || word == 1 && value > 1 {
                    return Err("pulse configuration field exceeds its supported width".into());
                }
                if s.enabled & (1 << ch) != 0
                    && value != c.words[word]
                    && !((0x8800..=0x8803).contains(&c.words[0])
                        && (word != 0 || (0x8800..=0x8803).contains(&value)))
                {
                    let reset =
                        word == 2 && value == 0 && matches!(c.words[0], 0x2089..=0x208b | 0x870c);
                    // Quadrature channels take count, latch and terminal
                    // updates while they run; channel 0 re-anchors its preload.
                    let quadrature = self.pins.quadrature
                        && (2..=4).contains(&word)
                        && (ch == 0 && c.words[0] == 0x870c
                            || ch == 1 && matches!(c.words[0], 0x2089 | 0x208a));
                    if !reset && !quadrature {
                        return Err("changing an armed pulse configuration requires disable".into());
                    }
                }
                s.channels[ch].words[word] = value;
                if ch == 0 && word == 2 && self.pins.quadrature {
                    s.preload = value as u16;
                    s.anchor = s.phase_position;
                }
            }
        }
        Ok(())
    }
}

impl Device for PulseUnit {
    fn checkpoint(&self) -> Result<crate::checkpoint::Checkpoint, String> {
        crate::checkpoint::Checkpoint::encode(
            self.name(),
            &*self
                .pins
                .state
                .lock()
                .map_err(|_| "PulseUnit state poisoned")?,
        )
    }
    fn validate_checkpoint(&self, saved: &crate::checkpoint::Checkpoint) -> Result<(), String> {
        let _: State = saved.decode(self.name())?;
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &crate::checkpoint::Checkpoint) -> Result<(), String> {
        *self
            .pins
            .state
            .lock()
            .map_err(|_| "PulseUnit state poisoned")? = saved.decode::<State>(self.name())?;
        Ok(())
    }

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr2-pulse"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.advance(cx);
        if size != 4 || address & 3 != 0 {
            cx.fail("pulse unit requires aligned word accesses");
            return 0;
        }
        let s = self.pins.state.lock().unwrap();
        match address {
            BASE => s.enabled as u64,
            a if a == BASE + 0x10 => s.route as u64,
            a if a == BASE + 0x14 => s.clock_route as u64,
            a if a == BASE + 0x18 => s.route_high as u64,
            _ => match Self::locate(address, size) {
                Some((ch, word)) => {
                    if word == 2
                        && self.pins.one_shots & (1 << ch) != 0
                        && let Some(start) = s.channels[ch].start
                    {
                        let divisor = DIVISORS[(s.channels[ch].words[0] & 3) as usize];
                        let elapsed = cx.now().saturating_sub(start);
                        return u64::from(
                            s.channels[ch].words[2].saturating_sub((elapsed / divisor) as u32),
                        );
                    }
                    if word == 2
                        && self.pins.readbacks & (1 << ch) != 0
                        && let Some(due) = s.channels[ch].due
                    {
                        let divisor = DIVISORS[(s.channels[ch].words[0] & 3) as usize];
                        return due.saturating_sub(cx.now()).div_ceil(divisor) & 0xffff_ffff;
                    }
                    if s.enabled & (1 << ch) != 0 && (word == 2 || word == 3) {
                        if s.channels[ch].words[0] == CAPTURE_MODE {
                            if word == 3 {
                                return u64::from(s.channels[ch].words[3]);
                            }
                            let elapsed = cx.now().saturating_sub(s.capture_anchor);
                            return (elapsed / CAPTURE_TICKS_PER_COUNT) & 0xffff;
                        }
                        if word == 3
                            && !(self.pins.quadrature && ch == 1)
                            && matches!(s.channels[ch].words[0], 0x2089..=0x208b)
                        {
                            cx.fail("counter capture latch is unsupported");
                            return 0;
                        }
                    }
                    s.channels[ch].words[word] as u64
                }
                None => {
                    cx.unmodeled();
                    0
                }
            },
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.advance(cx);
        if cx.refusal().is_none()
            && let Err(error) = self.put(cx, address, size, value)
        {
            cx.fail(error);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let mut s = self.pins.state.lock().unwrap();
        for (ch, c) in s.channels.iter_mut().enumerate() {
            let Some(due) = c.due else { continue };
            if cx.now() < due {
                continue;
            }
            // A one-shot countdown fires once and stays expired until the
            // channel is disabled; an interval timer re-arms immediately.
            let one_shot = c.start.is_some();
            let Some(line) = self.pins.irqs[ch] else {
                if one_shot {
                    c.due = None;
                    continue;
                }
                cx.fail("pulse IRQ is unconnected");
                return;
            };
            match line {
                InterruptLine::Core { core, line } => {
                    if let Err(error) = cx.raise_irq(core, line) {
                        cx.fail(error);
                        return;
                    }
                }
                InterruptLine::Shared(line) => cx.set_irq(line, true),
            }
            for m in self.pins.mirrored.iter().filter(|m| usize::from(m.channel) == ch) {
                if let Err(error) = cx.raise_irq(m.core, m.line) {
                    cx.fail(error);
                    return;
                }
            }
            c.due = if one_shot || c.words[0] & 0x4000 != 0 {
                None
            } else {
                due.checked_add((1 + (cx.now() - due) / c.interval) * c.interval)
            };
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.pins
            .state
            .lock()
            .unwrap()
            .channels
            .iter()
            .filter_map(|c| c.due)
            .min()
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(&*self.pins.state.lock().unwrap()).unwrap()
    }
}

#[cfg(test)]
#[path = "pulse/input_tests.rs"]
mod input_tests;
#[cfg(test)]
#[path = "pulse/interval_tests.rs"]
mod interval_tests;
#[cfg(test)]
#[path = "pulse/one_shot_tests.rs"]
mod one_shot_tests;
