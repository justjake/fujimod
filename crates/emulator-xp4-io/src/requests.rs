//! Posted control request words: 108d518 writes requests 2/3/5/6 and waits for
//! low three busy bits to clear. No target reset/clock routing is inferred.
use fujimod_emulator_devices::{Cx, Device, Record, Value as Field};
use serde_json::{Value, json};
use std::collections::BTreeMap;

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
    generation: u64,
}
impl Requests {
    pub fn new(inputs: &[(u64, u64)]) -> Result<Self, String> {
        let mut words = BTreeMap::new();
        for &(address, ticks) in inputs {
            if address & 3 != 0
                || address > 0xffff_fffc
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
        }
        Ok(Self {
            words,
            generation: 0,
        })
    }
}
impl Device for Requests {
    fn name(&self) -> &'static str {
        "xp4-control-requests"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if size == 4
            && let Some(word) = self.words.get(&address)
        {
            return u64::from(word.value);
        }
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(word) = self.words.get_mut(&address).filter(|_| size == 4) else {
            cx.unmodeled();
            return;
        };
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
