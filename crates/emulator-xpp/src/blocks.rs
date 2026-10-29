//! X-Processor Pro register blocks with no work in flight.
//!
//! Several imaging blocks of X-T20 2.11 are configured from section 5 with
//! plain word writes, often through the table copier 0xc41d30, and read back
//! with read-modify-write updates. Some have handshakes that wait for a
//! status bit after a stop or start: 0xc3c0c0 writes 0 to 0xfff3b000 and
//! waits until bit 0 reads set; 0xc3a954 does the same for the units at
//! 0xfff55000 + 0x1000*n, and their stop 0xc3be40 waits for +0xd8 bit 8.
//! Each block is a register file: words read back their last write and start
//! at 0, and the declared idle bits always read set, as no work is ever in
//! flight. What the blocks compute and any data they move are not modeled.
//!
//! A block may declare [`Frames`]: while its start bit is set it finishes a
//! frame at every multiple of the frame period and raises one interrupt edge;
//! its stopped bit reads set only while the start bit is clear. The live-view
//! output units at 0xfff55000 + 0x1000*n start with +0xc8 bit 28 (0xc3a98c),
//! their stop clears it and waits for +0xd8 bit 8, and their edge handlers
//! (GIC 107 + n) report the frame to the imaging dispatcher.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Frame law of a block: (offset, mask) of the start and stopped bits, the
/// interrupt line and the frame period in ticks.
#[derive(Clone, Copy, Debug)]
pub struct Frames {
    pub start: (u64, u32),
    pub stopped: (u64, u32),
    pub irq: u32,
    pub period: u64,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    due: Option<u64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    frames: u64,
}
fn is_zero(n: &u64) -> bool {
    *n == 0
}
pub struct Block {
    base: u64,
    size: u64,
    /// (offset, mask) of status bits that always read set.
    idle: Vec<(u64, u32)>,
    frames: Option<Frames>,
    state: State,
}
impl Block {
    pub fn new(
        base: u64,
        size: u64,
        idle: Vec<(u64, u32)>,
        frames: Option<Frames>,
    ) -> Result<Self, String> {
        let bit = |(o, mask): (u64, u32)| o & 3 != 0 || o >= size || mask == 0;
        if base & 0xfff != 0
            || size == 0
            || size > 0x1000
            || size & 3 != 0
            || idle.iter().any(|&b| bit(b))
            || frames.is_some_and(|f| {
                bit(f.start)
                    || bit(f.stopped)
                    || !(32..512).contains(&f.irq)
                    || f.period == 0
                    || idle.contains(&f.stopped)
            })
        {
            return Err("Invalid X-Processor Pro register block".into());
        }
        Ok(Self {
            base,
            size,
            idle,
            frames,
            state: State::default(),
        })
    }
    fn started(&self, frames: &Frames) -> bool {
        let (offset, mask) = frames.start;
        self.state.words.get(&offset).is_some_and(|w| w & mask != 0)
    }
    /// Register window size in bytes.
    pub fn window(&self) -> u64 {
        self.size
    }
    /// The stopped bit reads the run state, not the last write.
    fn stopped_mask(&self, offset: u64) -> u32 {
        self.frames
            .filter(|f| f.stopped.0 == offset)
            .map_or(0, |f| f.stopped.1)
    }
    fn offset(&self, address: u64, size: u8) -> Option<u64> {
        let offset = address.checked_sub(self.base)?;
        (size == 4 && offset & 3 == 0 && offset < self.size).then_some(offset)
    }
}
impl Device for Block {
    fn name(&self) -> &'static str {
        "xpp-register-block"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        let mut idle = self
            .idle
            .iter()
            .filter(|(o, _)| *o == offset)
            .fold(0, |bits, (_, mask)| bits | mask);
        if let Some(frames) = self.frames.filter(|f| f.stopped.0 == offset) {
            idle = idle & !frames.stopped.1
                | if self.started(&frames) {
                    0
                } else {
                    frames.stopped.1
                };
        }
        u64::from(
            self.state.words.get(&offset).copied().unwrap_or(0) & !self.stopped_mask(offset) | idle,
        )
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        match (self.offset(address, size), u32::try_from(value)) {
            (Some(offset), Ok(0)) => {
                self.state.words.remove(&offset);
            }
            (Some(offset), Ok(value)) => {
                self.state.words.insert(offset, value);
            }
            _ => return cx.unmodeled(),
        }
        if let Some(frames) = self.frames {
            self.state.due = match (self.started(&frames), self.state.due) {
                (false, _) => None,
                (true, Some(at)) => Some(at),
                (true, None) => (cx.now() / frames.period)
                    .checked_add(1)
                    .and_then(|n| n.checked_mul(frames.period)),
            };
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let (Some(frames), Some(due)) = (self.frames, self.state.due) else {
            return;
        };
        if cx.now() < due {
            return;
        }
        cx.set_irq(frames.irq, true);
        cx.set_irq(frames.irq, false);
        self.state.frames += 1;
        let step = ((cx.now() - due) / frames.period + 1).checked_mul(frames.period);
        self.state.due = step.and_then(|step| due.checked_add(step));
        if self.state.due.is_none() {
            cx.fail("Register block frame deadline overflow");
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
        let started = self.frames.is_some_and(|f| {
            let (offset, mask) = f.start;
            state.words.get(&offset).is_some_and(|w| w & mask != 0)
        });
        if state
            .words
            .iter()
            .any(|(o, v)| o & 3 != 0 || *o >= self.size || *v == 0)
            || started != state.due.is_some()
            || self
                .frames
                .zip(state.due)
                .is_some_and(|(f, at)| at % f.period != 0)
        {
            return Err("Invalid register block checkpoint".into());
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, read, write};

    const BASE: u64 = 0xfff55000;

    #[test]
    fn words_read_back_and_idle_bits_read_set() {
        let mut unit = Block::new(BASE, 0x200, vec![(0, 1), (0xd8, 0x100)], None).unwrap();
        let mut h = TestHost::new();
        // 0xc3a954: stop, then wait for bit 0.
        write(&mut unit, &mut h, BASE, 0);
        assert_eq!(read(&mut unit, &mut h, BASE), 1);
        // 0xc3ab78 sets +0xd8 bit 0; 0xc3be40 waits for bit 8.
        write(&mut unit, &mut h, BASE + 0xd8, 1);
        assert_eq!(read(&mut unit, &mut h, BASE + 0xd8), 0x101);
        write(&mut unit, &mut h, BASE + 0xc0, 0xa7d_6000);
        let saved = unit.save();
        write(&mut unit, &mut h, BASE + 0xc0, 0);
        unit.restore(&saved).unwrap();
        assert_eq!(read(&mut unit, &mut h, BASE + 0xc0), 0xa7d_6000);
        let mut cx = h.cx();
        unit.read(&mut cx, BASE + 0x200, 4);
        assert!(cx.is_unmodeled());
        assert!(Block::new(BASE, 0x200, vec![(0x200, 1)], None).is_err());
    }

    #[test]
    fn started_blocks_finish_a_frame_each_period() {
        use fujimod_emulator_devices::test_host::advance;
        let frames = Frames {
            start: (0xc8, 1 << 28),
            stopped: (0xd8, 0x100),
            irq: 107,
            period: 10,
        };
        assert!(Block::new(BASE, 0x200, vec![(0xd8, 0x100)], Some(frames)).is_err());
        let mut unit = Block::new(BASE, 0x200, vec![(0, 1)], Some(frames)).unwrap();
        let mut h = TestHost::new();
        assert_eq!(read(&mut unit, &mut h, BASE + 0xd8), 0x100);
        assert_eq!(unit.next_deadline(0), None);
        h.now = 3;
        write(&mut unit, &mut h, BASE + 0xc8, 1 << 28 | 1 << 24);
        assert_eq!(read(&mut unit, &mut h, BASE + 0xd8), 0);
        assert_eq!(unit.next_deadline(3), Some(10));
        // 0xc3abe8 sets bit 0 for each frame; bit 8 keeps reading the run state.
        write(&mut unit, &mut h, BASE + 0xd8, 0x101);
        assert_eq!(read(&mut unit, &mut h, BASE + 0xd8), 1);
        advance(&mut unit, &mut h, 25);
        assert_eq!(unit.state.frames, 1);
        assert_eq!(unit.next_deadline(25), Some(30));
        let saved = unit.save();
        write(&mut unit, &mut h, BASE + 0xc8, 1 << 24);
        assert_eq!(read(&mut unit, &mut h, BASE + 0xd8), 0x101);
        assert_eq!(unit.next_deadline(25), None);
        unit.restore(&saved).unwrap();
        assert_eq!(unit.next_deadline(25), Some(30));
        assert!(
            unit.restore(&serde_json::json!({ "words": [] , "due": 30 }))
                .is_err()
        );
    }
}
