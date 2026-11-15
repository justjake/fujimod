//! XP4 filter engine at 0xfffb9000: the register contract of the X-T3 5.11
//! driver.
//!
//! The same driver runs the block for the still step TCUBEPR and for the
//! face detection picture. The engine copies a picture from a source to a
//! destination buffer through two symmetric filters: the +0x34 taps
//! satisfy t4 + 2 (t1 + t2 + t3) = 256 and the +0x30 taps
//! t3 + 2 (t1 + t2) = 128.
//!
//! | Offset | Bits |
//! |---|---|
//! | +0x00 | 0 written clear to stop; reads set once stopped |
//! | +0x04 | 0 start |
//! | +0x08 | 16, 12, 8, 4, 0 |
//! | +0x10 | source (a multiple of 8) |
//! | +0x14 | 14..0 source pitch |
//! | +0x18 | height << 16, width (14 bits each) |
//! | +0x20 | destination (a multiple of 8) |
//! | +0x24 | 14..0 destination pitch |
//! | +0x28 | 9..0 |
//! | +0x30 | three 7-bit taps |
//! | +0x34 | four 8-bit taps |
//! | +0x38 | 9..0 (0x200) |
//! | +0x3c | 31, 28, 26..24, 23..16, 12, 10..8, 7..0 |
//! | +0x40 | read only; bits 0 and 16: idle |
//! | +0x44 | written 1 to clear; bit 0: run finished |
//! | +0x48 | 23..16, 7..0 |
//!
//! The memory type words 0xfff4a710 (source) and 0xfff4a714 (destination)
//! take bits 1..0. The driver's open registers its handler on interrupt
//! line 127; the handler reads +0x44, writes 1 to it and calls the job's
//! callback. A start sets +0x3c bits 28 and 12, then +0x04 bit 0;
//! the stop clears those +0x3c bits, waits for +0x40 bits 0 and
//! 16, stops the engine through +0x00 and clears +0x44.
//!
//! Modeled: the words read back their last write; a start finishes after
//! `run_ticks`, sets +0x44 bit 0 and raises the line until it is cleared.
//! The pictures are not read or written: the destination keeps its contents.
//! Only memory type 0 is accepted. The run time is not measured.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const WINDOW: u64 = 0x1000;
const CONTROL: u64 = 0x00;
const START: u64 = 0x04;
const IDLE: u64 = 0x40;
const FINISHED: u64 = 0x44;
/// Bits 0 and 16 of +0x40 read set while no run is in progress.
const IDLE_BITS: u32 = 0x1_0001;
/// Written words and the bits each writer can set.
const WORDS: &[(u64, u32)] = &[
    (0x08, 0x1_1111),
    (0x10, 0xffff_fff8),
    (0x14, 0x7fff),
    (0x18, 0x3fff_3fff),
    (0x20, 0xffff_fff8),
    (0x24, 0x7fff),
    (0x28, 0x3ff),
    (0x30, 0x7f_7f7f),
    (0x34, 0xffff_ffff),
    (0x38, 0x3ff),
    (0x3c, 0x97ff_17ff),
    (0x48, 0xff_00ff),
];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    finished: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    due: Option<u64>,
    runs: u64,
}

pub struct FilterEngine {
    base: u64,
    memory_types: u64,
    irq: u32,
    ticks: u64,
    state: State,
}

fn mask(offset: u64) -> Option<u32> {
    WORDS.iter().find(|&&(o, _)| o == offset).map(|&(_, m)| m)
}

impl FilterEngine {
    /// `memory_types` is the address of the source memory type word; the
    /// destination's follows it.
    pub fn new(base: u64, memory_types: u64, irq: u32, ticks: u64) -> Result<Self, String> {
        if base & 0xfff != 0
            || base > u64::from(u32::MAX)
            || memory_types & 3 != 0
            || (base..base + WINDOW).contains(&memory_types)
            || !(32..512).contains(&irq)
            || ticks == 0
        {
            return Err("Invalid XP4 filter engine".into());
        }
        Ok(Self {
            base,
            memory_types,
            irq,
            ticks,
            state: State::default(),
        })
    }
    /// The physical windows the engine owns.
    pub fn windows(&self) -> Vec<(u64, u64)> {
        vec![
            (self.base, self.base + WINDOW),
            (self.memory_types, self.memory_types + 8),
        ]
    }
    fn check(&self, state: &State) -> Result<(), String> {
        if state
            .words
            .iter()
            .any(|(o, v)| *v == 0 || mask(*o).is_none_or(|m| v & !m != 0))
            || state.due.is_some() && state.finished
        {
            return Err("Invalid XP4 filter engine checkpoint".into());
        }
        Ok(())
    }
}

impl Device for FilterEngine {
    fn name(&self) -> &'static str {
        "xp4-filter-engine"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.advance(cx);
        let offset = address.wrapping_sub(self.base);
        if size != 4 || offset & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        if address == self.memory_types || address == self.memory_types + 4 {
            return 0;
        }
        let idle = self.state.due.is_none();
        u64::from(match offset {
            CONTROL => u32::from(idle),
            START => 0,
            IDLE => {
                if idle {
                    IDLE_BITS
                } else {
                    0
                }
            }
            FINISHED => u32::from(self.state.finished),
            o if mask(o).is_some() => self.state.words.get(&o).copied().unwrap_or(0),
            _ => {
                cx.unmodeled();
                return 0;
            }
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.advance(cx);
        let Ok(value) = u32::try_from(value) else {
            return cx.unmodeled();
        };
        if size != 4 {
            return cx.unmodeled();
        }
        if address == self.memory_types || address == self.memory_types + 4 {
            if value != 0 {
                cx.fail(format!("XP4 filter engine memory type {value} is not modeled"));
            }
            return;
        }
        let offset = address.wrapping_sub(self.base);
        match offset {
            CONTROL if value == 0 => self.state.due = None,
            START if value <= 1 => {
                if value == 1 {
                    if self.state.due.is_some() || self.state.finished {
                        return cx.fail("XP4 filter engine started before its last run was cleared");
                    }
                    self.state.due = cx.now().checked_add(self.ticks);
                    if self.state.due.is_none() {
                        return cx.fail("XP4 filter engine deadline overflow");
                    }
                }
            }
            FINISHED if value <= 1 => {
                if value == 1 && self.state.finished {
                    self.state.finished = false;
                    cx.set_irq(self.irq, false);
                }
            }
            o if mask(o).is_some_and(|m| value & !m == 0) => {
                if value == 0 {
                    self.state.words.remove(&o);
                } else {
                    self.state.words.insert(o, value);
                }
            }
            _ => cx.unmodeled(),
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if !self.state.due.is_some_and(|at| at <= cx.now()) {
            return;
        }
        self.state.due = None;
        self.state.runs += 1;
        self.state.finished = true;
        cx.set_irq(self.irq, true);
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

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};

    const BASE: u64 = 0xfffb9000;
    const TYPES: u64 = 0xfff4a710;

    fn refused(d: &mut FilterEngine, h: &mut TestHost, address: u64, value: u32) -> bool {
        let mut cx = h.cx();
        d.write(&mut cx, address, 4, u64::from(value));
        cx.refusal().is_some()
    }

    #[test]
    fn the_driver_sequence_runs_and_finishes() {
        let mut d = FilterEngine::new(BASE, TYPES, 127, 10).unwrap();
        let mut h = TestHost::new();
        // Open: stop handshake and finished-status clear.
        let control = read(&mut d, &mut h, BASE + CONTROL) as u32;
        write(&mut d, &mut h, BASE + CONTROL, control & !1);
        assert_eq!(read(&mut d, &mut h, BASE + CONTROL) & 1, 1);
        assert_eq!(read(&mut d, &mut h, BASE + IDLE) as u32, IDLE_BITS);
        write(&mut d, &mut h, BASE + FINISHED, 1);
        // Program and start.
        write(&mut d, &mut h, BASE + 0x10, 0x4000_0000);
        write(&mut d, &mut h, BASE + 0x18, 480 << 16 | 640);
        assert_eq!(read(&mut d, &mut h, BASE + 0x18), 480 << 16 | 640);
        write(&mut d, &mut h, TYPES, 0);
        write(&mut d, &mut h, TYPES + 4, 0);
        write(&mut d, &mut h, BASE + 0x3c, 0x8000_0000 | 0x1000_1000);
        write(&mut d, &mut h, BASE + START, 1);
        assert_eq!(read(&mut d, &mut h, BASE + IDLE), 0);
        advance(&mut d, &mut h, 10);
        assert_eq!(read(&mut d, &mut h, BASE + FINISHED), 1);
        assert_eq!(read(&mut d, &mut h, BASE + IDLE) as u32, IDLE_BITS);
        let saved = d.save();
        write(&mut d, &mut h, BASE + FINISHED, 1);
        assert_eq!(read(&mut d, &mut h, BASE + FINISHED), 0);
        let mut other = FilterEngine::new(BASE, TYPES, 127, 10).unwrap();
        other.restore(&saved).unwrap();
        assert_eq!(read(&mut other, &mut h, BASE + FINISHED), 1);
    }

    #[test]
    fn undeclared_bits_and_memory_types_are_refused() {
        let mut d = FilterEngine::new(BASE, TYPES, 127, 10).unwrap();
        let mut h = TestHost::new();
        assert!(refused(&mut d, &mut h, BASE + 0x10, 4));
        assert!(refused(&mut d, &mut h, BASE + 0x0c, 1));
        assert!(refused(&mut d, &mut h, BASE + CONTROL, 1));
        assert!(refused(&mut d, &mut h, TYPES, 1));
        assert!(d.restore(&serde_json::json!({"words": {"12": 1}, "finished": false, "runs": 0})).is_err());
    }
}
