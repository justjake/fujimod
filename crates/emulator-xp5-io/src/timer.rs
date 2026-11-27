//! X100VI 48 MHz general/PWM and reload timers (ff908000), plus six
//! extended general channels (ff909000). Finder logical17 uses general6/IRQ42.
//! External capture inputs are not synthesized.
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const HZ: u128 = 48_000_000;

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct Counter {
    value: u32,
    reload: u32,
    enabled: bool,
    started_ns: u64,
    generation: u64,
}
impl Counter {
    fn current(&self, now: Duration) -> u32 {
        if !self.enabled {
            return self.value;
        }
        let ticks = (now.as_nanos().saturating_sub(self.started_ns as u128) * HZ) / 1_000_000_000;
        if ticks <= self.value as u128 {
            return self.value - ticks as u32;
        }
        let phase = (ticks - self.value as u128 - 1) % (self.reload as u128 + 1);
        self.reload - phase as u32
    }
    fn deadline(&self, now: Duration) -> Duration {
        let ticks = self.current(now) as u128 + 1;
        let ns = (ticks * 1_000_000_000).div_ceil(HZ);
        now + Duration::from_nanos(ns as u64)
    }
}

// General register words: initial count, terminal reload/compare, two PWM
// compares. Bit4 selects software one-shot handling, not an IRQ acknowledgment.
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct General {
    control: u32,
    words: [u32; 4],
    count: Counter,
    pending: bool,
    #[serde(default)]
    capture_origin: u64,
}
impl General {
    fn compare(&self, now: Duration) -> Option<Duration> {
        let current = self.count.current(now);
        let compare = self.words[3];
        if self.control & 0x1000 == 0 || compare == 0 || compare >= current {
            return None;
        }
        let ns = ((current - compare) as u128 * 1_000_000_000).div_ceil(HZ);
        Some(now + Duration::from_nanos(ns as u64))
    }
}

/// Extended-timer general-channel words (initial count, period, first and second
/// compare) shared read-only with models of what the PWM outputs drive.
pub type PwmOutputs = std::sync::Arc<std::sync::Mutex<[[u32; 4]; 6]>>;

#[derive(Default, Serialize, Deserialize)]
pub struct Timer {
    #[serde(skip)]
    base: u64,
    counters: [Counter; 16],
    #[serde(default)]
    general: [General; 16],
    #[serde(default)]
    extended: bool,
    #[serde(default)]
    routing: [u32; 2],
    #[serde(default)]
    group: u32,
    #[serde(skip)]
    companion_cpu: Option<usize>,
    #[serde(skip)]
    focus_capture: Option<fujimod_emulator_devices::xp5::pins::SharedPins>,
    /// General channel that is the focus position counter (an up/down count of the focus
    /// motor), and the motor position when the guest last loaded it. Host wiring, not state.
    focus_position: Option<(usize, i64)>,
    /// The counter falls when the motor steps forward.
    focus_position_inverted: bool,
    /// Extended timer: the general channel whose PWM pair drives the focus motor.
    focus_drive: Option<usize>,
    /// Host speed option, not hardware (`companion_idle_divide = N`): while the focus motor's
    /// PWM is not driving its bridge (`Pins::focus_driving`) or holds a neutral duty, only one in N reload-timer interrupts is raised on the
    /// companion core. The GFX100RF companion takes channel 11 at 16 kHz for its servo loop
    /// whether or not it is driving, and entering the companion's interpreter that often was
    /// most of the host time in shooting mode (22.6 of 44 s for 9 guest seconds; 3.5 s with
    /// every interrupt divided by 8). While the bridge is on every interrupt is raised, so the
    /// servo's own timing is unchanged. The guest does see the slower idle loop.
    #[serde(skip)]
    companion_idle_divide: u32,
    #[serde(skip)]
    pwm: Option<PwmOutputs>,
    /// General channels that count pulses of an external input the board declares
    /// (a command dial): bit n holds channel n at its loaded word instead of letting
    /// it run on the 48 MHz clock, since nothing turns the dial.
    #[serde(skip)]
    external_count: u16,
}
impl Timer {
    pub fn new(base: u64) -> Self {
        Self {
            base,
            ..Self::default()
        }
    }
    pub fn extended(base: u64) -> Self {
        Self {
            base,
            extended: true,
            ..Self::default()
        }
    }
    fn general_len(&self) -> usize {
        if self.extended { 6 } else { 16 }
    }
    fn general_irq(&self, n: usize) -> u32 {
        (if self.extended { 68 } else { 36 }) + n as u32
    }
    fn schedule_general(&self, cx: &mut Cx<'_>, n: usize) {
        let g = &self.general[n];
        if self.focus_capture.is_some() && n == 9 && g.control & 3 == 3 {
            return; // External pulse capture, not a free-running48MHz timer.
        }
        if self.counts_externally(n) {
            return; // An idle external input never reaches a terminal count.
        }
        // Boot enables some unconfigured PWM channels with both period words
        // zero. They have no programmed terminal interval; do not synthesize
        // a 48 MHz interrupt storm that starves guest register programming.
        if g.count.enabled && (g.words[0] != 0 || g.words[1] != 0) {
            let token = (1 << 63) | (g.count.generation << 8) | n as u64;
            // Fast PWM interrupt edges are coalesced to the declared 1ms output
            // sampling interval instead of trapping every physical pulse.
            // Normal interval timers, including finder6, retain their deadline.
            let now = Duration::from_nanos(cx.now());
            // The focus motor's PWM channel is coalesced the same way: its servo loop runs on a
            // reload counter, not on this period interrupt (GFX100RF: about 62 kHz otherwise).
            let floor = if (g.control & 0x1000 != 0 || self.focus_drive == Some(n))
                && g.count.reload < 47999
            {
                now + Duration::from_millis(1)
            } else {
                now
            };
            cx.schedule(
                (g.count.deadline(now).max(floor))
                    .as_nanos()
                    .min(u64::MAX as u128) as u64,
                token,
            );
            if let Some(at) = g.compare(now) {
                cx.schedule(
                    (at.max(floor)).as_nanos().min(u64::MAX as u128) as u64,
                    token | 0x40,
                );
            }
        }
    }
    fn general_register(&self, o: u64) -> Option<(usize, usize)> {
        if (0x100..0x100 + self.general_len() as u64 * 16).contains(&o) && o & 3 == 0 {
            Some(((o - 0x100) as usize / 16, (o as usize & 15) / 4))
        } else {
            None
        }
    }
    pub fn with_companion(mut self, cpu: Option<usize>) -> Self {
        self.companion_cpu = cpu;
        self
    }
    /// Hold the general channels in `mask` at their loaded word: their clock is an
    /// external input with no pulses.
    pub fn with_external_count(mut self, mask: u16) -> Self {
        self.external_count = mask;
        self
    }
    /// GFX100RF 1.13: lens resource 0x1a is timer id 0x25, general channel 8 (tables 0x1bafc98
    /// and 0x19d7840). The lens module loads 0x8000 and reads position as count - 0x8000
    /// (0x165a9cc, 0x165abe8). The channel follows the focus motor's net steps one to one;
    /// the counts per step of the real encoder are not known.
    pub fn with_focus_position(mut self, channel: usize) -> Self {
        self.focus_position = Some((channel, 0));
        self.external_count |= 1 << channel;
        self
    }
    pub fn with_companion_idle_divide(mut self, divide: u32) -> Self {
        self.companion_idle_divide = divide;
        self
    }
    pub fn with_focus_position_inverted(mut self, inverted: bool) -> Self {
        self.focus_position_inverted = inverted;
        self
    }
    /// GFX100RF 1.13: the companion core's focus servo programs extended general channel 0
    /// (its view 0x5f909100: +4 period, +8 and +0xc compares, both period / 2 at rest). The
    /// drive is the second compare about the half period.
    pub fn with_focus_drive(
        mut self,
        pins: fujimod_emulator_devices::xp5::pins::SharedPins,
        channel: usize,
    ) -> Self {
        self.focus_capture = Some(pins);
        self.focus_drive = Some(channel);
        self
    }
    /// Advance the PWM-driven focus motor to now and present its home sensor: the pad and the
    /// two level interrupts INTID 100 + pin and 116 + pin (selectors 0xb7 and 0xb8).
    fn advance_focus(&self, cx: &mut Cx<'_>) {
        let Some(pins) = &self.focus_capture else {
            return;
        };
        let mut pins = pins.lock().unwrap();
        let Some(home) = pins.advance_focus(cx.now()) else {
            return;
        };
        let pin = pins.focus_servo.as_ref().map_or(0, |servo| servo.home.1);
        pins.set_input(2, pin, home);
        pins.set_input(1, pin, !home);
        drop(pins);
        cx.set_irq(100 + pin as u32, home);
        cx.set_irq(116 + pin as u32, !home);
    }
    fn focus_steps(&self) -> i64 {
        self.focus_capture
            .as_ref()
            .map_or(0, |pins| pins.lock().unwrap().focus_position)
    }
    fn counts_externally(&self, n: usize) -> bool {
        n < 16 && self.external_count >> n & 1 != 0
    }
    pub fn with_focus_capture(
        mut self,
        pins: fujimod_emulator_devices::xp5::pins::SharedPins,
    ) -> Self {
        self.focus_capture = Some(pins);
        self
    }
    /// Publish the general-channel words of an extended timer to `outputs`.
    pub fn with_pwm_outputs(mut self, outputs: PwmOutputs) -> Self {
        self.pwm = Some(outputs);
        self.publish_pwm();
        self
    }
    fn publish_pwm(&self) {
        if let Some(outputs) = &self.pwm {
            let mut outputs = outputs.lock().unwrap();
            for (n, output) in outputs.iter_mut().enumerate() {
                *output = self.general[n].words;
            }
        }
    }
    fn schedule(&self, cx: &mut Cx<'_>, channel: usize) {
        let c = &self.counters[channel];
        if c.enabled {
            cx.schedule(
                (c.deadline(Duration::from_nanos(cx.now())))
                    .as_nanos()
                    .min(u64::MAX as u128) as u64,
                (c.generation << 4) | channel as u64,
            );
        }
    }
    fn channel(offset: u64) -> Option<(usize, bool)> {
        if (0x310..0x390).contains(&offset) && offset & 3 == 0 {
            Some((((offset - 0x310) / 8) as usize, offset & 4 != 0))
        } else {
            None
        }
    }
}
#[derive(Serialize)]
struct TimerSnapshotRef<'a> {
    counters: &'a [Counter; 16],
    general: &'a [General; 16],
    routing: &'a [u32; 2],
    group: &'a u32,
}
#[derive(Deserialize)]
struct TimerSnapshot {
    counters: [Counter; 16],
    general: [General; 16],
    routing: [u32; 2],
    group: u32,
}
impl Device for Timer {
    fn name(&self) -> &'static str {
        if self.extended {
            "soc-extended-timers"
        } else {
            "soc-reload-counters"
        }
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(self.base);
        if size != 4 || offset & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        if offset == 0 {
            return self.general[..self.general_len()]
                .iter()
                .enumerate()
                .fold(0, |v, (n, g)| v | ((g.count.enabled as u64) << n));
        }
        if offset == 4 || offset == 8 {
            return self.routing[(offset / 4 - 1) as usize] as u64;
        }
        if offset == 0x80 {
            return self.group as u64;
        }
        if (0x10..0x10 + self.general_len() as u64 * 4).contains(&offset) {
            let g = &self.general[(offset as usize - 0x10) / 4];
            return (g.control | ((g.pending as u32) << 16)) as u64;
        }
        if let Some((n, w)) = self.general_register(offset) {
            if n == 9
                && w == 0
                && self.general[n].control & 3 == 3
                && let Some(pins) = &self.focus_capture
            {
                // Logical18/selector33 -> general9. The timer scales
                // the captured low13 bits by8 to compare with lens steps.
                let pulses = pins.lock().unwrap().focus_pulses;
                return self.general[n].words[0]
                    .wrapping_add((pulses.wrapping_sub(self.general[n].capture_origin) / 8) as u32)
                    as u64;
            }
            // While a general channel runs, word 0 reads its live down-count.
            // X-T5 4.31 stage0's microsecond delay (0xFF80196C) polls 0x110
            // after loading 0xFFFFFFFF there and enabling channel 1.
            if w == 0 && self.general[n].count.enabled && !self.counts_externally(n) {
                return self.general[n]
                    .count
                    .current(Duration::from_nanos(cx.now())) as u64;
            }
            if w == 0
                && let Some((channel, origin)) = self.focus_position
                && channel == n
            {
                self.advance_focus(cx);
                let moved = self.focus_steps() - origin;
                let moved = if self.focus_position_inverted {
                    -moved
                } else {
                    moved
                };
                return self.general[n].words[0].wrapping_add(moved as u32) as u64;
            }
            if w == 0 && self.counts_externally(n) {
                return self.general[n].words[0] as u64;
            }
            return self.general[n].words[w] as u64;
        }
        if (0x200..0x200 + self.general_len() as u64 * 4).contains(&offset) {
            return self.general[(offset as usize - 0x200) / 4]
                .count
                .current(Duration::from_nanos(cx.now())) as u64;
        }
        if self.extended {
            cx.unmodeled();
            return 0;
        }
        if offset == 0x300 {
            return self
                .counters
                .iter()
                .enumerate()
                .fold(0, |v, (n, c)| v | ((c.enabled as u64) << n));
        }
        if let Some((n, reload)) = Self::channel(offset) {
            return if reload {
                self.counters[n].reload
            } else {
                self.counters[n].current(Duration::from_nanos(cx.now()))
            } as u64;
        }
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(channel) = self.focus_drive else {
            return self.write_register(cx, address, size, value);
        };
        // Move the focus motor under the drive it had up to now, then take the new one.
        self.advance_focus(cx);
        self.write_register(cx, address, size, value);
        // The servo counts as started from its first write to the drive channel's words.
        let touched = self
            .general_register(address.wrapping_sub(self.base))
            .is_some_and(|(n, _)| n == channel);
        let g = &self.general[channel];
        // Signed about the half period: observed duty words on the GFX100RF were 1177 with
        // the servo pushing one way and 133 with it pushing the other, at period 1311
        // (655 +- 522), with the first compare at 0. A stopped channel drives nothing.
        let drive = if g.count.enabled {
            i64::from(g.words[3]) - i64::from(g.words[1] / 2)
        } else {
            0
        };
        if let Some(pins) = &self.focus_capture
            && let Some(servo) = pins.lock().unwrap().focus_servo.as_mut()
        {
            servo.drive = drive;
            servo.started |= touched;
        }
    }

    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token & (1 << 63) != 0 {
            let n = (token & 15) as usize;
            let generation = (token & !(1 << 63)) >> 8;
            let g = &mut self.general[n];
            if !g.count.enabled || generation != g.count.generation {
                return;
            }
            g.pending = true;
            cx.set_irq(self.general_irq(n), true);
            cx.set_irq(self.general_irq(n), false);
            if token & 0x40 == 0 {
                let g = &mut self.general[n];
                g.count.value = g.count.current(Duration::from_nanos(cx.now()));
                g.count.started_ns = Duration::from_nanos(cx.now()).as_nanos() as u64;
                g.count.generation += 1;
                self.schedule_general(cx, n);
            }
            return;
        }
        let n = (token & 15) as usize;
        let c = &self.counters[n];
        if !c.enabled || token >> 4 != c.generation {
            return;
        }
        // Timer channels map to edge-triggered interrupt IDs 52..67. The interrupt
        // remains asserted only for the duration of the modeled edge.
        cx.set_irq(52 + n as u32, true);
        cx.set_irq(52 + n as u32, false);
        // M-core routing maps reload channels 8..15 to IRQs 9..16; reload channels
        // 0..7 have no M-core route.
        // `companion_idle_divide`: while the focus servo's bridge is off, only one in N of these
        // interrupts reaches the companion core (see the field).
        let idle = self.companion_idle_divide > 1
            && self.focus_capture.as_ref().is_some_and(|pins| {
                let pins = pins.lock().unwrap();
                // Not driving, or driving with a neutral duty: the motor is held where it is.
                // The first loop pass that commands a move makes the duty non-neutral and the
                // full rate is back within one thinned period.
                pins.focus_driving() == Some(false)
                    || pins
                        .focus_servo
                        .as_ref()
                        .is_some_and(|servo| servo.started && servo.drive == 0)
            });
        if n >= 8
            && let Some(cpu) = self.companion_cpu
            && let Err(e) = cx.raise_irq(cpu, (n + 1) as u32)
        {
            cx.fail(e);
        }
        if idle && n >= 8 {
            // Thinned: the next expiry served is `companion_idle_divide` periods away, so the
            // skipped ones cost no device event either.
            let c = &self.counters[n];
            let now = Duration::from_nanos(cx.now());
            let next = c.deadline(now);
            let period = next.saturating_sub(now);
            let at = next + period * (self.companion_idle_divide - 1);
            cx.schedule(
                at.as_nanos().min(u64::MAX as u128) as u64,
                (c.generation << 4) | n as u64,
            );
        } else {
            self.schedule(cx, n);
        }
    }
    // Field-for-field the `checkpoint_fields!` encoding, written out so a restore
    // also republishes the PWM words.
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(
            self.name(),
            &TimerSnapshotRef {
                counters: &self.counters,
                general: &self.general,
                routing: &self.routing,
                group: &self.group,
            },
        )
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let _: TimerSnapshot = saved.decode(self.name())?;
        if !saved.blobs().is_empty() {
            return Err("device snapshot blob lengths differ".into());
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let TimerSnapshot {
            counters,
            general,
            routing,
            group,
        } = saved.decode(self.name())?;
        self.counters = counters;
        self.general = general;
        self.routing = routing;
        self.group = group;
        self.publish_pwm();
        Ok(())
    }
    fn save(&self) -> Value {
        serde_json::to_value(self).expect("timer state")
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        let mut saved: Self = serde_json::from_value(state.clone())
            .map_err(|e| format!("invalid timer checkpoint: {e}"))?;
        saved.base = self.base;
        saved.companion_cpu = self.companion_cpu;
        saved.focus_capture = self.focus_capture.clone();
        saved.focus_position = self.focus_position;
        saved.focus_position_inverted = self.focus_position_inverted;
        saved.focus_drive = self.focus_drive;
        saved.companion_idle_divide = self.companion_idle_divide;
        saved.external_count = self.external_count;
        saved.pwm = self.pwm.clone();
        *self = saved;
        self.publish_pwm();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn countdown_reload_and_large_elapsed_time() {
        let c = Counter {
            value: 480_000,
            reload: 479_999,
            enabled: true,
            started_ns: 0,
            generation: 0,
        };
        assert_eq!(c.current(Duration::from_millis(5)), 240_000);
        assert_eq!(c.current(Duration::from_millis(10)), 0);
        assert_eq!(c.current(Duration::from_nanos(10_000_021)), 479_999);
        assert_eq!(c.current(Duration::from_secs(1000)), 0);
        assert_eq!(
            c.deadline(Duration::from_nanos(10_000_021)),
            Duration::from_nanos(20_000_021)
        );
    }
    #[derive(Default)]
    struct Host {
        now: Duration,
        events: Vec<(Duration, usize, u64)>,
        irqs: Vec<u32>,
    }
    impl fujimod_emulator_devices::Host for Host {
        fn now(&self) -> u64 {
            self.now.as_nanos() as u64
        }
        fn schedule(&mut self, device: usize, at: u64, token: u64) {
            self.events.push((Duration::from_nanos(at), device, token));
        }
        fn set_irq(&mut self, irq: u32, level: bool) {
            if level {
                self.irqs.push(irq);
            }
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            Err("no RAM".into())
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            Err("no RAM".into())
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            Err("no companion CPU".into())
        }
        fn record(&mut self, _: fujimod_emulator_devices::Record) {}
        fn log(&mut self, _: &str) {}
        fn console(&mut self, _: &str) {}
    }
    #[test]
    fn virtual_focus_external_capture_reset_and_restore() {
        let pins = fujimod_emulator_devices::xp5::pins::Pins::shared();
        pins.lock().unwrap().focus_pulses = 80;
        let mut timer = Timer::new(0).with_focus_capture(pins.clone());
        let mut host = Host::default();
        for (o, v) in [
            (0x34, 15),
            (0x190, 0),
            (0x194, 0),
            (0x198, u32::MAX),
            (0x19c, u32::MAX),
            (0, 0x10001 << 9),
        ] {
            timer.write(&mut Cx::new(&mut host, 0, 0, 0), o, 4, u64::from(v));
        }
        assert!(host.events.is_empty());
        pins.lock().unwrap().focus_pulses += 31;
        host.now = Duration::from_secs(10);
        assert_eq!(timer.read(&mut Cx::new(&mut host, 0, 0, 0), 0x190, 4), 3);
        let mut restored = Timer::new(0).with_focus_capture(pins.clone());
        restored.restore(&timer.save()).unwrap();
        pins.lock().unwrap().focus_pulses += 1;
        assert_eq!(restored.read(&mut Cx::new(&mut host, 0, 0, 0), 0x190, 4), 4);
        restored.write(&mut Cx::new(&mut host, 0, 0, 0), 0x190, 4, 0);
        assert_eq!(restored.read(&mut Cx::new(&mut host, 0, 0, 0), 0x190, 4), 0);
    }
    #[test]
    fn running_general_word0_reads_the_live_count() {
        let mut timer = Timer::new(0);
        let mut host = Host::default();
        // X-T5 4.31 stage0 delay: clear status, load both words, enable channel 1.
        for (o, v) in [
            (0x14, 0x10000),
            (0x110, u32::MAX),
            (0x114, u32::MAX),
            (0, 0x20002),
        ] {
            timer.write(&mut Cx::new(&mut host, 0, 0, 0), o, 4, u64::from(v));
        }
        host.now = Duration::from_micros(2000);
        let elapsed = u64::from(u32::MAX) - timer.read(&mut Cx::new(&mut host, 0, 0, 0), 0x110, 4);
        assert_eq!(elapsed, 2000 * 48);
        // The reload word and a stopped channel's word 0 still read back as written.
        assert_eq!(
            timer.read(&mut Cx::new(&mut host, 0, 0, 0), 0x114, 4),
            u64::from(u32::MAX)
        );
        timer.write(&mut Cx::new(&mut host, 0, 0, 0), 0, 4, 0x20000);
        timer.write(&mut Cx::new(&mut host, 0, 0, 0), 0x110, 4, 1234);
        assert_eq!(timer.read(&mut Cx::new(&mut host, 0, 0, 0), 0x110, 4), 1234);
    }
    #[test]
    fn finder_period_expiry_ack_and_cancel() {
        use fujimod_emulator_devices::test_host::bus::TestBus;
        let mut bus = TestBus::default();
        bus.register(0xff908000, 0x1000, Box::new(Timer::new(0xff908000)))
            .unwrap();
        let mut h = Host::default();
        // Logical17/resource21/general6 uses a 5523-microsecond period.
        let ticks = 0x1593 * 48;
        for (o, v) in [
            (0x28, 0x10000),
            (0x160, ticks),
            (0x164, ticks - 1),
            (0x168, 0),
            (0x16c, 0),
            (0, 0x10001 << 6),
        ] {
            bus.write(&mut Cx::new(&mut h, 0, 0, 0), 0xff908000 + o, 4, v)
                .unwrap();
        }
        let (at, d, t) = h.events.pop().unwrap();
        assert_eq!(
            at,
            Duration::from_nanos((ticks + 1) * 1_000_000_000 / 48_000_000 + 1)
        );
        h.now = Duration::from_micros(1000);
        assert_eq!(
            bus.read(&mut Cx::new(&mut h, 0, 0, 0), 0xff908218, 4)
                .unwrap(),
            ticks - 48000
        );
        h.now = at;
        bus.event(&mut Cx::new(&mut h, 0, 0, 0), d, t).unwrap();
        assert_eq!(h.irqs, vec![42]);
        let (next, d, t) = h.events.pop().unwrap();
        assert!(next > at);
        bus.write(&mut Cx::new(&mut h, 0, 0, 0), 0xff908028, 4, 0x10000)
            .unwrap();
        assert_eq!(
            bus.read(&mut Cx::new(&mut h, 0, 0, 0), 0xff908028, 4)
                .unwrap(),
            0
        );
        // Stock stopping callback invalidates already queued periodic expiry.
        bus.write(&mut Cx::new(&mut h, 0, 0, 0), 0xff908000, 4, 0x10000 << 6)
            .unwrap();
        h.now = next;
        bus.event(&mut Cx::new(&mut h, 0, 0, 0), d, t).unwrap();
        assert_eq!(h.irqs, vec![42]);
    }
    #[test]
    fn extended_pwm_compare_then_reload_and_checkpoint() {
        use fujimod_emulator_devices::test_host::bus::TestBus;
        let mut bus = TestBus::default();
        let mut timer = Timer::extended(0);
        let mut h = Host::default();
        for (o, v) in [
            (0x10, 0x11000),
            (0x100, 48000),
            (0x104, 47999),
            (0x108, 24000),
            (0x10c, 24000),
            (0, 0x10001),
        ] {
            timer.write(&mut Cx::new(&mut h, 0, 0, 0), o, 4, v);
        }
        let mut restored = Timer::extended(0);
        restored.restore(&timer.save()).unwrap();
        bus.register(0xff909000, 0x1000, Box::new(restored))
            .unwrap();
        h.events.sort_by_key(|e| e.0);
        let (at, d, t) = h.events.remove(0);
        assert_eq!(at, Duration::from_micros(500));
        h.now = at;
        bus.event(&mut Cx::new(&mut h, 0, 0, 0), d, t).unwrap();
        assert_eq!(h.irqs, vec![68]);
        let (at, d, t) = h.events.remove(0);
        h.now = at;
        bus.event(&mut Cx::new(&mut h, 0, 0, 0), d, t).unwrap();
        assert_eq!(h.irqs, vec![68, 68]);
        assert_eq!(h.events.len(), 2);
    }
    #[test]
    fn unconfigured_pwm_does_not_flood_event_queue() {
        let mut t = Timer::new(0);
        let mut h = Host::default();
        t.write(&mut Cx::new(&mut h, 0, 0, 0), 0, 4, 0x10001);
        assert!(h.events.is_empty());
        t.write(&mut Cx::new(&mut h, 0, 0, 0), 0x100, 4, 48000);
        t.write(&mut Cx::new(&mut h, 0, 0, 0), 0x104, 4, 47999);
        assert!(!h.events.is_empty());
    }
    #[test]
    fn fast_pwm_coalesces_edges_but_keeps_live_count() {
        let mut t = Timer::new(0);
        let mut h = Host::default();
        for (o, v) in [
            (0x3c, 0x15000),
            (0x1b0, 12),
            (0x1b4, 11),
            (0x1b8, 6),
            (0x1bc, 6),
            (0, 0x10001 << 11),
        ] {
            t.write(&mut Cx::new(&mut h, 0, 0, 0), o, 4, v);
        }
        assert_eq!(h.events.len(), 2);
        assert!(h.events.iter().all(|e| e.0 == Duration::from_millis(1)));
        h.now = Duration::from_nanos(125);
        assert_eq!(t.read(&mut Cx::new(&mut h, 0, 0, 0), 0x22c, 4), 6);
    }
}

impl Timer {
    fn write_register(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(self.base);
        if size != 4 || offset & 3 != 0 {
            cx.unmodeled();
            return;
        }
        let value = value as u32;
        if offset == 0 {
            for n in 0..self.general_len() {
                if value & (1 << (16 + n)) == 0 {
                    continue;
                }
                let g = &mut self.general[n];
                let enable = value & (1 << n) != 0;
                if enable != g.count.enabled {
                    g.count.value = if enable {
                        g.words[0]
                    } else {
                        g.count.current(Duration::from_nanos(cx.now()))
                    };
                    g.count.started_ns = Duration::from_nanos(cx.now()).as_nanos() as u64;
                    g.count.enabled = enable;
                    g.count.generation += 1;
                    if !enable {
                        g.pending = false;
                    }
                    self.schedule_general(cx, n);
                }
            }
            return;
        }
        if offset == 4 || offset == 8 {
            let r = &mut self.routing[(offset / 4 - 1) as usize];
            let mask = value >> 16;
            *r = (*r & !mask) | (value & mask);
            return;
        }
        if offset == 0x80 {
            self.group = value;
            return;
        }
        if (0x10..0x10 + self.general_len() as u64 * 4).contains(&offset) {
            let n = (offset as usize - 0x10) / 4;
            let g = &mut self.general[n];
            // Stock config writes bit16 to clear the terminal status. IRQs are
            // edge-triggered; software one-shots ACK by stopping at +0.
            if value & 0x10000 != 0 {
                g.pending = false;
            }
            g.control = value & 0xffff;
            g.count.value = g.count.current(Duration::from_nanos(cx.now()));
            g.count.started_ns = Duration::from_nanos(cx.now()).as_nanos() as u64;
            g.count.generation += 1;
            self.schedule_general(cx, n);
            return;
        }
        if let Some((n, w)) = self.general_register(offset) {
            if w == 0
                && let Some((channel, _)) = self.focus_position
                && channel == n
            {
                self.focus_position = Some((channel, self.focus_steps()));
            }
            let g = &mut self.general[n];
            g.count.value = g.count.current(Duration::from_nanos(cx.now()));
            g.count.started_ns = Duration::from_nanos(cx.now()).as_nanos() as u64;
            g.words[w] = value;
            if w == 0 {
                g.count.value = value;
                if n == 9
                    && let Some(pins) = &self.focus_capture
                {
                    g.capture_origin = pins.lock().unwrap().focus_pulses;
                }
            }
            if w == 1 {
                g.count.reload = value;
            }
            g.count.generation += 1;
            self.schedule_general(cx, n);
            self.publish_pwm();
            return;
        }
        if self.extended {
            cx.unmodeled();
            return;
        }
        if offset == 0x300 {
            // High half selects which low-half enable bits to update (1 shifted by channel).
            for n in 0..16 {
                if value & (1 << (16 + n)) == 0 {
                    continue;
                }
                let c = &mut self.counters[n];
                let enable = value & (1 << n) != 0;
                if enable != c.enabled {
                    c.value = c.current(Duration::from_nanos(cx.now()));
                    c.started_ns = Duration::from_nanos(cx.now()).as_nanos() as u64;
                    c.enabled = enable;
                    c.generation += 1;
                    self.schedule(cx, n);
                }
            }
        } else if let Some((n, reload)) = Self::channel(offset) {
            let c = &mut self.counters[n];
            let now = Duration::from_nanos(cx.now());
            c.value = c.current(now);
            c.started_ns = now.as_nanos() as u64;
            if reload {
                c.reload = value;
            } else {
                c.value = value;
            }
            c.generation += 1;
            self.schedule(cx, n);
        } else {
            cx.unmodeled();
        }
    }
}
