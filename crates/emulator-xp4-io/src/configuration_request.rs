//! Experimental source-bound configuration retention and command-2 groups.
//! Delays and initial values are declared inputs, not physical clock laws.
use fujimod_emulator_devices::{Cx, Device, Record, Value as Field};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const DESCRIPTORS: [(u64, u64); 52] = [
    (0xff701000, 0xff701040),
    (0xff701000, 0xff701044),
    (0xff701000, 0xff701048),
    (0xff701000, 0xff70104c),
    (0xff701200, 0xff701240),
    (0xff701200, 0xff701244),
    (0xff701200, 0xff701248),
    (0xff701400, 0xff701440),
    (0xff701400, 0xff701444),
    (0xff701400, 0xff701448),
    (0xff701400, 0xff70144c),
    (0xff701600, 0xff701640),
    (0xff701600, 0xff701644),
    (0xff701600, 0xff701648),
    (0xff701600, 0xff70164c),
    (0xff701800, 0xff701840),
    (0xff701800, 0xff701844),
    (0xff701800, 0xff701848),
    (0xff701800, 0xff70184c),
    (0xff701a00, 0xff701a40),
    (0xff701a00, 0xff701a44),
    (0xff701a00, 0xff701a48),
    (0xff701a00, 0xff701a4c),
    (0xff701c00, 0xff701c40),
    (0xff701c00, 0xff701c44),
    (0xff701c00, 0xff701c48),
    (0xff701c00, 0xff701c4c),
    (0xff702000, 0xff702040),
    (0xff702000, 0xff702044),
    (0xff702000, 0xff702048),
    (0xff702000, 0xff70204c),
    (0xff702200, 0xff702240),
    (0xff703000, 0xff703040),
    (0xff704000, 0xff704040),
    (0xff704000, 0xff704044),
    (0xff707000, 0xff707040),
    (0xff708000, 0xff708040),
    (0xff70c000, 0xff70c040),
    (0xff70c000, 0xff70c044),
    (0xff70c000, 0xff70c048),
    (0xff70c000, 0xff70c04c),
    (0xff70a000, 0xff70a040),
    (0xff70a000, 0xff70a044),
    (0xff70a000, 0xff70a048),
    (0xff70b000, 0xff70b040),
    (0xff70b000, 0xff70b044),
    (0xff706000, 0xff706040),
    (0xff709000, 0xff709040),
    (0xff709000, 0xff709044),
    (0xff709000, 0xff709048),
    (0xff705000, 0xff705040),
    (0xfffae000, 0xfffae040),
];
pub const GROUP_BASES: [u64; 20] = [
    0xff701000,
    0xff701200,
    0xff701400,
    0xff701600,
    0xff701800,
    0xff701a00,
    0xff701c00,
    0xff702000,
    0xff702200,
    0xff703000,
    0xff704000,
    0xff707000,
    0xff708000,
    0xff70c000,
    0xff70a000,
    0xff70b000,
    0xff706000,
    0xff709000,
    0xff705000,
    0xfffae000,
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inputs {
    pub initial_values: Vec<u32>,
    pub completion_ticks: Vec<u64>,
}
impl Inputs {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.initial_values.len() != DESCRIPTORS.len()
            || self.completion_ticks.len() != GROUP_BASES.len()
            || self.initial_values.iter().any(|value| value & !0x1ff != 0)
            || self.completion_ticks.iter().any(|ticks| !(1..=1_000_000).contains(ticks))
        {
            return Err("Invalid explicit XP4 configuration/request reset or delay inputs".into());
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    at: u64,
    token: u64,
    values: Vec<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationRequests {
    inputs: Inputs,
    staged: Vec<u32>,
    applied: Vec<u32>,
    written: Vec<bool>,
    pending: Vec<Option<Pending>>,
    generation: u64,
    admitted: bool,
}
impl ConfigurationRequests {
    pub fn new(inputs: Inputs) -> Result<Self, String> {
        inputs.validate()?;
        Ok(Self {
            staged: inputs.initial_values.clone(), applied: inputs.initial_values.clone(),
            inputs, written: vec![false; 20], pending: vec![None; 20], generation: 0, admitted: false,
        })
    }
    pub fn windows(&self) -> Vec<(u64, u64)> {
        DESCRIPTORS.iter().map(|&(_, address)| (address, address + 4))
            .chain(GROUP_BASES.iter().map(|base| (base + 0x80, base + 0x84))).collect()
    }
    fn selector(address: u64) -> Option<usize> {
        DESCRIPTORS.iter().position(|&(_, word)| word == address)
    }
    fn group(base: u64) -> usize { GROUP_BASES.iter().position(|&b| b == base).unwrap() }
    fn request(address: u64) -> Option<usize> {
        GROUP_BASES.iter().position(|base| base + 0x80 == address)
    }
    fn values(&self, group: usize) -> Vec<u32> {
        DESCRIPTORS.iter().enumerate().filter(|(_, (base, _))| *base == GROUP_BASES[group])
            .map(|(index, _)| self.staged[index]).collect()
    }
    pub(crate) fn owns(&self, address: u64, size: u8) -> bool {
        size == 4 && (Self::selector(address).is_some() || Self::request(address).is_some())
    }
    fn admit(&mut self, cx: &mut Cx<'_>) {
        if self.admitted { return; }
        cx.record(Record::new("XP4 experimental configuration/request assumptions; no physical clock/reset effects", [
            ("profile", Field::constant(super::system_control::PROFILE)),
            ("law", Field::constant("Declared nine-bit retention and virtual command-2 acknowledgment; unmeasured delay")),
        ]));
        for (index, &(_, address)) in DESCRIPTORS.iter().enumerate() {
            cx.record(Record::new("XP4 experimental configuration reset input", [
                ("address", Field::int(address)), ("declared_reset", Field::int(self.inputs.initial_values[index])),
            ]));
        }
        for (index, base) in GROUP_BASES.iter().enumerate() {
            cx.record(Record::new("XP4 experimental request group delay input", [
                ("request_address", Field::int(base + 0x80)),
                ("ticks_unmeasured", Field::int(self.inputs.completion_ticks[index])),
            ]));
        }
        self.admitted = true;
    }
    pub(crate) fn validate_state(&self, expected: &Inputs) -> Result<(), String> {
        self.inputs.validate()?;
        if &self.inputs != expected || self.staged.len() != 52 || self.applied.len() != 52
            || self.written.len() != 20 || self.pending.len() != 20
            || self.staged.iter().chain(self.applied.iter()).any(|value| value & !0x1ff != 0)
            || self.generation == 0 && self.applied != self.inputs.initial_values
        {
            return Err("Invalid XP4 configuration/request checkpoint or changed inputs".into());
        }
        let mut tokens = BTreeSet::new();
        for group in 0..20 {
            if !self.written[group] && (self.pending[group].is_some()
                || DESCRIPTORS.iter().enumerate().any(|(index, &(base, _))| base == GROUP_BASES[group]
                    && (self.staged[index] != self.inputs.initial_values[index]
                        || self.applied[index] != self.inputs.initial_values[index])))
            {
                return Err("Invalid unwritten XP4 configuration group checkpoint".into());
            }
            if let Some(pending) = &self.pending[group] {
                if pending.token == 0 || pending.token > self.generation || !tokens.insert(pending.token)
                    || pending.at < self.inputs.completion_ticks[group] || pending.values != self.values(group)
                {
                    return Err("Invalid XP4 configuration/request pending checkpoint".into());
                }
            }
        }
        Ok(())
    }
}
impl Device for ConfigurationRequests {
    fn name(&self) -> &'static str { "xp4-functional-configuration-requests" }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if !self.owns(address, size) { cx.unmodeled(); return 0; }
        self.admit(cx);
        if let Some(index) = Self::selector(address) { return u64::from(self.staged[index]); }
        u64::from(if self.pending[Self::request(address).unwrap()].is_some() { 2u32 } else { 0 })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if !self.owns(address, size) { cx.unmodeled(); return; }
        if let Some(index) = Self::selector(address) {
            let group = Self::group(DESCRIPTORS[index].0);
            if value > 0x1ff || self.pending[group].is_some() {
                cx.fail("XP4 configuration write outside nine-bit mask or while group busy"); return;
            }
            self.admit(cx); self.staged[index] = value as u32; self.written[group] = true;
            cx.record(Record::new("XP4 experimental configuration staged", [
                ("address", Field::int(address)), ("value", Field::int(value)),
            ]));
            return;
        }
        let group = Self::request(address).unwrap();
        if value != 2 || !self.written[group] {
            cx.fail("XP4 configuration group requires prior configuration then command 2"); return;
        }
        if let Some(pending) = &self.pending[group] {
            cx.record(Record::new("XP4 experimental configuration request reissue preserves deadline", [
                ("request_address", Field::int(address)), ("deadline", Field::int(pending.at)),
            ]));
            return;
        }
        let ticks = self.inputs.completion_ticks[group];
        let (Some(at), Some(token)) = (cx.now().checked_add(ticks), self.generation.checked_add(1)) else {
            cx.fail("XP4 configuration/request deadline or generation overflow"); return;
        };
        let values = self.values(group);
        self.admit(cx); self.generation = token;
        self.pending[group] = Some(Pending { at, token, values });
        cx.record(Record::new("XP4 experimental configuration request scheduled", [
            ("request_address", Field::int(address)), ("command", Field::int(2u32)),
            ("deadline", Field::int(at)), ("ticks_unmeasured", Field::int(ticks)),
        ]));
        cx.schedule(at, token);
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.pending.iter().flatten().map(|pending| pending.at).min()
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        for group in 0..20 {
            if self.pending[group].as_ref().is_some_and(|p| p.token == token && p.at <= cx.now()) {
                let pending = self.pending[group].take().unwrap();
                let mut values = pending.values.into_iter();
                for (index, &(base, _)) in DESCRIPTORS.iter().enumerate() {
                    if base == GROUP_BASES[group] { self.applied[index] = values.next().unwrap(); }
                }
                cx.record(Record::new("XP4 experimental configuration request acknowledged; busy bits cleared", [
                    ("request_address", Field::int(GROUP_BASES[group] + 0x80)),
                    ("deadline", Field::int(pending.at)), ("tick", Field::int(cx.now())),
                ]));
            }
        }
    }
    fn save(&self) -> Value { json!(self) }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: Self = serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
        state.validate_state(&self.inputs)?; *self = state; Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::Host;
    #[derive(Default)]
    struct Clock { now: u64, events: Vec<(u64, u64)>, records: Vec<Record> }
    impl Host for Clock {
        fn now(&self) -> u64 { self.now }
        fn schedule(&mut self, _: usize, at: u64, token: u64) { self.events.push((at, token)); }
        fn set_irq(&mut self, _: u32, _: bool) { panic!("configuration requests have no IRQ") }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> { panic!("no IRQ") }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> { panic!("no DMA") }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> { panic!("no DMA") }
        fn record(&mut self, record: Record) { self.records.push(record); }
    }
    fn inputs(value: u32, ticks: u64) -> Inputs {
        Inputs { initial_values: vec![value; 52], completion_ticks: vec![ticks; 20] }
    }
    fn write(model: &mut ConfigurationRequests, clock: &mut Clock, address: u64, size: u8, value: u64) -> Result<(), String> {
        let mut cx = Cx::new(clock, 0, 0, 0); model.write(&mut cx, address, size, value);
        cx.take_refusal().map_or(Ok(()), Err)
    }
    fn read(model: &mut ConfigurationRequests, clock: &mut Clock, address: u64, size: u8) -> Result<u64, String> {
        let mut cx = Cx::new(clock, 0, 0, 0); let value = model.read(&mut cx, address, size);
        cx.take_refusal().map_or(Ok(value), Err)
    }
    #[test]
    fn all_configuration_words_have_explicit_reset_and_exact_windows() {
        for reset in [0, 0x1ff, 0x155] {
            let mut model = ConfigurationRequests::new(inputs(reset, 8)).unwrap(); let mut clock = Clock::default();
            let mut windows = model.windows(); windows.sort_unstable();
            assert_eq!(windows.len(), 72); assert!(windows.windows(2).all(|w| w[0].1 <= w[1].0));
            for &(_, address) in &DESCRIPTORS {
                assert_eq!(read(&mut model, &mut clock, address, 4).unwrap(), u64::from(reset));
                write(&mut model, &mut clock, address, 4, 0x100).unwrap();
                assert_eq!(read(&mut model, &mut clock, address, 4).unwrap(), 0x100);
                let before = model.save();
                assert!(write(&mut model, &mut clock, address, 4, 0x200).is_err());
                assert!(write(&mut model, &mut clock, address, 4, 1u64 << 32).is_err());
                for size in [1, 2, 8] {
                    assert!(read(&mut model, &mut clock, address, size).is_err());
                    assert!(write(&mut model, &mut clock, address, size, 0).is_err());
                }
                assert_eq!(model.save(), before);
            }
            for address in [0xff701a00, 0xff701a50, 0xff701a81, 0xff701a84, 0xfff48604, 0xfff4860c] {
                assert!(read(&mut model, &mut clock, address, 4).is_err());
                assert!(write(&mut model, &mut clock, address, 4, 0).is_err());
            }
        }
    }
    #[test]
    fn every_group_latches_all_members_and_completes_only_at_its_deadline() {
        for ticks in [1, 2, 8, 256, 4096] {
            let mut model = ConfigurationRequests::new(inputs(0, ticks)).unwrap(); let mut clock = Clock::default();
            for &(_, address) in &DESCRIPTORS { write(&mut model, &mut clock, address, 4, address & 0x1ff).unwrap(); }
            for group in 0..20 {
                let address = GROUP_BASES[group] + 0x80;
                write(&mut model, &mut clock, address, 4, 2).unwrap();
                let token = model.pending[group].as_ref().unwrap().token;
                let before = model.save();
                for _ in 0..20 { assert_eq!(read(&mut model, &mut clock, address, 4).unwrap(), 2); }
                assert_eq!(model.save(), before);
                clock.now = ticks - 1; model.event(&mut Cx::new(&mut clock, 0, 0, 0), token);
                assert!(model.pending[group].is_some());
                clock.now = ticks + u64::from(group == 19); model.event(&mut Cx::new(&mut clock, 0, 0, 0), token);
                assert_eq!(read(&mut model, &mut clock, address, 4).unwrap(), 0);
                for (index, &(base, _)) in DESCRIPTORS.iter().enumerate() {
                    if base == GROUP_BASES[group] { assert_eq!(model.applied[index], model.staged[index]); }
                }
                clock.now = 0;
            }
            assert_eq!(clock.events.len(), 20); assert!(model.pending.iter().all(Option::is_none));
        }
    }
    #[test]
    fn shared_groups_refuse_busy_writes_reissues_do_not_postpone_and_other_groups_are_independent() {
        let mut model = ConfigurationRequests::new(inputs(0, 8)).unwrap(); let mut clock = Clock::default();
        assert!(write(&mut model, &mut clock, GROUP_BASES[0] + 0x80, 4, 2).is_err());
        for &(_, address) in &DESCRIPTORS { write(&mut model, &mut clock, address, 4, 1).unwrap(); }
        for &base in &GROUP_BASES { write(&mut model, &mut clock, base + 0x80, 4, 2).unwrap(); }
        let before = model.save(); clock.now = 3;
        for &base in &GROUP_BASES {
            write(&mut model, &mut clock, base + 0x80, 4, 2).unwrap();
            for value in [0, 1, 3, 5, 6, 1u64 << 32] {
                assert!(write(&mut model, &mut clock, base + 0x80, 4, value).is_err());
            }
            for size in [1, 2, 8] { assert!(read(&mut model, &mut clock, base + 0x80, size).is_err()); }
        }
        for &(_, address) in &DESCRIPTORS { assert!(write(&mut model, &mut clock, address, 4, 1).is_err()); }
        assert_eq!(model.save(), before); assert_eq!(clock.events.len(), 20);
        clock.now = 8; model.event(&mut Cx::new(&mut clock, 0, 0, 0), 1);
        assert!(model.pending[0].is_none()); assert!(model.pending[1].is_some());
        write(&mut model, &mut clock, DESCRIPTORS[0].1, 4, 2).unwrap();
    }
    #[test]
    fn inputs_reject_missing_lengths_masks_delays_and_duplicate_fields() {
        for (field, length) in [("initial_values", 51), ("initial_values", 53), ("completion_ticks", 19), ("completion_ticks", 21)] {
            let mut bad = json!(inputs(0, 8)); bad[field] = json!(vec![1; length]);
            assert!(ConfigurationRequests::new(serde_json::from_value(bad).unwrap()).is_err());
        }
        for index in 0..52 { let mut bad = inputs(0, 8); bad.initial_values[index] = 0x200; assert!(ConfigurationRequests::new(bad).is_err()); }
        for index in 0..20 {
            for ticks in [0, 1_000_001, u64::MAX] { let mut bad = inputs(0, 8); bad.completion_ticks[index] = ticks; assert!(ConfigurationRequests::new(bad).is_err()); }
        }
        assert!(serde_json::from_value::<Inputs>(json!({"initial_values": []})).is_err());
        assert!(serde_json::from_str::<Inputs>(r#"{"initial_values":[],"initial_values":[],"completion_ticks":[]}"#).is_err());
    }
    #[test]
    fn checkpoint_roundtrip_refuses_bad_masks_lengths_inputs_and_pending_tokens() {
        let mut model = ConfigurationRequests::new(inputs(0, 8)).unwrap(); let mut clock = Clock::default();
        for &(_, address) in &DESCRIPTORS { write(&mut model, &mut clock, address, 4, 1).unwrap(); }
        for &base in &GROUP_BASES { write(&mut model, &mut clock, base + 0x80, 4, 2).unwrap(); }
        let saved = model.save(); let mut resumed = ConfigurationRequests::new(inputs(0, 8)).unwrap(); resumed.restore(&saved).unwrap();
        for field in ["staged", "applied"] {
            let mut bad = saved.clone(); bad[field][0] = json!(0x200); assert!(resumed.restore(&bad).is_err());
            let mut bad = saved.clone(); bad[field].as_array_mut().unwrap().pop(); assert!(resumed.restore(&bad).is_err());
        }
        for (field, value) in [("token", json!(0)), ("token", json!(21)), ("at", json!(0)), ("values", json!([]))] {
            let mut bad = saved.clone(); bad["pending"][0][field] = value; assert!(resumed.restore(&bad).is_err());
        }
        let mut bad = saved.clone(); bad["pending"][1]["token"] = bad["pending"][0]["token"].clone(); assert!(resumed.restore(&bad).is_err());
        let mut bad = saved.clone(); bad["written"][0] = json!(false); assert!(resumed.restore(&bad).is_err());
        assert!(ConfigurationRequests::new(inputs(0, 9)).unwrap().restore(&saved).is_err());
        assert_eq!(resumed.save(), saved);
        clock.now = 8;
        for token in 1..=20 {
            model.event(&mut Cx::new(&mut clock, 0, 0, 0), token);
            resumed.event(&mut Cx::new(&mut clock, 0, 0, 0), token);
        }
        assert_eq!(model.save(), resumed.save()); resumed.restore(&model.save()).unwrap();
    }
    #[test]
    fn stale_idle_events_and_overflow_never_complete_or_mutate_requests() {
        let mut model = ConfigurationRequests::new(inputs(0, 8)).unwrap(); let mut clock = Clock::default();
        let before = model.save(); model.event(&mut Cx::new(&mut clock, 0, 0, 0), 1); assert_eq!(model.save(), before);
        write(&mut model, &mut clock, DESCRIPTORS[0].1, 4, 1).unwrap();
        clock.now = u64::MAX; let before = model.save();
        assert!(write(&mut model, &mut clock, GROUP_BASES[0] + 0x80, 4, 2).is_err()); assert_eq!(model.save(), before);
        clock.now = 0; model.generation = u64::MAX; let before = model.save();
        assert!(write(&mut model, &mut clock, GROUP_BASES[0] + 0x80, 4, 2).is_err()); assert_eq!(model.save(), before);
        model.generation = 0; write(&mut model, &mut clock, GROUP_BASES[0] + 0x80, 4, 2).unwrap();
        clock.now = 8; model.event(&mut Cx::new(&mut clock, 0, 0, 0), 1);
        write(&mut model, &mut clock, GROUP_BASES[0] + 0x80, 4, 2).unwrap(); let before = model.save();
        model.event(&mut Cx::new(&mut clock, 0, 0, 0), 1); assert_eq!(model.save(), before);
    }
    #[test]
    fn identical_register_and_time_trace_is_cpu_pc_independent() {
        let mut outputs = Vec::new();
        for (cpu, pc) in [(0, 0), (3, 0xdead0000)] {
            let mut model = ConfigurationRequests::new(inputs(0, 8)).unwrap(); let mut clock = Clock::default();
            for &(_, address) in &DESCRIPTORS { model.write(&mut Cx::new(&mut clock, cpu, pc, 0), address, 4, 0x100); }
            for &base in &GROUP_BASES { model.write(&mut Cx::new(&mut clock, cpu, pc, 0), base + 0x80, 4, 2); }
            clock.now = 8;
            for token in 1..=20 { model.event(&mut Cx::new(&mut clock, cpu, pc, 0), token); }
            outputs.push((model.save(), json!(clock.records), clock.events));
        }
        assert_eq!(outputs[0], outputs[1]);
    }
}
