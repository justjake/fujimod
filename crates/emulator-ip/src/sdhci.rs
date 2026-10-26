//! Reusable SDHCI clock control. Wiring and settling time are physical inputs.
//! Card protocol, host DMA and interrupts are separate mechanisms.
use fujimod_emulator_devices::{Cx, Device, Record, Value as Field};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const ENABLE: u16 = 1;
const STABLE: u16 = 2;
const WRITABLE: u16 = 0xffc5;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Clock {
    control: u16,
    deadline: Option<u64>,
}

/// One or more independently programmed internal/card output clock words.
#[derive(Serialize, Deserialize)]
pub struct Clocks {
    timing: BTreeMap<u64, u64>,
    state: BTreeMap<u64, Clock>,
}
impl Clocks {
    pub fn new(words: &[(u64, u64)]) -> Result<Self, String> {
        let timing: BTreeMap<_, _> = words.iter().copied().collect();
        if words.is_empty()
            || words.len() > 16
            || timing.len() != words.len()
            || words.iter().any(|&(address, ticks)| {
                address & 1 != 0 || address > u32::MAX as u64 - 1 || ticks == 0
            })
        {
            return Err("Invalid SDHCI clock wiring or settling input".into());
        }
        Ok(Self {
            state: timing
                .keys()
                .map(|&address| {
                    (
                        address,
                        Clock {
                            control: 0,
                            deadline: None,
                        },
                    )
                })
                .collect(),
            timing,
        })
    }
}
impl Device for Clocks {
    fujimod_emulator_devices::checkpoint_fields!(state:BTreeMap<u64,Clock>);
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(serde_json::json!({"state":self.state,"timing":self.timing}))
    }
    fn name(&self) -> &'static str {
        "sdhci-clocks"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if size != 2 || !self.state.contains_key(&address) {
            cx.unmodeled();
            return 0;
        }
        self.advance(cx);
        u64::from(self.state[&address].control)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if size != 2
            || value > u16::MAX as u64
            || value & !u64::from(WRITABLE | STABLE) != 0
            || !self.state.contains_key(&address)
        {
            cx.unmodeled();
            return;
        }
        self.advance(cx);
        let clock = self.state.get_mut(&address).unwrap();
        let value = value as u16 & WRITABLE;
        // Divided-clock mode has no PLL: changing its output divider does
        // not restart the internal oscillator (SDHCI 4.20, 2.2.15 and 3.2.3).
        // The output must be stopped before changing its frequency.
        if clock.control & value & 4 != 0 && (clock.control ^ value) & 0xffc0 != 0 {
            cx.unmodeled(); return;
        }
        let relock = (clock.control ^ value) & ENABLE != 0;
        if value & ENABLE == 0 {
            clock.control = value;
            clock.deadline = None;
        } else if relock {
            let Some(at) = cx.now().checked_add(self.timing[&address]) else {
                cx.fail("SDHCI clock settling deadline overflow");
                return;
            };
            clock.control = value;
            clock.deadline = Some(at);
            cx.schedule(at, 0);
        } else {
            clock.control = value | (clock.control & STABLE);
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token == 0 {
            self.advance(cx);
        } else {
            cx.fail("Unknown SDHCI clock event token");
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        for (&address, clock) in &mut self.state {
            if let Some(at) = clock.deadline
                && at <= cx.now()
            {
                clock.control |= STABLE;
                clock.deadline = None;
                cx.record(Record::new(
                    "SDHCI internal clock stable",
                    [
                        ("address", Field::hex(address.into())),
                        ("tick", Field::int(at)),
                    ],
                ));
            }
        }
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.values().filter_map(|clock| clock.deadline).min()
    }
    fn save(&self) -> Value {
        serde_json::json!({"timing": self.timing, "clocks": self.state})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        #[derive(Deserialize, serde::Serialize)]
        #[serde(deny_unknown_fields)]
        struct State {
            timing: BTreeMap<u64, u64>,
            clocks: BTreeMap<u64, Clock>,
        }
        let saved: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if saved.timing != self.timing
            || !saved.clocks.keys().eq(self.state.keys())
            || saved.clocks.values().any(|c| {
                c.control & !(WRITABLE | STABLE) != 0
                    || c.control & ENABLE == 0 && (c.control & STABLE != 0 || c.deadline.is_some())
                    || c.control & ENABLE != 0 && (c.control & STABLE == 0) != c.deadline.is_some()
            })
        {
            return Err("Invalid SDHCI clock checkpoint or changed physical input".into());
        }
        self.state = saved.clocks;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::Host;
    #[derive(Default)]
    struct TestHost {
        now: u64,
        records: Vec<Record>,
        events: Vec<(usize, u64, u64)>,
    }
    impl Host for TestHost {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, device: usize, at: u64, token: u64) {
            self.events.push((device, at, token));
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            panic!("clock settling has no IRQ");
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            panic!("clock settling has no IRQ");
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            panic!("clock cannot inspect RAM");
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            panic!("clock cannot modify RAM");
        }
        fn record(&mut self, r: Record) {
            self.records.push(r);
        }
    }
    fn write(d: &mut Clocks, h: &mut TestHost, address: u64, value: u64, pc: u64) {
        let mut cx = Cx::new(h, 3, pc, 5);
        d.write(&mut cx, address, 2, value);
        assert_eq!(cx.refusal(), None);
    }
    #[test]
    fn divided_clock_keeps_its_oscillator_ready_across_output_frequency_changes() {
        let mut clock=Clocks::new(&[(0xff9b002c,100)]).unwrap();
        let mut host=TestHost::default();
        write(&mut clock,&mut host,0xff9b002c,0x3e01,0);
        host.now=100;
        assert_eq!(clock.read(&mut Cx::new(&mut host,0,0,0),0xff9b002c,2),0x3e03);
        write(&mut clock,&mut host,0xff9b002c,0x3e07,0);
        write(&mut clock,&mut host,0xff9b002c,0x103,0);
        write(&mut clock,&mut host,0xff9b002c,0x107,0);
        assert_eq!(clock.read(&mut Cx::new(&mut host,0,0,0),0xff9b002c,2),0x107);
        assert_eq!(clock.next_deadline(host.now),None);
        let saved=clock.save();
        let mut cx=Cx::new(&mut host,0,0,0);
        clock.write(&mut cx,0xff9b002c,2,0x207);
        assert!(cx.refusal().is_some());
        assert_eq!(clock.save(),saved);
    }
    fn sequence(pc: u64) -> (Value, Vec<Record>) {
        let words = [(0xfffc622c, 8), (0xfffca22c, 20)];
        let mut clocks = Clocks::new(&words).unwrap();
        let mut h = TestHost {
            now: 100,
            ..Default::default()
        };
        write(&mut clocks, &mut h, words[0].0, 0xfa03, pc);
        write(&mut clocks, &mut h, words[1].0, 1, pc);
        assert_eq!(clocks.state[&words[0].0].control, 0xfa01);
        assert_eq!(clocks.next_deadline(h.now), Some(108));
        let pending = clocks.save();
        assert_eq!(clocks.save(), pending);
        let mut restored = Clocks::new(&words).unwrap();
        restored.restore(&pending).unwrap();
        h.now = 107;
        restored.event(&mut Cx::new(&mut h, 0, pc, 5), 0);
        assert_eq!(restored.save(), pending);
        h.now = 111;
        restored.event(&mut Cx::new(&mut h, 0, pc, 5), 0);
        assert_eq!(restored.state[&words[0].0].control, 0xfa03);
        assert_eq!(restored.next_deadline(h.now), Some(120));
        write(&mut restored, &mut h, words[0].0, 0xfa07, pc);
        assert_eq!(restored.state[&words[0].0].control, 0xfa07);
        assert_eq!(restored.next_deadline(h.now), Some(120));
        write(&mut restored, &mut h, words[0].0, 0x401, pc);
        assert_eq!(restored.next_deadline(h.now), Some(120));
        h.now = 115;
        write(&mut restored, &mut h, words[0].0, 0, pc);
        h.now = 119;
        restored.event(&mut Cx::new(&mut h, 0, pc, 5), 0);
        assert_eq!(restored.state[&words[0].0].control, 0);
        h.now = 120;
        restored.event(&mut Cx::new(&mut h, 0, pc, 5), 0);
        assert_eq!(restored.next_deadline(h.now), None);
        assert_eq!(restored.state[&words[1].0].control, 3);
        assert_eq!(h.records.len(), 2);
        assert_eq!(
            h.records[0]
                .fields
                .iter()
                .map(|(k, v)| (k.as_ref(), v.clone()))
                .collect::<Vec<_>>(),
            vec![
                ("address", Field::hex(words[0].0.into())),
                ("tick", Field::int(108)),
            ]
        );
        assert_eq!(
            h.records[1]
                .fields
                .iter()
                .map(|(k, v)| (k.as_ref(), v.clone()))
                .collect::<Vec<_>>(),
            vec![
                ("address", Field::hex(words[1].0.into())),
                ("tick", Field::int(120)),
            ]
        );
        let wrong = Clocks::new(&[(words[0].0, 9), words[1]]).unwrap();
        assert!(restored.restore(&wrong.save()).is_err());
        (restored.save(), h.records)
    }
    #[test]
    fn exact_deadlines_cancellation_restore_and_output_gate_are_program_independent() {
        assert_eq!(sequence(0), sequence(0xdeadbeef));
    }
}

pub mod host;
pub mod registers;
