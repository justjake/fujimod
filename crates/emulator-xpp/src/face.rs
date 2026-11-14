//! X-Processor Pro face detection engine at 0xfff71000.
//!
//! X-T20 2.11 section 6 drives it from task face (0x1861000..0x186c000). Its
//! stop 0x18610b0 writes 0 to +0x00 and waits until +0x00 reads non-zero; its
//! start and stop paths (0x186bcd0, 0x186bf20) wait until +0x818 reads
//! non-zero. A run is programmed through the words +0x08..+0x838 and started
//! with bit 0 of +0x04. Descriptor row 0x34 of 0xed471c connects the engine to
//! interrupt 124 (level); the handler 0x18659b8 writes 0x11111 to the status
//! +0x834 (0x1861e88) and calls the registered callback.
//!
//! Each start finishes one run after the declared run ticks. It sets status
//! bit 0, and the interrupt line is high while a status bit is set; writing a
//! status bit clears it. The run finds nothing: the result words read 0 or
//! their last write. +0x00 bit 0 always reads set, as no run holds the engine
//! busy. The image is not read.
//!
//! +0x818 holds one ready flag per channel: bit 0 of nibbles 0-2 for the
//! channels whose control words are +0x800..+0x808, and of nibbles 4-6 for
//! +0x80c..+0x814. X-T2 4.41 0x18d9f48 clears the enable bit 28 of all six
//! channel words (0x18dce30) and then waits until +0x818 reads exactly
//! 0x01110111; the per-channel start and stop paths of both bodies (X-T2
//! 0x18dcc4c/0x18dce30, X-T20 0x186bcd0/0x186beb4) wait until it reads
//! non-zero. No channel is ever busy in this model, so all six flags read set.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Register window size in bytes.
pub const WINDOW: u64 = 0x1000;
const CONTROL: u64 = 0x000;
const START: u64 = 0x004;
const READY: u64 = 0x818;
const STATUS: u64 = 0x834;
const DONE: u32 = 1;
/// Every channel's ready flag (+0x818).
const CHANNELS_READY: u32 = 0x0111_0111;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    status: u32,
    due: Option<u64>,
    line: bool,
    runs: u64,
}
pub struct Engine {
    base: u64,
    irq: u32,
    run_ticks: u64,
    state: State,
}
impl Engine {
    pub fn new(base: u64, irq: u32, run_ticks: u64) -> Result<Self, String> {
        if base & 0xfff != 0 || !(32..512).contains(&irq) || run_ticks == 0 {
            return Err("Invalid X-Processor Pro face detection engine".into());
        }
        Ok(Self {
            base,
            irq,
            run_ticks,
            state: State::default(),
        })
    }
    fn update_line(&mut self, cx: &mut Cx<'_>) {
        let line = self.state.status != 0;
        if line != self.state.line {
            self.state.line = line;
            cx.set_irq(self.irq, line);
        }
    }
}
impl Device for Engine {
    fn name(&self) -> &'static str {
        "xpp-face-detection"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let o = address - self.base;
        if size != 4 || o & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        let word = self.state.words.get(&o).copied().unwrap_or(0);
        u64::from(match o {
            CONTROL => word | 1,
            READY => CHANNELS_READY,
            STATUS => self.state.status,
            _ => word,
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let o = address - self.base;
        let Ok(value) = u32::try_from(value) else {
            return cx.unmodeled();
        };
        if size != 4 || o & 3 != 0 {
            return cx.unmodeled();
        }
        if o == STATUS {
            self.state.status &= !value;
            return self.update_line(cx);
        }
        if value == 0 {
            self.state.words.remove(&o);
        } else {
            self.state.words.insert(o, value);
        }
        if o == START && value & 1 != 0 && self.state.due.is_none() {
            self.state.due = cx.now().checked_add(self.run_ticks);
            if self.state.due.is_none() {
                cx.fail("Face detection deadline overflow");
            }
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.state.due.is_some_and(|at| at <= cx.now()) {
            self.state.due = None;
            self.state.runs += 1;
            self.state.status |= DONE;
            self.update_line(cx);
        }
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.due
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.check(&state)?;
        self.state = state;
        Ok(())
    }
    fujimod_emulator_devices::checkpoint_state!(state: State, Self::check);
}
impl Engine {
    fn check(&self, state: &State) -> Result<(), String> {
        if state
            .words
            .iter()
            .any(|(&o, &v)| o & 3 != 0 || o >= WINDOW || o == STATUS || v == 0)
            || state.line != (state.status != 0)
        {
            return Err("Invalid face detection checkpoint".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};

    const BASE: u64 = 0xfff71000;

    #[test]
    fn a_started_run_finds_nothing_and_holds_its_level_interrupt() {
        assert!(Engine::new(BASE, 124, 0).is_err());
        let mut engine = Engine::new(BASE, 124, 100).unwrap();
        let mut h = TestHost::new();
        // 0x18610b0: stop and wait for +0x00; 0x186bcd0 waits for +0x818.
        write(&mut engine, &mut h, BASE, 0);
        assert_eq!(read(&mut engine, &mut h, BASE), 1);
        assert_eq!(read(&mut engine, &mut h, BASE + READY), 0x0111_0111);
        write(&mut engine, &mut h, BASE + 0x48, 0x27f_01df);
        write(&mut engine, &mut h, BASE + START, 1);
        assert_eq!(engine.next_deadline(0), Some(100));
        let saved = engine.save();
        advance(&mut engine, &mut h, 100);
        assert_eq!(read(&mut engine, &mut h, BASE + STATUS), u64::from(DONE));
        assert!(engine.state.line);
        assert_eq!(read(&mut engine, &mut h, BASE + 0x200), 0);
        // 0x1861e88 acknowledges with 0x11111.
        write(&mut engine, &mut h, BASE + STATUS, 0x11111);
        assert!(!engine.state.line);
        assert_eq!(engine.next_deadline(100), None);
        engine.restore(&saved).unwrap();
        assert_eq!(engine.next_deadline(0), Some(100));
        assert_eq!(read(&mut engine, &mut h, BASE + 0x48), 0x27f_01df);
        assert!(
            engine
                .restore(&serde_json::json!({"words": [], "status": 1, "due": null, "line": false, "runs": 0}))
                .is_err()
        );
    }

    #[test]
    fn stopping_every_channel_reads_all_six_ready_flags() {
        let mut engine = Engine::new(BASE, 124, 100).unwrap();
        let mut h = TestHost::new();
        // X-T2 4.41 0x18dce30 clears bit 28 of each channel word, then
        // 0x18d9f48 waits for +0x818 == 0x01110111.
        for channel in [0x800, 0x804, 0x808, 0x80c, 0x810, 0x814] {
            write(&mut engine, &mut h, BASE + channel, 0x1000_0001);
            assert_ne!(read(&mut engine, &mut h, BASE + READY), 0);
            write(&mut engine, &mut h, BASE + channel, 1);
        }
        assert_eq!(read(&mut engine, &mut h, BASE + READY), 0x0111_0111);
    }
}
