//! X-Processor Pro imaging frame sync at 0xfff38000 and the frame interrupt
//! words of the imaging control page (+0x140 enable, +0x144 status).
//!
//! X-T20 2.11 section 5 programs the timing words +0x04..+0x58
//! (0xc3fd10..0xc401b4) and +0x00: 0xc3fc98 writes bit 1, 0xc3fcb8 writes
//! bits 0, 1 and 4, 0xc3fce0 clears bit 1. Seg6 0x16b91bc clears bit 0 and
//! waits until it reads set, while bit 1 may be clear. Bit 0 is the frame
//! flag: a frame starts every declared period counted from tick zero and sets
//! it; the other bits read back their last write, and whether any of them
//! gates the frames is not known. Each frame also latches the status
//! bits of the declared frame sources in +0x144. A source's line is high
//! while its status and enable bits are both set; writing 1 to a status bit
//! clears it (0xc359b4, called by the handlers 0xd09958 and 0xd099ec). All
//! sources start on the same frame tick; the phase between them is not known.
//! What the timing words select and the frame rate they give are not modeled.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Register window size in bytes.
pub const WINDOW: u64 = 0x100;
/// Frame interrupt words inside the imaging control page.
pub const INTERRUPT_WINDOW: u64 = 8;
const LAST_WORD: u64 = 0x58;
const FRAME: u32 = 1;
const ENABLE: u64 = 0;
const STATUS: u64 = 4;
/// Status and enable bit of each source.
const SOURCES: [u32; 3] = [1, 1 << 8, 1 << 16];

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    enable: u32,
    status: u32,
    next: u64,
    lines: [bool; 3],
}
pub struct Generator {
    base: u64,
    interrupts: u64,
    /// Line of each source that frames latch.
    frame_lines: Vec<(usize, u32)>,
    period: u64,
    state: State,
}
impl Generator {
    pub fn new(
        base: u64,
        interrupts: u64,
        frame_lines: Vec<(usize, u32)>,
        period: u64,
    ) -> Result<Self, String> {
        let mut sources: Vec<_> = frame_lines.iter().map(|(s, _)| *s).collect();
        sources.sort_unstable();
        sources.dedup();
        if base & 0xfff != 0
            || interrupts & 7 != 0
            || period == 0
            || sources.len() != frame_lines.len()
            || frame_lines
                .iter()
                .any(|(s, irq)| *s >= SOURCES.len() || !(32..512).contains(irq))
        {
            return Err("Invalid X-Processor Pro frame sync".into());
        }
        Ok(Self {
            base,
            interrupts,
            frame_lines,
            period,
            state: State {
                next: period,
                ..State::default()
            },
        })
    }
    fn update_lines(&mut self, cx: &mut Cx<'_>) {
        for &(source, irq) in &self.frame_lines {
            let level = self.state.status & self.state.enable & SOURCES[source] != 0;
            if level != self.state.lines[source] {
                self.state.lines[source] = level;
                cx.set_irq(irq, level);
            }
        }
    }
    /// Offset in the sync window, or `None` for the interrupt words.
    fn offset(&self, address: u64, size: u8) -> Option<Result<u64, u64>> {
        if size != 4 || address & 3 != 0 {
            return None;
        }
        if let Some(o) = address.checked_sub(self.base).filter(|o| *o <= LAST_WORD) {
            return Some(Ok(o));
        }
        address
            .checked_sub(self.interrupts)
            .filter(|o| *o < INTERRUPT_WINDOW)
            .map(Err)
    }
}
impl Device for Generator {
    fn name(&self) -> &'static str {
        "xpp-frame-sync"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        u64::from(match self.offset(address, size) {
            Some(Ok(o)) => self.state.words.get(&o).copied().unwrap_or(0),
            Some(Err(ENABLE)) => self.state.enable,
            Some(Err(STATUS)) => self.state.status,
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
            Some(Ok(o)) => {
                self.state.words.insert(o, value);
            }
            Some(Err(ENABLE)) => self.state.enable = value,
            Some(Err(STATUS)) => self.state.status &= !value,
            _ => return cx.unmodeled(),
        }
        self.update_lines(cx);
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let next = self.state.next;
        if next > cx.now() {
            return;
        }
        *self.state.words.entry(0).or_default() |= FRAME;
        for &(source, _) in &self.frame_lines {
            self.state.status |= SOURCES[source];
        }
        let frames = (cx.now() - next) / self.period + 1;
        match frames
            .checked_mul(self.period)
            .and_then(|step| next.checked_add(step))
        {
            Some(at) => self.state.next = at,
            None => cx.fail("Frame sync deadline overflow"),
        }
        self.update_lines(cx);
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        Some(self.state.next)
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state.next == 0
            || state.next % self.period != 0
            || state.words.keys().any(|o| o & 3 != 0 || *o > LAST_WORD)
        {
            return Err("Invalid frame sync checkpoint".into());
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};

    const BASE: u64 = 0xfff38000;
    const INTERRUPTS: u64 = 0xfff39140;

    #[test]
    fn frames_set_the_flag_and_latch_enabled_sources_until_acknowledged() {
        let mut sync = Generator::new(BASE, INTERRUPTS, vec![(0, 102), (1, 100)], 1000).unwrap();
        let mut h = TestHost::new();
        write(&mut sync, &mut h, BASE + 0x08, 0x0010_0020);
        assert_eq!(sync.next_deadline(0), Some(1000));
        // 0x16b91bc: clear bit 0, then wait for the next frame.
        write(&mut sync, &mut h, BASE, 0x10);
        advance(&mut sync, &mut h, 999);
        assert_eq!(read(&mut sync, &mut h, BASE), 0x10);
        write(&mut sync, &mut h, INTERRUPTS, 0x101);
        advance(&mut sync, &mut h, 1000);
        assert_eq!(read(&mut sync, &mut h, BASE), 0x11);
        assert_eq!(read(&mut sync, &mut h, INTERRUPTS + 4), 0x101);
        assert_eq!(sync.state.lines, [true, true, false]);
        write(&mut sync, &mut h, INTERRUPTS + 4, 1);
        assert_eq!(sync.state.lines, [false, true, false]);
        let saved = sync.save();
        advance(&mut sync, &mut h, 3100);
        assert_eq!(sync.next_deadline(0), Some(4000));
        sync.restore(&saved).unwrap();
        assert_eq!(sync.next_deadline(0), Some(2000));
        assert_eq!(read(&mut sync, &mut h, BASE + 0x08), 0x0010_0020);
        let mut cx = h.cx();
        sync.read(&mut cx, BASE + 0x5c, 4);
        assert!(cx.is_unmodeled());
    }
}
