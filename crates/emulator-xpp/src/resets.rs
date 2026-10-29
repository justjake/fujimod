//! X-Processor Pro reset and gate controls at 0xfff0a000.
//!
//! Words read back their last write. The X-T20 2.11 SD start-up 0xc65b18
//! drives +0x1c bit 18, 19 or 20 low and then high for SD channel 0, 1 or 2
//! and then sets bit 0. A falling edge of a channel bit is a hardware reset
//! pulse for that SD host: the UHS-II fallback at 0xc65dc4 runs it before it
//! rewrites power and clock words by read-modify-write, which relies on the
//! cleared UHS-II settings. 0xc65a04/0xc65e58 set and clear +0x14 bit 22
//! around the UHS-II attempt; 0xc621d0/0xc620b8 set and clear pad bits in
//! +0x70 and +0x88. The section 6 USB start-up 0x15274f0 sets +0x100 bit 5,
//! clears +0x118 bit 13 and +0x110 bit 2, then drives +0x10c bit 0 low and
//! high again before it programs the DWC3. What those bits gate is not
//! modeled.
//!
//! +0x08 holds the polarity of the sixteen external interrupt lines: bit n
//! set inverts line n (0xc694d4). Line n is interrupt 151 + n (0xc5cfd4 maps
//! interrupt rows to lines). [`ExternalLines`] drives the interrupt of each
//! connected line at its pin level, inverted by its polarity bit, whenever
//! the polarity word is written or a connected part changes its pin. Level
//! or edge sensitivity is the interrupt controller's configuration. Lines
//! without a connected part stay low.
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_ip::socionext::gpio::SharedPins;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Register window size in bytes.
pub const WINDOW: u64 = 0x1000;
const WORDS: [u64; 12] = [
    0x08, 0x14, 0x1c, 0x20, 0x6c, 0x70, 0x88, 0x9c, 0x100, 0x10c, 0x110, 0x118,
];
const SD_RESETS: u64 = 0x1c;
const SD_CHANNELS: usize = 3;
const SD_FIRST_BIT: u32 = 18;
const POLARITY: u64 = 0x08;
/// Interrupt of external line 0.
pub const FIRST_LINE_IRQ: u32 = 151;
/// External interrupt lines.
pub const LINES: usize = 16;

struct Connection {
    line: usize,
    port: usize,
    pin: usize,
}
#[derive(Default)]
struct Lines {
    pins: Option<SharedPins>,
    connections: Vec<Connection>,
    polarity: u32,
    levels: u32,
}
/// External interrupt lines with parts connected to GPIO pins.
#[derive(Clone, Default)]
pub struct ExternalLines(Arc<Mutex<Lines>>);
impl ExternalLines {
    /// Connect line `line` to GPIO `port`/`pin` of `pins`.
    pub fn connect(
        &self,
        pins: SharedPins,
        line: usize,
        port: usize,
        pin: usize,
    ) -> Result<(), String> {
        let mut lines = self.0.lock().map_err(|_| "External lines poisoned")?;
        if line >= LINES
            || port >= 16
            || pin >= 16
            || lines.connections.iter().any(|c| c.line == line)
            || lines.pins.as_ref().is_some_and(|p| !Arc::ptr_eq(p, &pins))
        {
            return Err("Invalid external interrupt connection".into());
        }
        lines.pins = Some(pins);
        lines.connections.push(Connection { line, port, pin });
        Ok(())
    }
    /// Drive each connected line's interrupt at its current level.
    pub fn update(&self, cx: &mut Cx<'_>) {
        let mut lines = self.0.lock().unwrap();
        let Some(pins) = lines.pins.clone() else {
            return;
        };
        let pins = pins.lock().unwrap();
        let mut levels = lines.levels;
        for c in &lines.connections {
            let high = (pins.input(c.port, c.pin) as u32 ^ lines.polarity >> c.line) & 1 != 0;
            let bit = 1 << c.line;
            if high != (levels & bit != 0) {
                levels ^= bit;
                cx.set_irq(FIRST_LINE_IRQ + c.line as u32, high);
            }
        }
        lines.levels = levels;
    }
    fn set_polarity(&self, cx: &mut Cx<'_>, polarity: u32) {
        self.0.lock().unwrap().polarity = polarity;
        self.update(cx);
    }
    fn levels(&self) -> u32 {
        self.0.lock().unwrap().levels
    }
    fn restore(&self, polarity: u32, levels: u32) {
        let mut lines = self.0.lock().unwrap();
        lines.polarity = polarity;
        lines.levels = levels;
    }
}

/// Count of reset pulses on one line; a receiver resets once per new pulse.
#[derive(Clone, Default)]
pub struct ResetPulses(Arc<AtomicU64>);
impl ResetPulses {
    pub fn count(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
    fn set(&self, count: u64) {
        self.0.store(count, Ordering::Relaxed);
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    pulses: [u64; SD_CHANNELS],
    #[serde(default, skip_serializing_if = "is_zero")]
    lines: u32,
}
fn is_zero(levels: &u32) -> bool {
    *levels == 0
}
pub struct Gates {
    base: u64,
    words: BTreeMap<u64, u32>,
    sd: [ResetPulses; SD_CHANNELS],
    lines: ExternalLines,
}
impl Gates {
    pub fn new(base: u64) -> Result<Self, String> {
        if base & 0xfff != 0 {
            return Err("Invalid X-Processor Pro reset/gate base".into());
        }
        Ok(Self {
            base,
            words: BTreeMap::new(),
            sd: Default::default(),
            lines: ExternalLines::default(),
        })
    }
    /// The external interrupt lines whose polarity this block holds.
    pub fn external_lines(&self) -> ExternalLines {
        self.lines.clone()
    }
    /// The reset line of SD channel `channel`.
    pub fn sd_reset(&self, channel: usize) -> Result<ResetPulses, String> {
        self.sd
            .get(channel)
            .cloned()
            .ok_or_else(|| "Invalid SD reset channel".into())
    }
    fn offset(&self, address: u64, size: u8) -> Option<u64> {
        let offset = address.checked_sub(self.base)?;
        (size == 4 && WORDS.contains(&offset)).then_some(offset)
    }
}
impl Device for Gates {
    fn name(&self) -> &'static str {
        "xpp-reset-gates"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match self.offset(address, size) {
            Some(offset) => self.words.get(&offset).copied().unwrap_or(0).into(),
            None => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return;
        };
        let value = value as u32;
        if offset == SD_RESETS {
            let old = self.words.get(&offset).copied().unwrap_or(0);
            for (channel, line) in self.sd.iter().enumerate() {
                let bit = 1 << (SD_FIRST_BIT + channel as u32);
                if old & bit != 0 && value & bit == 0 {
                    line.set(line.count() + 1);
                }
            }
        }
        self.words.insert(offset, value);
        if offset == POLARITY {
            self.lines.set_polarity(cx, value);
        }
    }
    fn save(&self) -> Value {
        serde_json::to_value(State {
            words: self.words.clone(),
            pulses: std::array::from_fn(|channel| self.sd[channel].count()),
            lines: self.lines.levels(),
        })
        .unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state.words.keys().any(|offset| !WORDS.contains(offset)) {
            return Err("Invalid reset/gate checkpoint".into());
        }
        let polarity = state.words.get(&POLARITY).copied().unwrap_or(0);
        self.lines.restore(polarity, state.lines);
        self.words = state.words;
        for (line, count) in self.sd.iter().zip(state.pulses) {
            line.set(count);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, read, write};

    const BASE: u64 = 0xfff0a000;

    #[test]
    fn falling_sd_bits_pulse_their_channel_and_words_read_back() {
        let mut gates = Gates::new(BASE).unwrap();
        let mut h = TestHost::new();
        let line = gates.sd_reset(0).unwrap();
        // 0xc65b50: clear bit 18 (already low), set it, then set bit 0.
        for value in [0, 0x40000, 0x40001] {
            write(&mut gates, &mut h, BASE + 0x1c, value);
        }
        assert_eq!(line.count(), 0);
        assert_eq!(read(&mut gates, &mut h, BASE + 0x1c), 0x40001);
        for value in [0x1, 0x40001] {
            write(&mut gates, &mut h, BASE + 0x1c, value);
        }
        assert_eq!(line.count(), 1);
        assert_eq!(gates.sd_reset(1).unwrap().count(), 0);
        write(&mut gates, &mut h, BASE + 0x14, 0x400000);
        let saved = gates.save();
        write(&mut gates, &mut h, BASE + 0x1c, 0);
        assert_eq!(line.count(), 2);
        gates.restore(&saved).unwrap();
        assert_eq!(line.count(), 1);
        assert_eq!(read(&mut gates, &mut h, BASE + 0x14), 0x400000);
        assert!(gates.sd_reset(3).is_err());
    }

    #[test]
    fn connected_lines_follow_their_pin_and_polarity() {
        use fujimod_emulator_ip::socionext::gpio::Pins;
        let mut gates = Gates::new(BASE).unwrap();
        let lines = gates.external_lines();
        let pins = Pins::new([0; 16]);
        pins.lock().unwrap().set_input(14, 1, true);
        lines.connect(pins.clone(), 15, 14, 1).unwrap();
        assert!(lines.connect(pins.clone(), 15, 14, 2).is_err());
        let mut h = TestHost::new();
        // 0xc7b118: line 15 inverted, waiting for the pin to go low.
        write(&mut gates, &mut h, BASE + 0x08, 1 << 15);
        assert_eq!(gates.external_lines().levels(), 0);
        pins.lock().unwrap().set_input(14, 1, false);
        lines.update(&mut h.cx());
        assert_eq!(lines.levels(), 1 << 15);
        let saved = gates.save();
        // 0xc7b14c: not inverted, waiting for the pin to rise.
        write(&mut gates, &mut h, BASE + 0x08, 0);
        assert_eq!(lines.levels(), 0);
        gates.restore(&saved).unwrap();
        assert_eq!(lines.levels(), 1 << 15);
        assert_eq!(read(&mut gates, &mut h, BASE + 0x08), 1 << 15);
    }
}
