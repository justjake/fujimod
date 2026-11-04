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
//! +0x00 bit 0 reads set. Other words read back their last write.
//!
//! The twelve output buffers are the words +0x1b8..+0x1e4, programmed for
//! the first of two halves; +0x10 is the distance to the second (0x3690).
//! The engine alternates halves frame by frame, beginning with the first,
//! and keeps alternating when the firmware restarts it between frames (the
//! camera fills both halves while +0x1cc keeps the first half's address).
//! Section 6 0x1698dd0 takes each completion as the half it was writing and
//! switches its own pointer (0x1bc6310) to the other, without reprogramming
//! the engine. With a declared scene, each frame writes the zone sums buffer
//! (+0x1cc) of the half it completes: three planes of 16x16 u32 zone sums
//! (red, green, blue; green is about 2.6 times red, probably both green
//! sites) and a fourth plane of zeros, as in the camera's captures. The
//! scene is uniform, so every zone of a plane holds the declared value.
//! Declared buffers name one of the twelve output words and the u32 words
//! the engine writes there each frame, in the half it completes (measured
//! buffers of the same scene). The other buffers are not written.
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
const HALF: u64 = 0x10;
const ZONE_SUMS: u64 = 0x1cc;
const ZONES: usize = 256;
const OUTPUTS: std::ops::RangeInclusive<u64> = 0x1b8..=0x1e4;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    status: u32,
    due: Option<u64>,
    line: bool,
    frames: u64,
    /// The engine writes the second half next.
    #[serde(default)]
    second: bool,
}
pub struct Engine {
    base: u64,
    irq: u32,
    period: u64,
    scene: Option<[u32; 3]>,
    buffers: Vec<(u64, Vec<u8>)>,
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
            scene: None,
            buffers: Vec::new(),
            state: State::default(),
        })
    }
    /// A uniform scene: the red, green and blue sum of every zone.
    pub fn with_scene(mut self, zone_sums: [u32; 3]) -> Self {
        self.scene = Some(zone_sums);
        self
    }
    /// A declared buffer: the output word (+0x1b8..+0x1e4) and its contents.
    pub fn with_buffer(mut self, output: u64, words: &[u32]) -> Result<Self, String> {
        if !OUTPUTS.contains(&output)
            || output & 3 != 0
            || words.is_empty()
            || (output == ZONE_SUMS && self.scene.is_some())
            || self.buffers.iter().any(|(o, _)| *o == output)
        {
            return Err("Invalid X-Processor Pro statistics buffer".into());
        }
        let bytes = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.buffers.push((output, bytes));
        Ok(self)
    }
    fn write_output(&self, cx: &mut Cx<'_>, output: u64, data: &[u8]) {
        let half = if self.state.second {
            self.word(HALF)
        } else {
            0
        };
        let address = u64::from(self.word(output)) + u64::from(half);
        if cx.ram_write(address, data).is_err() {
            cx.fail("Statistics buffer outside RAM");
        }
    }
    fn write_buffers(&self, cx: &mut Cx<'_>) {
        if let Some(scene) = self.scene {
            let mut planes = Vec::with_capacity(4 * ZONES * 4);
            for value in scene.into_iter().chain([0]) {
                for _ in 0..ZONES {
                    planes.extend_from_slice(&value.to_le_bytes());
                }
            }
            self.write_output(cx, ZONE_SUMS, &planes);
        }
        for (output, data) in &self.buffers {
            self.write_output(cx, *output, data);
        }
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
        self.write_buffers(cx);
        self.state.second = !self.state.second;
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
        self.check(&state)?;
        self.state = state;
        Ok(())
    }
    fujimod_emulator_devices::checkpoint_state!(state: State, Self::check);
}
impl Engine {
    fn check(&self, state: &State) -> Result<(), String> {
        let started = state.words.get(&CONTROL).is_some_and(|w| w & START != 0);
        if started != state.due.is_some()
            || state.due.is_some_and(|at| at % self.period != 0)
            || state.words.keys().any(|o| o & 3 != 0 || *o >= STATE)
        {
            return Err("Invalid statistics engine checkpoint".into());
        }
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

    #[test]
    fn declared_scene_fills_the_zone_sums_of_alternating_halves() {
        let mut stats = Engine::new(BASE, 106, 1000)
            .unwrap()
            .with_scene([145_000, 383_000, 34_000]);
        let mut h = TestHost::new();
        let word = |h: &TestHost, address: usize| {
            u32::from_le_bytes(h.ram[address..address + 4].try_into().unwrap())
        };
        let plane =
            |h: &TestHost, address: usize| [0, 1, 2, 3].map(|n| word(h, address + 0x400 * n));
        write(&mut stats, &mut h, BASE + HALF, 0x3690);
        write(&mut stats, &mut h, BASE + ZONE_SUMS, 0x1000);
        write(&mut stats, &mut h, BASE + CONTROL, START);
        advance(&mut stats, &mut h, 1000);
        assert_eq!(plane(&h, 0x1000), [145_000, 383_000, 34_000, 0]);
        assert_eq!(plane(&h, 0x13fc), [145_000, 383_000, 34_000, 0]);
        assert_eq!(word(&h, 0x4690), 0);
        advance(&mut stats, &mut h, 2000);
        assert_eq!(plane(&h, 0x4690), [145_000, 383_000, 34_000, 0]);
        // A restart keeps alternating.
        write(&mut stats, &mut h, BASE + CONTROL, 0);
        h.ram[0x1000..0x1004].fill(0);
        write(&mut stats, &mut h, BASE + CONTROL, START);
        advance(&mut stats, &mut h, 3000);
        assert_eq!(word(&h, 0x1000), 145_000);
        h.ram[0x4690..0x4694].fill(0);
        write(&mut stats, &mut h, BASE + CONTROL, 0);
        write(&mut stats, &mut h, BASE + CONTROL, START);
        advance(&mut stats, &mut h, 4000);
        assert_eq!(word(&h, 0x4690), 145_000);
    }

    #[test]
    fn declared_buffers_are_written_to_their_output_each_frame() {
        assert!(
            Engine::new(BASE, 106, 1000)
                .unwrap()
                .with_buffer(0x1b4, &[1])
                .is_err()
        );
        assert!(
            Engine::new(BASE, 106, 1000)
                .unwrap()
                .with_scene([1, 2, 3])
                .with_buffer(ZONE_SUMS, &[1])
                .is_err()
        );
        let mut stats = Engine::new(BASE, 106, 1000)
            .unwrap()
            .with_buffer(0x1d0, &[7, 0, 9])
            .unwrap();
        let mut h = TestHost::new();
        let word = |h: &TestHost, address: usize| {
            u32::from_le_bytes(h.ram[address..address + 4].try_into().unwrap())
        };
        write(&mut stats, &mut h, BASE + HALF, 0x3690);
        write(&mut stats, &mut h, BASE + 0x1d0, 0x2000);
        write(&mut stats, &mut h, BASE + CONTROL, START);
        advance(&mut stats, &mut h, 1000);
        assert_eq!([0, 4, 8].map(|o| word(&h, 0x2000 + o)), [7, 0, 9]);
        advance(&mut stats, &mut h, 2000);
        assert_eq!(word(&h, 0x5690), 7);
    }
}
