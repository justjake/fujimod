//! XP4 face detection engine: the register contract of the X-T3 5.11 driver.
//!
//! The driver has one accessor per word or word group. The engine is opened
//! on the face task's open message and started once per live view frame
//! from the "fe_fdet" step.
//!
//! | Offset | Use |
//! |---|---|
//! | +0x00 | bit 0 written 0 to stop; reads nonzero once idle (the stop polls it) |
//! | +0x04 | bit 0 starts a run |
//! | +0x08 | mode: bits 2..0 (< 5), flags 4, 8, 12, 16 |
//! | +0x0c..+0x5fc | configuration words, read back by the driver |
//! | +0x600..+0x608 | results |
//! | +0x6e0, +0x6e4 | configuration words |
//! | +0x700 | channel flags, bits 0, 4, 8, 16, 20, 24 |
//! | +0x704 | channel fields, read with +0x700 |
//! | +0x708 | writes 1 to clear +0x700 flags |
//! | +0x710..+0x71c | configuration words |
//! | +0x730 | bit 0: run finished (read by the handler) |
//! | +0x734 | bit 0: run finished; writes 1 to clear |
//! | +0x738 | bit 0: arm the channels |
//!
//! The run sequence writes the mode and configuration, sets +0x738
//! bit 0, polls +0x700 until all six channel flags read 1, clears them
//! through +0x708, then sets +0x04 bit 0. The open registers the driver's
//! handler on interrupt line 123. The handler reads +0x730, writes it back
//! to +0x734 and posts the finished run to the face task.
//!
//! Modeled: configuration words read back their last write; arming sets
//! the six channel flags at once; a mode 0 run finishes `run_ticks` after
//! its start and raises the line until +0x734 bit 0 is cleared. The image
//! is not read and no face is detected: the result words read zero. The
//! run time and the arming latency are not measured. Runs in other modes
//! (the driver also starts mode 4 and polls +0x734) are refused.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const WINDOW: u64 = 0x1000;
const CONTROL: u64 = 0x00;
const START: u64 = 0x04;
const MODE: u64 = 0x08;
const CHANNEL_FLAGS: u64 = 0x700;
const CHANNEL_FIELDS: u64 = 0x704;
const CHANNEL_CLEAR: u64 = 0x708;
const FINISHED: u64 = 0x730;
const FINISHED_CLEAR: u64 = 0x734;
const ARM: u64 = 0x738;
/// The six channel flags of +0x700 (and their clear bits in +0x708).
const CHANNELS: u32 = 0x0111_0111;
/// The mode word's fields.
const MODE_BITS: u32 = 0x1_1117;
/// Configuration words that read back their last write.
const CONFIGURATION: &[(u64, u64)] = &[(0x0c, 0x10), (0x14, 0x600), (0x6e0, 0x6e8), (0x710, 0x720)];
/// Result words; no face is detected, so they read zero.
const RESULTS: &[u64] = &[0x600, 0x604, 0x608];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    mode: u32,
    start: bool,
    armed: bool,
    channels: u32,
    finished: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    due: Option<u64>,
    runs: u64,
}

pub struct FaceDetector {
    base: u64,
    irq: u32,
    ticks: u64,
    state: State,
}

fn configuration(offset: u64) -> bool {
    offset & 3 == 0 && CONFIGURATION.iter().any(|&(lo, hi)| (lo..hi).contains(&offset))
}

impl FaceDetector {
    pub fn new(base: u64, irq: u32, ticks: u64) -> Result<Self, String> {
        if base & 0xfff != 0 || base > u64::from(u32::MAX) || !(32..512).contains(&irq) || ticks == 0 {
            return Err("Invalid XP4 face detector".into());
        }
        Ok(Self {
            base,
            irq,
            ticks,
            state: State::default(),
        })
    }
    /// The physical window the engine owns.
    pub fn windows(&self) -> Vec<(u64, u64)> {
        vec![(self.base, self.base + WINDOW)]
    }
    fn check(&self, state: &State) -> Result<(), String> {
        if state.words.iter().any(|(o, v)| !configuration(*o) || *v == 0)
            || state.mode & !MODE_BITS != 0
            || state.channels & !CHANNELS != 0
            || state.due.is_some() && (state.finished || state.mode & 7 != 0)
        {
            return Err("Invalid XP4 face detector checkpoint".into());
        }
        Ok(())
    }
}

impl Device for FaceDetector {
    fn name(&self) -> &'static str {
        "xp4-face-detector"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.advance(cx);
        let offset = address.wrapping_sub(self.base);
        if size != 4 || offset & 3 != 0 || offset >= WINDOW {
            cx.unmodeled();
            return 0;
        }
        u64::from(match offset {
            CONTROL => u32::from(self.state.due.is_none()),
            START => u32::from(self.state.start),
            MODE => self.state.mode,
            CHANNEL_FLAGS => self.state.channels,
            CHANNEL_FIELDS => 0,
            FINISHED | FINISHED_CLEAR => u32::from(self.state.finished),
            ARM => u32::from(self.state.armed),
            o if RESULTS.contains(&o) => 0,
            o if configuration(o) => self.state.words.get(&o).copied().unwrap_or(0),
            _ => {
                cx.unmodeled();
                return 0;
            }
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.advance(cx);
        let offset = address.wrapping_sub(self.base);
        let Ok(value) = u32::try_from(value) else {
            return cx.unmodeled();
        };
        if size != 4 || offset & 3 != 0 || offset >= WINDOW {
            return cx.unmodeled();
        }
        match offset {
            CONTROL if value == 0 => {
                // Stop: a run in progress is canceled without finishing.
                self.state.due = None;
            }
            START if value <= 1 => {
                let start = value == 1;
                if start && !self.state.start {
                    if self.state.mode & 7 != 0 {
                        return cx.fail(format!(
                            "XP4 face detector run in mode {} is not modeled",
                            self.state.mode & 7
                        ));
                    }
                    if self.state.due.is_some() || self.state.finished {
                        return cx.fail("XP4 face detector started before its last run was cleared");
                    }
                    self.state.due = cx.now().checked_add(self.ticks);
                    if self.state.due.is_none() {
                        return cx.fail("XP4 face detector deadline overflow");
                    }
                }
                self.state.start = start;
            }
            MODE if value & !MODE_BITS == 0 && value & 7 < 5 => self.state.mode = value,
            CHANNEL_CLEAR if value & !CHANNELS == 0 => self.state.channels &= !value,
            FINISHED_CLEAR if value <= 1 => {
                if value == 1 && self.state.finished {
                    self.state.finished = false;
                    cx.set_irq(self.irq, false);
                }
            }
            ARM if value <= 1 => {
                self.state.armed = value == 1;
                if self.state.armed {
                    self.state.channels = CHANNELS;
                }
            }
            o if configuration(o) => {
                if value == 0 {
                    self.state.words.remove(&o);
                } else {
                    self.state.words.insert(o, value);
                }
            }
            _ => return cx.unmodeled(),
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

    const BASE: u64 = 0xfff7c000;

    fn refused(d: &mut FaceDetector, h: &mut TestHost, offset: u64, value: Option<u32>) -> bool {
        let mut cx = h.cx();
        match value {
            Some(v) => d.write(&mut cx, BASE + offset, 4, u64::from(v)),
            None => {
                d.read(&mut cx, BASE + offset, 4);
            }
        }
        cx.refusal().is_some()
    }

    #[test]
    fn a_run_arms_the_channels_and_finishes_without_faces() {
        let mut d = FaceDetector::new(BASE, 123, 10).unwrap();
        let mut h = TestHost::new();
        write(&mut d, &mut h, BASE + MODE, 0);
        write(&mut d, &mut h, BASE + 0x48, 0x4000_0000);
        write(&mut d, &mut h, BASE + 0x330, 0x1234);
        assert_eq!(read(&mut d, &mut h, BASE + 0x330), 0x1234);
        write(&mut d, &mut h, BASE + ARM, 1);
        assert_eq!(read(&mut d, &mut h, BASE + CHANNEL_FLAGS), u64::from(CHANNELS));
        write(&mut d, &mut h, BASE + CHANNEL_CLEAR, CHANNELS);
        assert_eq!(read(&mut d, &mut h, BASE + CHANNEL_FLAGS), 0);
        write(&mut d, &mut h, BASE + START, 1);
        assert_eq!(read(&mut d, &mut h, BASE + CONTROL), 0);
        assert_eq!(d.next_deadline(0), Some(10));
        advance(&mut d, &mut h, 9);
        assert_eq!(read(&mut d, &mut h, BASE + FINISHED), 0);
        advance(&mut d, &mut h, 10);
        assert_eq!(read(&mut d, &mut h, BASE + FINISHED), 1);
        assert_eq!(read(&mut d, &mut h, BASE + FINISHED_CLEAR), 1);
        assert_eq!(read(&mut d, &mut h, BASE + CONTROL), 1);
        for o in RESULTS {
            assert_eq!(read(&mut d, &mut h, BASE + o), 0);
        }
        let saved = d.save();
        write(&mut d, &mut h, BASE + FINISHED_CLEAR, 1);
        assert_eq!(read(&mut d, &mut h, BASE + FINISHED), 0);
        write(&mut d, &mut h, BASE + START, 0);
        let mut other = FaceDetector::new(BASE, 123, 10).unwrap();
        other.restore(&saved).unwrap();
        assert_eq!(read(&mut other, &mut h, BASE + FINISHED), 1);
        assert_eq!(read(&mut other, &mut h, BASE + 0x330), 0x1234);
    }

    #[test]
    fn a_stop_cancels_the_run_and_reads_idle() {
        let mut d = FaceDetector::new(BASE, 123, 10).unwrap();
        let mut h = TestHost::new();
        write(&mut d, &mut h, BASE + START, 1);
        write(&mut d, &mut h, BASE + CONTROL, 0);
        assert_eq!(read(&mut d, &mut h, BASE + CONTROL), 1);
        assert_eq!(d.next_deadline(0), None);
        advance(&mut d, &mut h, 20);
        assert_eq!(read(&mut d, &mut h, BASE + FINISHED), 0);
    }

    #[test]
    fn undeclared_words_and_other_modes_are_refused() {
        let mut d = FaceDetector::new(BASE, 123, 10).unwrap();
        let mut h = TestHost::new();
        assert!(refused(&mut d, &mut h, 0x10, None));
        assert!(refused(&mut d, &mut h, 0x600, Some(1)));
        assert!(refused(&mut d, &mut h, 0x800, Some(1)));
        assert!(refused(&mut d, &mut h, CONTROL, Some(1)));
        assert!(refused(&mut d, &mut h, MODE, Some(5)));
        write(&mut d, &mut h, BASE + MODE, 4);
        assert!(refused(&mut d, &mut h, START, Some(1)));
        assert!(d.restore(&serde_json::json!({"words": {"2048": 1}, "mode": 0, "start": false,
            "armed": false, "channels": 0, "finished": false, "runs": 0})).is_err());
    }
}
