//! X-Processor Pro imaging statistics engine at 0xfff54000.
//!
//! X-T20 2.11 section 5 programs the configuration and output buffer words
//! +0x04..+0x1e4, enables interrupts with the low ten bits of +0x04 and
//! starts the engine with bit 28 of +0x1e8. The level handler 0xc36280 reads
//! the status +0x08 (bits 0..9) and writes it back to clear it. The stop
//! sequence 0xc365a8 clears +0x00 bit 0, waits until +0x00 reads exactly 1,
//! then until +0x1ec bit 16 (idle) reads set, and clears the start bit.
//!
//! While started, the engine finishes a frame at every frame tick, counted
//! from tick zero with the frame sync's declared period, and sets every
//! enabled status bit; the interrupt line is high while an enabled status bit
//! is set. No frame is ever in flight, so +0x1ec reads only the idle bit and
//! +0x00 bit 0 reads set. The statistics are not computed and the buffers
//! are not written. Other words read back their last write.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Register window size in bytes.
pub const WINDOW: u64 = 0x200;
const RUN: u32 = 1;
const ENABLE: u64 = 0x04;
const STATUS: u64 = 0x08;
const CONTROL: u64 = 0x1e8;
const STATE: u64 = 0x1ec;
const SOURCES: u32 = 0x3ff;
const START: u32 = 1 << 28;
const IDLE: u32 = 1 << 16;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    status: u32,
    due: Option<u64>,
    line: bool,
    frames: u64,
}
pub struct Engine {
    base: u64,
    irq: u32,
    period: u64,
    state: State,
}
impl Engine {
    pub fn new(base: u64, irq: u32, period: u64) -> Result<Self, String> {
        if base & 0xfff != 0 || !(32..512).contains(&irq) || period == 0 {
            return Err("Invalid X-Processor Pro statistics engine".into());
        }
        Ok(Self {
            base,
            irq,
            period,
            state: State::default(),
        })
    }
    fn word(&self, offset: u64) -> u32 {
        self.state.words.get(&offset).copied().unwrap_or(0)
    }
    fn update_line(&mut self, cx: &mut Cx<'_>) {
        let level = self.state.status & self.word(ENABLE) & SOURCES != 0;
        if level != self.state.line {
            self.state.line = level;
            cx.set_irq(self.irq, level);
        }
    }
    fn offset(&self, address: u64, size: u8) -> Option<u64> {
        let offset = address.checked_sub(self.base)?;
        (size == 4 && offset & 3 == 0 && offset < WINDOW).then_some(offset)
    }
}
impl Device for Engine {
    fn name(&self) -> &'static str {
        "xpp-imaging-statistics"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        u64::from(match self.offset(address, size) {
            Some(0) => self.word(0) | RUN,
            Some(STATUS) => self.state.status,
            Some(STATE) => IDLE,
            Some(o) if o < STATE => self.word(o),
            _ => {
                cx.unmodeled();
                0
            }
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Ok(value) = u32::try_from(value) else {
            return cx.unmodeled();
        };
        match self.offset(address, size) {
            Some(STATUS) => self.state.status &= !value,
            Some(CONTROL) => {
                self.state.due = match (value & START != 0, self.state.due) {
                    (false, _) => None,
                    (true, Some(at)) => Some(at),
                    (true, None) => {
                        let now = cx.now();
                        (now / self.period)
                            .checked_add(1)
                            .and_then(|n| n.checked_mul(self.period))
                    }
                };
                self.state.words.insert(CONTROL, value);
            }
            Some(o) if o < STATE => {
                self.state.words.insert(o, value);
            }
            _ => return cx.unmodeled(),
        }
        self.update_line(cx);
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let Some(due) = self.state.due.filter(|at| *at <= cx.now()) else {
            return;
        };
        self.state.status |= self.word(ENABLE) & SOURCES;
        self.state.frames += 1;
        let frames = (cx.now() - due) / self.period + 1;
        self.state.due = frames
            .checked_mul(self.period)
            .and_then(|step| due.checked_add(step));
        if self.state.due.is_none() {
            cx.fail("Statistics engine deadline overflow");
        }
        self.update_line(cx);
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.due
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        let started = state.words.get(&CONTROL).is_some_and(|w| w & START != 0);
        if started != state.due.is_some()
            || state.due.is_some_and(|at| at % self.period != 0)
            || state.words.keys().any(|o| o & 3 != 0 || *o >= STATE)
        {
            return Err("Invalid statistics engine checkpoint".into());
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};

    const BASE: u64 = 0xfff54000;

    #[test]
    fn started_engine_completes_at_frame_ticks_and_stops_at_once() {
        let mut stats = Engine::new(BASE, 106, 1000).unwrap();
        let mut h = TestHost::new();
        write(&mut stats, &mut h, BASE + 0x1b8, 0x3f48_0000);
        write(&mut stats, &mut h, BASE + ENABLE, 0x3ff_03ff);
        h.now = 1500;
        write(&mut stats, &mut h, BASE + CONTROL, START);
        assert_eq!(stats.next_deadline(0), Some(2000));
        advance(&mut stats, &mut h, 1999);
        assert_eq!(read(&mut stats, &mut h, BASE + STATUS), 0);
        advance(&mut stats, &mut h, 2000);
        assert_eq!(read(&mut stats, &mut h, BASE + STATUS), 0x3ff);
        assert!(stats.state.line);
        // 0xc36280: write the status back to clear it.
        write(&mut stats, &mut h, BASE + STATUS, 0x3ff);
        assert_eq!(read(&mut stats, &mut h, BASE + STATUS), 0);
        assert!(!stats.state.line);
        let saved = stats.save();
        advance(&mut stats, &mut h, 4500);
        assert_eq!(stats.state.frames, 2);
        stats.restore(&saved).unwrap();
        assert_eq!(stats.next_deadline(0), Some(3000));
        // 0xc365a8: stop, then the handshakes.
        write(&mut stats, &mut h, BASE, 0);
        assert_eq!(read(&mut stats, &mut h, BASE), 1);
        assert_eq!(read(&mut stats, &mut h, BASE + STATE) & 0x10000, 0x10000);
        write(&mut stats, &mut h, BASE + CONTROL, 0);
        assert_eq!(stats.next_deadline(0), None);
        assert_eq!(read(&mut stats, &mut h, BASE + 0x1b8), 0x3f48_0000);
        let mut cx = h.cx();
        stats.write(&mut cx, BASE + STATE, 4, 0);
        assert!(cx.is_unmodeled());
    }
}
