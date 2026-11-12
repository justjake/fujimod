//! X-Processor 4 bank of sixteen countdown timers, each with its own interrupt line.
//!
//! Register layout, from the X-T3 5.11 kernel helpers that program it:
//!
//! - `+0x000` enable word: a write changes channel `n` only when bit `n + 16` is
//!   set, and then starts it when bit `n` is set and stops it when clear
//!   (the kernel's start writes `0x10001 << n`, its stop writes
//!   `0x10000 << n`). A read returns the running channels.
//! - `+0x010 + 4n` channel control. Every configuration the kernel's timer
//!   setup writes has bit 16 set, and the kernel tests it before it clears
//!   bits 12..13; this model takes bit 16 as the channel's
//!   counting interrupt mode. The other bits (one-shot bit 4, mode field
//!   12..13, bit 14) are stored and read back, not decoded. The stock boot also
//!   runs channels 2 and 4 with control `0x100d` (bit 16 clear) and a count of
//!   `0x8000_0000`; their counting rate in that configuration is not decoded,
//!   so their count and reload words read back their last write.
//! - `+0x100 + 0x10n` count, `+0x104 + 0x10n` reload, `+0x108`/`+0x10c` two
//!   words the kernel writes as zero (its channel reset writes all four).
//!
//! A running channel counts board ticks: it expires `count + 1` ticks after it
//! is started or loaded and then every `reload + 1` ticks. The kernel converts
//! microseconds at 54 MHz (`us * 54_000_000 / 1_000_000 - 1`), the
//! board tick rate. On each expiry a channel whose interrupt bit is set pulses
//! its routed line; the distributor latches the edge. The stock interrupt
//! handler stops the channel and calls the callback, so nothing
//! acknowledges the timer itself. That the line is an edge is inferred from
//! the missing acknowledge (the kernel's interrupt rows for these lines use
//! mode 3), not measured.
use crate::interval_timers::Counter;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CHANNELS: usize = 16;
const CONTROL: u64 = 0x10;
const COUNTERS: u64 = 0x100;
const INTERRUPT_ENABLE: u32 = 1 << 16;
fn decoded(control: u32) -> bool {
    control & INTERRUPT_ENABLE != 0
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Channel {
    counter: Counter,
    control: u32,
    extra: [u32; 2],
}

pub struct CountdownTimers {
    base: u64,
    lines: [Option<u32>; CHANNELS],
    channels: [Channel; CHANNELS],
}

impl CountdownTimers {
    /// `routes` pairs a channel with its interrupt line.
    pub fn new(base: u64, routes: &[(usize, u32)]) -> Result<Self, String> {
        if base & 0xfff != 0 || base > u32::MAX as u64 - 0x200 {
            return Err("Invalid XP4 countdown timer base".into());
        }
        let mut lines = [None; CHANNELS];
        for &(channel, line) in routes {
            if channel >= CHANNELS || lines[channel].is_some() || !(32..1020).contains(&line) {
                return Err("Invalid XP4 countdown timer route".into());
            }
            lines[channel] = Some(line);
        }
        Ok(Self {
            base,
            lines,
            channels: Default::default(),
        })
    }
    /// The register windows this bank owns, as `[start, end)` pairs.
    pub fn windows(&self) -> [(u64, u64); 2] {
        [
            (self.base, self.base + CONTROL + 4 * CHANNELS as u64),
            (self.base + COUNTERS, self.base + COUNTERS + 0x10 * CHANNELS as u64),
        ]
    }
    fn interrupting(&self, n: usize) -> bool {
        self.lines[n].is_some() && decoded(self.channels[n].control)
    }
    /// Expire the channels due by now and pulse the lines of those that interrupt.
    fn expire(&mut self, cx: &mut Cx<'_>) {
        let now = cx.now();
        for n in 0..CHANNELS {
            if self.channels[n].counter.expire(now)
                && self.interrupting(n)
                && let Some(line) = self.lines[n]
            {
                cx.set_irq(line, true);
                cx.set_irq(line, false);
            }
        }
    }
}

impl Device for CountdownTimers {
    fn name(&self) -> &'static str {
        "xp4-countdown-timers"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.expire(cx);
        let now = cx.now();
        let o = address.wrapping_sub(self.base);
        if size != 4 || o & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        u64::from(match o {
            0 => self
                .channels
                .iter()
                .enumerate()
                .fold(0, |bits, (n, c)| bits | (u32::from(c.counter.enabled) << n)),
            CONTROL..0x50 => self.channels[((o - CONTROL) / 4) as usize].control,
            COUNTERS..0x200 => {
                let c = &self.channels[((o - COUNTERS) / 0x10) as usize];
                match o & 0xc {
                    0 if decoded(c.control) => c.counter.value(now),
                    0 => c.counter.value,
                    4 => c.counter.reload,
                    8 => c.extra[0],
                    _ => c.extra[1],
                }
            }
            _ => {
                cx.unmodeled();
                0
            }
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.expire(cx);
        let now = cx.now();
        let o = address.wrapping_sub(self.base);
        if size != 4 || o & 3 != 0 {
            cx.unmodeled();
            return;
        }
        let value = value as u32;
        match o {
            0 => {
                for (n, c) in self.channels.iter_mut().enumerate() {
                    if value & (1 << (n + 16)) != 0 {
                        let on = value & (1 << n) != 0;
                        if decoded(c.control) {
                            c.counter.enable(now, on);
                        } else {
                            c.counter.enabled = on;
                        }
                    }
                }
            }
            CONTROL..0x50 => {
                let c = &mut self.channels[((o - CONTROL) / 4) as usize];
                if c.counter.enabled && decoded(c.control) != decoded(value) {
                    // Switching a running channel between the counting and the
                    // undecoded configuration has no modeled meaning.
                    cx.unmodeled();
                    return;
                }
                c.control = value;
            }
            COUNTERS..0x200 => {
                let c = &mut self.channels[((o - COUNTERS) / 0x10) as usize];
                match o & 0xc {
                    0 if decoded(c.control) => c.counter.load(now, value),
                    0 => c.counter.value = value,
                    4 => c.counter.reload = value,
                    8 => c.extra[0] = value,
                    _ => c.extra[1] = value,
                }
            }
            _ => cx.unmodeled(),
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        self.expire(cx);
    }
    fn event(&mut self, cx: &mut Cx<'_>, _token: u64) {
        self.expire(cx);
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        (0..CHANNELS)
            .filter(|&n| self.interrupting(n))
            .filter_map(|n| self.channels[n].counter.next)
            .min()
    }
    fn save(&self) -> Value {
        serde_json::json!({ "base": self.base, "lines": self.lines, "channels": self.channels })
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let invalid = || "Invalid XP4 countdown timer checkpoint".to_string();
        if value.get("base").and_then(Value::as_u64) != Some(self.base)
            || value.get("lines") != Some(&serde_json::json!(self.lines))
        {
            return Err(invalid());
        }
        let channels: [Channel; CHANNELS] =
            serde_json::from_value(value.get("channels").cloned().ok_or_else(invalid)?)
                .map_err(|_| invalid())?;
        if channels
            .iter()
            .any(|c| (c.counter.enabled && decoded(c.control)) != c.counter.next.is_some())
        {
            return Err(invalid());
        }
        self.channels = channels;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::Host;

    #[derive(Default)]
    struct Lines {
        now: u64,
        edges: Vec<(u32, bool)>,
    }
    impl Host for Lines {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {}
        fn set_irq(&mut self, line: u32, level: bool) {
            self.edges.push((line, level));
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            Err("no ram".into())
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            Err("no ram".into())
        }
        fn record(&mut self, _: fujimod_emulator_devices::Record) {}
    }
    const BASE: u64 = 0xfff3c000;
    fn write(t: &mut CountdownTimers, host: &mut Lines, offset: u64, value: u32) {
        let mut cx = Cx::new(host, 0, 0, 0);
        t.write(&mut cx, BASE + offset, 4, u64::from(value));
        assert!(cx.refusal().is_none());
    }
    fn read(t: &mut CountdownTimers, host: &mut Lines, offset: u64) -> u32 {
        let mut cx = Cx::new(host, 0, 0, 0);
        let value = t.read(&mut cx, BASE + offset, 4) as u32;
        assert!(cx.refusal().is_none());
        value
    }

    /// The X-T3 kernel's callback timer: 5000 us on channel 6 (0x41eb0 ticks).
    #[test]
    fn a_callback_channel_pulses_its_line_once_per_period_until_stopped() {
        let mut t = CountdownTimers::new(BASE, &[(6, 47)]).unwrap();
        let mut host = Lines { now: 1000, ..Default::default() };
        write(&mut t, &mut host, 0x28, 0x10000);
        write(&mut t, &mut host, 0x160, 270000);
        write(&mut t, &mut host, 0x164, 269999);
        write(&mut t, &mut host, 0x168, 0);
        write(&mut t, &mut host, 0x16c, 0);
        assert_eq!(t.next_deadline(1000), None);
        write(&mut t, &mut host, 0, 0x10001 << 6);
        assert_eq!(read(&mut t, &mut host, 0), 1 << 6);
        assert_eq!(t.next_deadline(1000), Some(1000 + 270001));
        host.now = 1000 + 270000;
        t.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert!(host.edges.is_empty());
        assert_eq!(read(&mut t, &mut host, 0x160), 0);
        host.now += 1;
        t.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(host.edges, vec![(47, true), (47, false)]);
        assert_eq!(t.next_deadline(host.now), Some(host.now + 270000));
        let saved = t.save();
        // The stock handler stops the channel; nothing else acknowledges it.
        write(&mut t, &mut host, 0, 0x10000 << 6);
        assert_eq!(read(&mut t, &mut host, 0), 0);
        assert_eq!(t.next_deadline(host.now), None);
        let mut resumed = CountdownTimers::new(BASE, &[(6, 47)]).unwrap();
        resumed.restore(&saved).unwrap();
        assert_eq!(resumed.next_deadline(host.now), Some(host.now + 270000));
        assert!(CountdownTimers::new(BASE, &[(6, 48)]).unwrap().restore(&saved).is_err());
    }

    #[test]
    fn a_channel_in_the_undecoded_configuration_keeps_its_last_writes() {
        let mut t = CountdownTimers::new(BASE, &[(4, 45)]).unwrap();
        let mut host = Lines::default();
        write(&mut t, &mut host, 0x20, 0x100d);
        write(&mut t, &mut host, 0x140, 0x8000_0000);
        write(&mut t, &mut host, 0x144, 0x8000_0000);
        write(&mut t, &mut host, 0, 0x100010);
        assert_eq!(read(&mut t, &mut host, 0x20), 0x100d);
        assert_eq!(t.next_deadline(0), None);
        host.now = 100;
        assert_eq!(read(&mut t, &mut host, 0), 1 << 4);
        assert_eq!(read(&mut t, &mut host, 0x140), 0x8000_0000);
        assert!(host.edges.is_empty());
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        t.write(&mut cx, BASE + 0x20, 4, 0x10000);
        assert!(cx.refusal().is_some());
    }

    #[test]
    fn words_outside_the_bank_and_bad_routes_are_refused() {
        let mut t = CountdownTimers::new(BASE, &[]).unwrap();
        let mut host = Lines::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        t.read(&mut cx, BASE + 0x80, 4);
        assert!(cx.refusal().is_some());
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        t.write(&mut cx, BASE + 0x161, 1, 0);
        assert!(cx.refusal().is_some());
        assert!(CountdownTimers::new(BASE, &[(16, 47)]).is_err());
        assert!(CountdownTimers::new(BASE, &[(1, 47), (1, 48)]).is_err());
        assert_eq!(
            CountdownTimers::new(BASE, &[]).unwrap().windows(),
            [(BASE, BASE + 0x50), (BASE + 0x100, BASE + 0x200)]
        );
    }
}
