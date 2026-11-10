//! Posted control request words: 108d518 writes requests 2/3/5/6 and waits for
//! low three busy bits to clear. Each request word is the `+0x80` word of a
//! block; the block also holds command words at `+0x00`/`+0x08`, a gate
//! register file at `+0x40..+0x4c` (108d8e4 masks 0x1ff), and a completion
//! word at `+0xf0`. The whole block is owned here: command and gate words read
//! back their last write, the request word keeps its posted-request law.
//! No target reset/clock routing is inferred.
use fujimod_emulator_devices::{Cx, Device, Record, Value as Field};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const REQUEST_OFFSET: u64 = 0x80;
const BLOCK_BYTES: u64 = 0x100;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Word {
    ticks: u64,
    value: u32,
    #[serde(rename = "next")]
    due: Option<(u64, u64)>,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requests {
    words: BTreeMap<u64, Word>,
    /// Command, gate and completion words of each block, by address.
    registers: BTreeMap<u64, u32>,
    /// Each block's `[base, base + 0x100)` window.
    blocks: Vec<(u64, u64)>,
    generation: u64,
}
impl Requests {
    pub fn new(inputs: &[(u64, u64)]) -> Result<Self, String> {
        let mut words = BTreeMap::new();
        let mut blocks = Vec::new();
        for &(address, ticks) in inputs {
            if address & 3 != 0
                || !(REQUEST_OFFSET..=0xffff_fffc).contains(&address)
                || !(1..=1_000_000).contains(&ticks)
                || words
                    .insert(
                        address,
                        Word {
                            ticks,
                            value: 0,
                            due: None,
                        },
                    )
                    .is_some()
            {
                return Err("Invalid or duplicate XP4 control request input".into());
            }
            let base = address - REQUEST_OFFSET;
            blocks.push((base, base + BLOCK_BYTES));
        }
        blocks.sort_unstable();
        if blocks.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err("Overlapping XP4 control request blocks".into());
        }
        Ok(Self {
            words,
            registers: BTreeMap::new(),
            blocks,
            generation: 0,
        })
    }
    fn in_block(&self, address: u64) -> bool {
        self.blocks
            .iter()
            .any(|&(lo, hi)| lo <= address && address < hi)
    }
}
impl Device for Requests {
    fn name(&self) -> &'static str {
        "xp4-control-requests"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if size == 4 {
            if let Some(word) = self.words.get(&address) {
                return u64::from(word.value);
            }
            if self.in_block(address) {
                return u64::from(self.registers.get(&address).copied().unwrap_or(0));
            }
        }
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if size != 4 {
            cx.unmodeled();
            return;
        }
        if let Some(word) = self.words.get_mut(&address) {
            if !matches!(value, 0 | 2 | 3 | 5 | 6) {
                cx.fail("Unsupported XP4 control request bits");
                return;
            }
            if value == 0 {
                word.value = 0;
                word.due = None;
                return;
            }
            if word.due.is_some() {
                if word.value != value as u32 {
                    cx.fail("Different XP4 control request while busy");
                }
                return; // An identical peer write cannot postpone an in-flight request.
            }
            self.generation += 1;
            word.value = value as u32;
            let at = cx.now() + word.ticks;
            word.due = Some((at, self.generation));
            cx.schedule(at, self.generation);
            return;
        }
        if self.in_block(address) {
            self.registers.insert(address, value as u32);
            return;
        }
        cx.unmodeled();
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.words.values().filter_map(|w| w.due.map(|d| d.0)).min()
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        for (&address, word) in &mut self.words {
            if let Some((at, generation)) = word.due
                && generation == token
                && at <= cx.now()
            {
                let request = word.value;
                word.value = 0;
                word.due = None;
                cx.record(Record::new(
                    "XP4 control request acknowledged",
                    [
                        ("address", Field::int(address)),
                        ("request", Field::int(request)),
                        ("tick", Field::int(at)),
                    ],
                ));
            }
        }
    }
    fn save(&self) -> Value {
        json!(self)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: Self = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if !state.words.keys().eq(self.words.keys())
            || state.blocks != self.blocks
            || state
                .registers
                .keys()
                .any(|&address| !state.in_block(address))
            || state.words.iter().any(|(a, w)| {
                w.ticks != self.words[a].ticks
                    || !matches!(w.value, 0 | 2 | 3 | 5 | 6)
                    || w.due.is_some() != (w.value != 0)
                    || w.due
                        .is_some_and(|(_, token)| token == 0 || token > state.generation)
            })
        {
            return Err("Invalid XP4 request checkpoint or clock input".into());
        }
        *self = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::Host;
    #[derive(Default)]
    struct Clock {
        now: u64,
        records: Vec<Record>,
        events: Vec<(u64, u64)>,
    }
    impl Host for Clock {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, _: usize, at: u64, token: u64) {
            self.events.push((at, token));
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            unreachable!()
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            unreachable!()
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            unreachable!()
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            unreachable!()
        }
        fn record(&mut self, record: Record) {
            self.records.push(record);
        }
    }
    #[test]
    fn restore_keeps_request_tokens_across_cancellation_and_resubmission() {
        let mut source = Requests::new(&[(0x1000, 8)]).unwrap();
        let mut clock = Clock::default();
        source.write(&mut Cx::new(&mut clock, 0, 0, 0), 0x1000, 4, 2);
        let mut resumed = Requests::new(&[(0x1000, 8)]).unwrap();
        resumed.restore(&source.save()).unwrap();
        for model in [&mut source, &mut resumed] {
            model.write(&mut Cx::new(&mut clock, 0, 0, 0), 0x1000, 4, 0);
            model.write(&mut Cx::new(&mut clock, 0, 0, 0), 0x1000, 4, 3);
        }
        clock.now = 8;
        for token in [1, 2] {
            source.event(&mut Cx::new(&mut clock, 0, 0x1000, 0), token);
            resumed.event(&mut Cx::new(&mut clock, 3, 0xdead0000, 0), token);
            assert_eq!(source.save(), resumed.save());
            assert_eq!(source.words[&0x1000].value, if token == 1 { 3 } else { 0 });
        }
    }
    #[test]
    fn extended_requests_finish_on_their_exact_clock_without_program_inputs() {
        for request in [5, 6] {
            let mut outcomes = vec![];
            for (pc, core) in [(0x1000, 0), (0xdeadbeef, 3)] {
                let mut m = Requests::new(&[(0x2000, 8)]).unwrap();
                let mut h = Clock {
                    now: 100,
                    ..Default::default()
                };
                m.write(&mut Cx::new(&mut h, core, pc, 0), 0x2000, 4, request);
                assert_eq!(m.next_deadline(100), Some(108));
                let before = m.save();
                h.now = 107;
                m.event(&mut Cx::new(&mut h, core, pc, 0), 1);
                assert_eq!(m.save(), before);
                h.now = 108;
                m.event(&mut Cx::new(&mut h, core, pc, 0), 1);
                assert_eq!(m.read(&mut Cx::new(&mut h, core, pc, 0), 0x2000, 4), 0);
                assert_eq!(m.next_deadline(108), None);
                assert_eq!(h.events, [(108, 1)]);
                assert_eq!(h.records.len(), 1);
                outcomes.push((m.save(), h.records));
            }
            assert_eq!(outcomes[0], outcomes[1]);
        }
    }
    #[test]
    fn block_command_and_gate_words_read_back_their_last_write() {
        let mut m = Requests::new(&[(0x1080, 8)]).unwrap();
        let mut h = Clock::default();
        // Command word +0x00 and gate word +0x40 hold their last write; unset
        // words in the block read zero.
        m.write(&mut Cx::new(&mut h, 0, 0, 0), 0x1040, 4, 0x1ff);
        m.write(&mut Cx::new(&mut h, 0, 0, 0), 0x1000, 4, 0x2);
        assert_eq!(m.read(&mut Cx::new(&mut h, 0, 0, 0), 0x1040, 4), 0x1ff);
        assert_eq!(m.read(&mut Cx::new(&mut h, 0, 0, 0), 0x1000, 4), 0x2);
        assert_eq!(m.read(&mut Cx::new(&mut h, 0, 0, 0), 0x10f0, 4), 0);
        // The request word keeps posted-request law, not last-write storage.
        m.write(&mut Cx::new(&mut h, 0, 0, 0), 0x1080, 4, 3);
        assert_eq!(m.read(&mut Cx::new(&mut h, 0, 0, 0), 0x1080, 4), 3);
        h.now = 8;
        m.event(&mut Cx::new(&mut h, 0, 0, 0), 1);
        assert_eq!(m.read(&mut Cx::new(&mut h, 0, 0, 0), 0x1080, 4), 0);
    }

    #[test]
    fn overlapping_blocks_and_out_of_range_request_words_are_rejected() {
        assert!(Requests::new(&[(0x40, 8)]).is_err());
        assert!(Requests::new(&[(0x1080, 8), (0x1100, 8)]).is_err());
        assert!(Requests::new(&[(0x1080, 8), (0x1200, 8)]).is_ok());
    }

    fn transfer(pc: u64, core: usize) -> (Value, Vec<Record>) {
        let mut m = Requests::new(&[(0x1000, 8), (0x2000, 16)]).unwrap();
        let mut h = Clock {
            now: 100,
            ..Default::default()
        };
        m.write(&mut Cx::new(&mut h, core, pc, 0), 0x1000, 4, 2);
        m.write(&mut Cx::new(&mut h, core, pc, 0), 0x2000, 4, 3);
        let before = m.save();
        h.now = 104;
        m.write(&mut Cx::new(&mut h, 3 - core, pc + 4, 0), 0x1000, 4, 2);
        assert_eq!(m.save(), before);
        assert_eq!(h.events, [(108, 1), (116, 2)]);
        h.now = 107;
        m.event(&mut Cx::new(&mut h, core, pc, 0), 1);
        assert_eq!(m.save(), before);
        assert_eq!(m.save(), m.save());
        h.now = 108;
        m.event(&mut Cx::new(&mut h, core, pc, 0), 1);
        assert_eq!(m.read(&mut Cx::new(&mut h, core, pc, 0), 0x1000, 4), 0);
        assert_eq!(m.next_deadline(108), Some(116));
        m.write(&mut Cx::new(&mut h, core, pc, 0), 0x2000, 4, 0);
        h.now = 116;
        m.event(&mut Cx::new(&mut h, core, pc, 0), 2);
        assert_eq!(h.records.len(), 1);
        assert_eq!(m.next_deadline(116), None);
        (m.save(), h.records)
    }
    #[test]
    fn peer_writes_do_not_delay_completion_and_observers_do_not_acknowledge() {
        assert_eq!(transfer(0xabc, 0), transfer(0xdef, 3));
    }
}
