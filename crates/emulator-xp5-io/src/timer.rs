//! X100VI 48 MHz general/PWM and reload timers (ff908000), plus six
//! extended general channels (ff909000). Finder logical17 uses general6/IRQ42.
//! External capture inputs are not synthesized.
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
        // Boot enables some unconfigured PWM channels with both period words
        // zero. They have no programmed terminal interval; do not synthesize
        // a 48 MHz interrupt storm that starves guest register programming.
        if g.count.enabled && (g.words[0] != 0 || g.words[1] != 0) {
            let token = (1 << 63) | (g.count.generation << 8) | n as u64;
            // Fast PWM interrupt edges are coalesced to the declared 1ms output
            // sampling interval instead of trapping every physical pulse.
            // Normal interval timers, including finder6, retain their deadline.
            let now = Duration::from_nanos(cx.now());
            let floor = if g.control & 0x1000 != 0 && g.count.reload < 47999 {
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
    pub fn with_focus_capture(mut self, pins: fujimod_emulator_devices::xp5::pins::SharedPins) -> Self {
        self.focus_capture = Some(pins);
        self
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
            if n == 9 && w == 0 && self.general[n].control & 3 == 3 {
                if let Some(pins) = &self.focus_capture {
                    // Logical18/selector33 -> general9. The timer scales
                    // the captured low13 bits by8 to compare with lens steps.
                    let pulses = pins.lock().unwrap().focus_pulses;
                    return self.general[n].words[0].wrapping_add(
                        (pulses.wrapping_sub(self.general[n].capture_origin) / 8) as u32,
                    ) as u64;
                }
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
            let g = &mut self.general[n];
            g.count.value = g.count.current(Duration::from_nanos(cx.now()));
            g.count.started_ns = Duration::from_nanos(cx.now()).as_nanos() as u64;
            g.words[w] = value;
            if w == 0 {
                g.count.value = value;
                if n == 9 {
                    if let Some(pins) = &self.focus_capture {
                        g.capture_origin = pins.lock().unwrap().focus_pulses;
                    }
                }
            }
            if w == 1 {
                g.count.reload = value;
            }
            g.count.generation += 1;
            self.schedule_general(cx, n);
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
        if n >= 8 {
            if let Some(cpu) = self.companion_cpu {
                if let Err(e) = cx.raise_irq(cpu, (n + 1) as u32) {
                    cx.fail(e);
                }
            }
        }
        self.schedule(cx, n);
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
        *self = saved;
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
