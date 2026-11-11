//! Opt-in functional hypotheses for source-bound XP4 control words and request banks.
//! Retention, reset values and completion latency are declared experimental
//! inputs, not measured clock/reset laws. No IRQ, DMA or CPU state is used.
use fujimod_emulator_devices::{Cx, Device, Record, Value as Field};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const CONTROL: u64 = 0xff70f03c;
pub const VALUE: u64 = 0xff701240;
pub const REQUEST: u64 = 0xff701280;
pub const PROFILE: &str = "experimental-source-functional-v1";
pub const BOOLEAN_WRITE_PORT: u64 = 0xfff4a400;
pub const READ_WORD_FFF48004: u64 = 0xfff48004;
pub const SERIAL_CLOCK_SELECT: u64 = 0xfff48700;
/// A single two-bit selector lane, or zero. No frequency/readback law.
fn serial_clock_word(value: u32) -> bool {
    (0..6).any(|lane| value & !(3u32 << (lane * 2)) == 0)
}

pub const A_WORDS: [(u64, u32); 16] = [
    (0xff70f000, 0x00000007),
    (0xff70f004, 0x000fffff),
    (0xff70f008, 0x01ffffff),
    (0xff70f00c, 0x000000ff),
    (0xff70f010, 0x000fffff),
    (0xff70f014, 0x0001ffff),
    (0xff70f018, 0x00000007),
    (0xff70f01c, 0x0000007b),
    (0xff70f020, 0x00000007),
    (0xff70f024, 0x000000ff),
    (0xff70f028, 0x00000003),
    (0xff70f02c, 0x00000003),
    (0xff70f030, 0x0000001c),
    (0xff70f034, 0x0001ffdf),
    (0xff70f038, 0x000fffff),
    (0xff70f03c, 0x000007ff),
];
pub const B_WORDS: [(u64, u32); 15] = [
    (0xfff48300, 0x00000007),
    (0xfff48304, 0x00000001),
    (0xfff48308, 0x00000fff),
    (0xfff4830c, 0x000001ff),
    (0xfff48310, 0x00000003),
    (0xfff48314, 0x00070f3f),
    (0xfff48318, 0x0000001f),
    (0xfff4831c, 0x0000003f),
    (0xfff48320, 0x00000003),
    (0xfff48324, 0x00000003),
    (0xfff48328, 0x00000001),
    (0xfff4832c, 0x0000001f),
    (0xfff48330, 0x0000007f),
    (0xfff48334, 0x000000cf),
    (0xfff48608, 0x00000007),
];

/// Explicit reset inputs for the exact source-derived request tables.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestBanks {
    pub a_reset: [u32; 16],
    pub b_reset: [u32; 15],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Graphics {
    pub initial_value: u32,
    pub completion_ticks: u64,
}
/// Optional write-port acceptance. Physical effects, reset and readback
/// semantics are unknown; this option supplies none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WritePortEnable {
    pub enabled: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteOnlyState {
    last_write: Option<u32>,
}
/// Explicit external word. Hardware meaning and reset value are unknown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadOnlyWordInput { pub value: u32 }
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub read_word_fff48004: Option<ReadOnlyWordInput>,
    pub profile: String,
    pub control_f03c_reset: u32,
    #[serde(default)]
    pub graphics: Option<Graphics>,
    #[serde(default)]
    pub retained_request_banks: Option<RequestBanks>,
    #[serde(default)]
    pub configuration_requests: Option<crate::configuration_request::Inputs>,
    #[serde(default)]
    pub selection_special: Option<crate::selection_special::Inputs>,
    #[serde(default)]
    pub boolean_write_fff4a400: Option<WritePortEnable>,
    #[serde(default)]
    pub serial_clock_select_fff48700: Option<WritePortEnable>,
}
impl Config {
    fn validate(&self) -> Result<(), String> {
        if self.profile != PROFILE || self.control_f03c_reset & !0x7ff != 0
            || self.boolean_write_fff4a400.as_ref().is_some_and(|port| !port.enabled)
            || self.serial_clock_select_fff48700.as_ref().is_some_and(|port| !port.enabled)
            || self.graphics.as_ref().is_some_and(|g| g.initial_value & !0x1ff != 0 || g.completion_ticks == 0)
        {
            return Err("Invalid experimental XP4 functional system-control input".into());
        }
        if let Some(inputs) = &self.selection_special { inputs.validate()?; }
        if let Some(inputs) = &self.configuration_requests {
            inputs.validate()?;
            if self.graphics.is_some() {
                return Err("XP4 graphics and configuration/request family inputs overlap".into());
            }
        }
        if let Some(banks) = &self.retained_request_banks
            && (banks.a_reset[15] != self.control_f03c_reset
                || banks
                    .a_reset
                    .iter()
                    .zip(A_WORDS)
                    .any(|(value, (_, mask))| value & !mask != 0)
                || banks
                    .b_reset
                    .iter()
                    .zip(B_WORDS)
                    .any(|(value, (_, mask))| value & !mask != 0))
        {
            return Err("Invalid XP4 request-bank reset inputs or conflicting A15 reset".into());
        }
        Ok(())
    }
}
/// A15 lives only in SystemControl.control; no duplicate mutable A15 state.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedBanks {
    a: [u32; 15],
    b: [u32; 15],
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    at: u64,
    token: u64,
    value: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemControl {
    config: Config,
    control: u32,
    #[serde(default)]
    retained_banks: Option<RetainedBanks>,
    #[serde(default)]
    configuration_state: Option<crate::configuration_request::ConfigurationRequests>,
    #[serde(default)]
    selection_state: Option<crate::selection_special::SelectionSpecial>,
    #[serde(default)]
    boolean_write_state: Option<WriteOnlyState>,
    #[serde(default)]
    serial_clock_state: Option<WriteOnlyState>,
    staged: u32,
    applied: u32,
    value_written: bool,
    pending: Option<Pending>,
    generation: u64,
    admitted: bool,
}
impl SystemControl {
    pub fn new(config: Config) -> Result<Self, String> {
        config.validate()?;
        let initial = config.graphics.as_ref().map_or(0, |g| g.initial_value);
        let retained_banks = config.retained_request_banks.as_ref().map(|banks| RetainedBanks {
            a: banks.a_reset[..15].try_into().unwrap(), b: banks.b_reset,
        });
        let configuration_state = config.configuration_requests.clone()
            .map(crate::configuration_request::ConfigurationRequests::new).transpose()?;
        let selection_state = config.selection_special.clone()
            .map(crate::selection_special::SelectionSpecial::new).transpose()?;
        let boolean_write_state = config.boolean_write_fff4a400.as_ref()
            .map(|_| WriteOnlyState { last_write: None });
        let serial_clock_state = config.serial_clock_select_fff48700.as_ref()
            .map(|_| WriteOnlyState { last_write: None });
        Ok(Self { control: config.control_f03c_reset, retained_banks, configuration_state, selection_state, boolean_write_state, serial_clock_state, config, staged: initial,
            applied: initial, value_written: false, pending: None, generation: 0, admitted: false })
    }
    pub fn windows(&self) -> Vec<(u64, u64)> {
        let mut spans = vec![(CONTROL, CONTROL + 4)];
        if self.config.read_word_fff48004.is_some() { spans.push((READ_WORD_FFF48004, READ_WORD_FFF48004 + 4)); }
        if self.retained_banks.is_some() {
            spans.extend(A_WORDS[..15].iter().chain(B_WORDS.iter()).map(|&(address, _)| (address, address + 4)));
        }
        if let Some(model) = &self.configuration_state { spans.extend(model.windows()); }
        if let Some(model) = &self.selection_state { spans.extend(model.windows()); }
        if self.boolean_write_state.is_some() { spans.push((BOOLEAN_WRITE_PORT, BOOLEAN_WRITE_PORT + 4)); }
        if self.serial_clock_state.is_some() { spans.push((SERIAL_CLOCK_SELECT, SERIAL_CLOCK_SELECT + 4)); }
        if self.config.graphics.is_some() { spans.extend([(VALUE, VALUE + 4), (REQUEST, REQUEST + 4)]); }
        spans
    }
    fn request_word(&self, address: u64) -> Option<(bool, usize, u32)> {
        self.retained_banks.as_ref()?;
        A_WORDS[..15].iter().position(|&(a, _)| a == address)
            .map(|index| (true, index, A_WORDS[index].1))
            .or_else(|| B_WORDS.iter().position(|&(a, _)| a == address)
                .map(|index| (false, index, B_WORDS[index].1)))
    }
    fn known(&self, address: u64, size: u8) -> bool {
        size == 4 && (address == CONTROL || self.request_word(address).is_some()
            || address == BOOLEAN_WRITE_PORT && self.boolean_write_state.is_some()
            || address == SERIAL_CLOCK_SELECT && self.serial_clock_state.is_some()
            || self.selection_state.as_ref().is_some_and(|model| model.owns(address, size))
            || self.configuration_state.as_ref().is_some_and(|model| model.owns(address, size)) || self.config.graphics.is_some() && matches!(address, VALUE | REQUEST))
    }
    fn admit(&mut self, cx: &mut Cx<'_>) {
        if !self.admitted {
            cx.record(Record::new("XP4 experimental functional system-control assumptions", [
                ("profile", Field::constant(PROFILE)),
                ("reset_control", Field::int(self.config.control_f03c_reset)),
                ("graphics_enabled", Field::int(u32::from(self.config.graphics.is_some()))),
                ("boolean_write_fff4a400_enabled", Field::int(u32::from(self.boolean_write_state.is_some()))),
                ("initial_graphics_value", Field::int(self.config.graphics.as_ref().map_or(0, |g| g.initial_value))),
                ("completion_ticks_unmeasured", Field::int(self.config.graphics.as_ref().map_or(0, |g| g.completion_ticks))),
                ("law", Field::constant("Retained control; optional delayed command-2 acknowledgment; no physical clock/reset claim")),
            ]));
            if let Some(banks) = &self.config.retained_request_banks {
                for ((address, mask), reset) in A_WORDS.iter().chain(B_WORDS.iter())
                    .zip(banks.a_reset.iter().chain(banks.b_reset.iter()))
                {
                    cx.record(Record::new("XP4 experimental retained request-bank reset; no physical clock/reset effects", [
                        ("address", Field::int(*address)), ("software_mask", Field::int(*mask)),
                        ("declared_reset", Field::int(*reset)),
                    ]));
                }
            }
            self.admitted = true;
        }
    }
}
impl Device for SystemControl {
    fn name(&self) -> &'static str { "xp4-functional-system-control" }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if address == READ_WORD_FFF48004 {
            if size == 4
                && let Some(input) = &self.config.read_word_fff48004
            {
                return u64::from(input.value);
            }
            cx.unmodeled(); return 0;
        }
        if matches!(address, BOOLEAN_WRITE_PORT | SERIAL_CLOCK_SELECT) { cx.unmodeled(); return 0; }
        if !self.known(address, size) { cx.unmodeled(); return 0; }
        self.admit(cx);
        if let Some(model) = &mut self.configuration_state
            && model.owns(address, size)
        {
            return model.read(cx, address, size);
        }
        if let Some(model) = &mut self.selection_state
            && model.owns(address, size)
        {
            return model.read(cx, address, size);
        }
        if let Some((is_a, index, _)) = self.request_word(address) {
            let banks = self.retained_banks.as_ref().unwrap();
            return u64::from(if is_a { banks.a[index] } else { banks.b[index] });
        }
        u64::from(match address { CONTROL => self.control, VALUE => self.staged, _ => if self.pending.is_some() { 2 } else { 0 } })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if !self.known(address, size) { cx.unmodeled(); return; }
        if value > u32::MAX as u64 { cx.fail("XP4 functional control requires a 32-bit value"); return; }
        if address == SERIAL_CLOCK_SELECT {
            if !serial_clock_word(value as u32) { cx.fail("XP4 serial clock write requires one two-bit selector lane"); return; }
            self.admit(cx);
            self.serial_clock_state.as_mut().unwrap().last_write = Some(value as u32);
            cx.record(Record::new("XP4 experimental serial clock selection write; physical effects unknown", [
                ("address", Field::int(address)), ("value", Field::int(value)),
                ("law", Field::constant("Accepted write only; no reset, readback, frequency, scheduling, IRQ or DMA effects")),
            ]));
            return;
        }
        if address == BOOLEAN_WRITE_PORT {
            if value > 1 { cx.fail("XP4 fff4a400 write port accepts only boolean values"); return; }
            self.admit(cx);
            self.boolean_write_state.as_mut().unwrap().last_write = Some(value as u32);
            cx.record(Record::new("XP4 experimental fff4a400 boolean write; physical effects unknown", [
                ("address", Field::int(address)), ("value", Field::int(value)),
                ("law", Field::constant("Accepted write only; no reset, readback, readiness, release, clock, IRQ or DMA effects")),
            ]));
            return;
        }
        if let Some(model) = &mut self.configuration_state
            && model.owns(address, size)
        {
            model.write(cx, address, size, value);
            return;
        }
        if let Some(model) = &mut self.selection_state
            && model.owns(address, size)
        {
            model.write(cx, address, size, value);
            return;
        }
        let value = value as u32;
        if let Some((is_a, index, mask)) = self.request_word(address) {
            if value & !mask != 0 {
                cx.fail("XP4 retained request-bank write outside software-derived mask"); return;
            }
            self.admit(cx);
            let banks = self.retained_banks.as_mut().unwrap();
            if is_a { banks.a[index] = value; } else { banks.b[index] = value; }
            cx.record(Record::new("XP4 experimental retained request-bank write", [
                ("address", Field::int(address)), ("value", Field::int(value)),
                ("software_mask", Field::int(mask)),
            ]));
            return;
        }
        match address {
            CONTROL => {
                if value & !0x7ff != 0 { cx.fail("XP4 functional control bits outside admitted mask 0x7ff"); return; }
                self.admit(cx); self.control = value;
            }
            VALUE => {
                if value & !0x1ff != 0 || self.pending.is_some() {
                    cx.fail("XP4 functional graphics configuration outside mask or while busy"); return;
                }
                self.admit(cx); self.staged = value; self.value_written = true;
            }
            _ => {
                if value != 2 || !self.value_written {
                    cx.fail("XP4 functional graphics requires configuration then command 2; cancellation is not admitted"); return;
                }
                if let Some(pending) = &self.pending {
                    cx.record(Record::new("XP4 experimental command 2 reissue preserves deadline", [("deadline", Field::int(pending.at))]));
                    return;
                }
                let ticks = self.config.graphics.as_ref().unwrap().completion_ticks;
                let (Some(at), Some(token)) = (cx.now().checked_add(ticks), self.generation.checked_add(1)) else {
                    cx.fail("XP4 functional graphics deadline or generation overflow"); return;
                };
                self.admit(cx); self.generation = token;
                self.pending = Some(Pending { at, token, value: self.staged });
                cx.record(Record::new("XP4 experimental command 2 scheduled", [
                    ("value", Field::int(self.staged)), ("deadline", Field::int(at)), ("ticks_unmeasured", Field::int(ticks)),
                ]));
                cx.schedule(at, token);
            }
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.pending.as_ref().map(|p| p.at).into_iter()
            .chain(self.configuration_state.as_ref().and_then(|model| model.next_deadline(now))).min()
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if let Some(model) = &mut self.configuration_state { model.event(cx, token); }
        if self.pending.as_ref().is_some_and(|p| p.token == token && p.at <= cx.now()) {
            let pending = self.pending.take().unwrap(); self.applied = pending.value;
            cx.record(Record::new("XP4 experimental command 2 acknowledged; busy bits cleared", [
                ("value", Field::int(pending.value)), ("deadline", Field::int(pending.at)), ("tick", Field::int(cx.now())),
            ]));
        }
    }
    fn save(&self) -> Value { json!(self) }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: Self = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        state.config.validate()?;
        let selection_valid = match (&state.selection_state, &state.config.selection_special) {
            (None, None) => true,
            (Some(model), Some(inputs)) => model.validate_state(inputs).is_ok(),
            _ => false,
        };
        let boolean_write_valid = match (&state.boolean_write_state, &state.config.boolean_write_fff4a400) {
            (None, None) => true,
            (Some(port), Some(_)) => port.last_write.is_none_or(|value| value <= 1)
                && (port.last_write.is_none() || state.admitted),
            _ => false,
        };
        let serial_clock_valid = match (&state.serial_clock_state, &state.config.serial_clock_select_fff48700) {
            (None, None) => true,
            (Some(port), Some(_)) => port.last_write.is_none_or(serial_clock_word)
                && (port.last_write.is_none() || state.admitted),
            _ => false,
        };
        let configuration_valid = match (&state.configuration_state, &state.config.configuration_requests) {
            (None, None) => true,
            (Some(model), Some(inputs)) => model.validate_state(inputs).is_ok(),
            _ => false,
        };
        let banks_valid = match (&state.retained_banks, &state.config.retained_request_banks) {
            (None, None) => true,
            (Some(banks), Some(_)) => banks.a.iter().zip(A_WORDS).all(|(value, (_, mask))| value & !mask == 0)
                && banks.b.iter().zip(B_WORDS).all(|(value, (_, mask))| value & !mask == 0),
            _ => false,
        };
        let no_graphics = state.config.graphics.is_none();
        let initial = state.config.graphics.as_ref().map_or(0, |g| g.initial_value);
        if !serial_clock_valid || !boolean_write_valid || !selection_valid || !configuration_valid || !banks_valid || (!state.value_written && (state.staged != initial || state.applied != initial || state.generation != 0))
            || (state.generation == 0 && state.applied != initial)
            || state.config != self.config || state.control & !0x7ff != 0 || state.staged & !0x1ff != 0 || state.applied & !0x1ff != 0
            || no_graphics && (state.staged != 0 || state.applied != 0 || state.value_written || state.pending.is_some() || state.generation != 0)
            || state.pending.as_ref().is_some_and(|p| !state.value_written || p.value != state.staged || p.token == 0 || p.token != state.generation
                || p.at < state.config.graphics.as_ref().map_or(u64::MAX, |g| g.completion_ticks))
        { return Err("Invalid XP4 functional system-control checkpoint or changed declared inputs".into()); }
        *self = state; Ok(())
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
        fn set_irq(&mut self, _: u32, _: bool) { panic!("system control must not produce IRQs") }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> { panic!("no IRQ") }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> { panic!("no DMA") }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> { panic!("no DMA") }
        fn record(&mut self, r: Record) { self.records.push(r); }
    }
    fn config(ticks: Option<u64>) -> Config {
        Config { read_word_fff48004: None, profile: PROFILE.into(), control_f03c_reset: 7, retained_request_banks: None, configuration_requests: None, selection_special: None, boolean_write_fff4a400: None, serial_clock_select_fff48700: None,
            graphics: ticks.map(|completion_ticks| Graphics { initial_value: 0, completion_ticks }) }
    }
    fn write(d: &mut SystemControl, c: &mut Clock, address: u64, size: u8, value: u64) -> Result<(), String> {
        let mut cx = Cx::new(c, 0, 0, 0); d.write(&mut cx, address, size, value);
        cx.take_refusal().map_or(Ok(()), Err)
    }
    fn read(d: &mut SystemControl, c: &mut Clock, address: u64, size: u8) -> Result<u64, String> {
        let mut cx = Cx::new(c, 0, 0, 0); let v = d.read(&mut cx, address, size);
        cx.take_refusal().map_or(Ok(v), Err)
    }
    #[test]
    fn explicit_read_word_is_immutable_and_context_independent() {
        for value in [0, 0xa5a5_123f] {
            let mut cfg = config(None); cfg.read_word_fff48004 = Some(ReadOnlyWordInput { value });
            let mut d = SystemControl::new(cfg).unwrap(); let mut c = Clock::default();
            let before = d.save();
            for (core, pc) in [(0, 0), (3, 0x12345678)] {
                let mut cx = Cx::new(&mut c, core, pc, 0);
                assert_eq!(d.read(&mut cx, READ_WORD_FFF48004, 4), u64::from(value));
                assert!(cx.take_refusal().is_none());
            }
            assert_eq!(d.save(), before); assert!(c.records.is_empty()); assert!(c.events.is_empty());
            for size in [1, 2, 8] { assert!(read(&mut d, &mut c, READ_WORD_FFF48004, size).is_err()); }
            assert!(read(&mut d, &mut c, READ_WORD_FFF48004 + 1, 4).is_err());
            for size in [1, 2, 4, 8] { assert!(write(&mut d, &mut c, READ_WORD_FFF48004, size, 0).is_err()); }
            assert_eq!(d.save(), before); assert!(c.records.is_empty()); assert!(c.events.is_empty());
        }
        let mut strict = SystemControl::new(config(None)).unwrap(); let mut c = Clock::default();
        let before = strict.save();
        assert!(read(&mut strict, &mut c, READ_WORD_FFF48004, 4).is_err());
        assert!(!strict.windows().contains(&(READ_WORD_FFF48004, READ_WORD_FFF48004 + 4)));
        assert_eq!(strict.save(), before); assert!(c.records.is_empty());
    }
    #[test]
    fn read_word_checkpoint_binds_presence_and_value_atomically() {
        let mut cfg = config(None); cfg.read_word_fff48004 = Some(ReadOnlyWordInput { value: 7 });
        let mut d = SystemControl::new(cfg.clone()).unwrap(); let good = d.save();
        d.restore(&good).unwrap();
        for input in [None, Some(ReadOnlyWordInput { value: 8 })] {
            let mut changed = cfg.clone(); changed.read_word_fff48004 = input;
            let other = SystemControl::new(changed).unwrap().save();
            assert!(d.restore(&other).is_err()); assert_eq!(d.save(), good);
        }
        let mut injected = good.clone(); injected["config"]["read_word_fff48004"]["value"] = json!(4294967296u64);
        assert!(d.restore(&injected).is_err()); assert_eq!(d.save(), good);
        assert!(serde_json::from_value::<ReadOnlyWordInput>(json!({})).is_err());
    }
    fn boolean_config() -> Config {
        let mut inputs = config(None);
        inputs.boolean_write_fff4a400 = Some(WritePortEnable { enabled: true });
        inputs
    }
    #[test]
    fn boolean_write_port_has_no_reset_or_readback_and_records_both_values() {
        let mut d = SystemControl::new(boolean_config()).unwrap();
        let mut c = Clock::default();
        assert!(d.windows().contains(&(BOOLEAN_WRITE_PORT, BOOLEAN_WRITE_PORT + 4)));
        assert_eq!(d.save()["boolean_write_state"]["last_write"], Value::Null);
        for value in [1, 0, 0, 1] {
            write(&mut d, &mut c, BOOLEAN_WRITE_PORT, 4, value).unwrap();
            assert_eq!(d.save()["boolean_write_state"]["last_write"], json!(value));
            assert_eq!(d.control, 7);
            assert_eq!(d.next_deadline(c.now), None);
        }
        assert_eq!(c.records.len(), 5); // One admission plus four stores.
        assert!(c.events.is_empty());
        let before = d.save();
        let records = c.records.len();
        assert!(read(&mut d, &mut c, BOOLEAN_WRITE_PORT, 4).is_err());
        assert_eq!(d.save(), before);
        assert_eq!(c.records.len(), records);
    }
    #[test]
    fn boolean_write_port_refuses_before_state_or_admission_mutation() {
        let mut d = SystemControl::new(boolean_config()).unwrap();
        let mut c = Clock::default();
        let before = d.save();
        for (address, size, value) in [(BOOLEAN_WRITE_PORT, 4, 2),
            (BOOLEAN_WRITE_PORT, 4, 0x100), (BOOLEAN_WRITE_PORT, 4, u64::MAX),
            (BOOLEAN_WRITE_PORT, 1, 1), (BOOLEAN_WRITE_PORT, 2, 1),
            (BOOLEAN_WRITE_PORT, 8, 1), (BOOLEAN_WRITE_PORT + 1, 4, 1),
            (BOOLEAN_WRITE_PORT - 4, 4, 1), (BOOLEAN_WRITE_PORT + 4, 4, 1)] {
            assert!(write(&mut d, &mut c, address, size, value).is_err());
            assert_eq!(d.save(), before);
            assert!(c.records.is_empty() && c.events.is_empty());
        }
        for size in [1, 2, 4, 8] {
            assert!(read(&mut d, &mut c, BOOLEAN_WRITE_PORT, size).is_err());
            assert_eq!(d.save(), before);
            assert!(c.records.is_empty());
        }
        let mut strict = SystemControl::new(config(None)).unwrap();
        assert!(!strict.windows().contains(&(BOOLEAN_WRITE_PORT, BOOLEAN_WRITE_PORT + 4)));
        assert!(write(&mut strict, &mut c, BOOLEAN_WRITE_PORT, 4, 1).is_err());
        assert!(c.records.is_empty());
    }
    #[test]
    fn boolean_write_port_checkpoint_binds_presence_inputs_and_boolean_value() {
        let mut d = SystemControl::new(boolean_config()).unwrap();
        let initial = d.save();
        d.restore(&initial).unwrap();
        write(&mut d, &mut Clock::default(), BOOLEAN_WRITE_PORT, 4, 1).unwrap();
        let written = d.save();
        let mut other = SystemControl::new(boolean_config()).unwrap();
        other.restore(&written).unwrap();
        assert_eq!(other.save(), written);
        for value in [json!(2), json!(-1), json!(true), json!("1")] {
            let mut bad = written.clone();
            bad["boolean_write_state"]["last_write"] = value;
            assert!(other.restore(&bad).is_err());
            assert_eq!(other.save(), written);
        }
        for path in ["boolean_write_state", "config"] {
            let mut bad = written.clone();
            if path == "config" { bad["config"]["boolean_write_fff4a400"] = Value::Null; }
            else { bad[path] = Value::Null; }
            assert!(other.restore(&bad).is_err());
            assert_eq!(other.save(), written);
        }
        let mut bad = written.clone(); bad["admitted"] = json!(false);
        assert!(other.restore(&bad).is_err());
        let mut strict = SystemControl::new(config(None)).unwrap();
        let before = strict.save();
        assert!(strict.restore(&written).is_err());
        assert_eq!(strict.save(), before);
        assert!(d.restore(&before).is_err());
        for port in [json!(true), json!({}), json!({"enabled": 1}),
            json!({"enabled": true, "initial_value": 0})] {
            let mut bad = serde_json::to_value(config(None)).unwrap();
            bad["boolean_write_fff4a400"] = port;
            assert!(serde_json::from_value::<Config>(bad).is_err());
        }
        let mut disabled = boolean_config();
        disabled.boolean_write_fff4a400.as_mut().unwrap().enabled = false;
        assert!(SystemControl::new(disabled).is_err());
    }
    #[test]
    fn boolean_write_port_is_independent_of_core_pc_and_time() {
        let mut states = Vec::new();
        for (core, pc, now) in [(0, 0x1234, 0), (3, 0x5678, 12345)] {
            let mut d = SystemControl::new(boolean_config()).unwrap();
            let mut c = Clock { now, ..Default::default() };
            for value in [1, 0, 1] {
                let mut cx = Cx::new(&mut c, core, pc, 0);
                d.write(&mut cx, BOOLEAN_WRITE_PORT, 4, value);
                assert!(cx.take_refusal().is_none());
            }
            assert!(c.events.is_empty());
            states.push(d.save());
        }
        assert_eq!(states[0], states[1]);
    }
    fn serial_clock_config() -> Config {
        let mut c = config(None);
        c.serial_clock_select_fff48700 = Some(WritePortEnable { enabled: true });
        c
    }
    #[test]
    fn serial_clock_accepts_only_one_selector_lane_and_never_reads_back() {
        let mut d = SystemControl::new(serial_clock_config()).unwrap();
        let mut c = Clock::default();
        assert_eq!(d.save()["serial_clock_state"]["last_write"], Value::Null);
        let mut admitted = 0;
        for value in 0..4096u64 {
            let valid = (0..6).any(|lane| value & !(3u64 << (lane * 2)) == 0);
            let before = d.save(); let records = c.records.len();
            let result = write(&mut d, &mut c, SERIAL_CLOCK_SELECT, 4, value);
            assert_eq!(result.is_ok(), valid, "value {value:x}");
            if valid {
                admitted += 1;
                assert_eq!(d.save()["serial_clock_state"]["last_write"], json!(value));
            } else { assert_eq!(d.save(), before); assert_eq!(c.records.len(), records); }
        }
        assert_eq!(admitted, 19);
        assert!(c.events.is_empty());
        let before = d.save(); let records = c.records.len();
        assert!(read(&mut d, &mut c, SERIAL_CLOCK_SELECT, 4).is_err());
        assert_eq!(d.save(), before); assert_eq!(c.records.len(), records);
        assert_eq!(d.next_deadline(c.now), None);
    }
    #[test]
    fn serial_clock_refuses_invalid_access_before_admission_or_state_changes() {
        let mut d = SystemControl::new(serial_clock_config()).unwrap();
        let mut c = Clock::default(); let before = d.save();
        for (address, size, value) in [(SERIAL_CLOCK_SELECT, 4, 0x1000),
            (SERIAL_CLOCK_SELECT, 4, u64::MAX), (SERIAL_CLOCK_SELECT, 4, 5),
            (SERIAL_CLOCK_SELECT, 1, 4), (SERIAL_CLOCK_SELECT, 2, 4),
            (SERIAL_CLOCK_SELECT, 8, 4), (SERIAL_CLOCK_SELECT + 1, 4, 4),
            (SERIAL_CLOCK_SELECT - 4, 4, 4), (SERIAL_CLOCK_SELECT + 4, 4, 4)] {
            assert!(write(&mut d, &mut c, address, size, value).is_err());
            assert_eq!(d.save(), before); assert!(c.records.is_empty() && c.events.is_empty());
        }
        for size in [1, 2, 4, 8] {
            assert!(read(&mut d, &mut c, SERIAL_CLOCK_SELECT, size).is_err());
            assert_eq!(d.save(), before); assert!(c.records.is_empty());
        }
        let mut strict = SystemControl::new(config(None)).unwrap();
        assert!(!strict.windows().contains(&(SERIAL_CLOCK_SELECT, SERIAL_CLOCK_SELECT + 4)));
        assert!(write(&mut strict, &mut c, SERIAL_CLOCK_SELECT, 4, 4).is_err());
    }
    #[test]
    fn serial_clock_checkpoint_requires_exact_inputs_presence_and_single_lane() {
        let mut d = SystemControl::new(serial_clock_config()).unwrap();
        d.restore(&d.save()).unwrap();
        write(&mut d, &mut Clock::default(), SERIAL_CLOCK_SELECT, 4, 4).unwrap();
        let state = d.save();
        let mut restored = SystemControl::new(serial_clock_config()).unwrap();
        restored.restore(&state).unwrap(); assert_eq!(restored.save(), state);
        for value in [json!(5), json!(4096), json!(-1), json!(true)] {
            let mut bad = state.clone(); bad["serial_clock_state"]["last_write"] = value;
            assert!(restored.restore(&bad).is_err()); assert_eq!(restored.save(), state);
        }
        for key in ["serial_clock_state", "config", "admitted"] {
            let mut bad = state.clone();
            match key {
                "config" => bad["config"]["serial_clock_select_fff48700"] = Value::Null,
                "admitted" => bad["admitted"] = json!(false),
                _ => bad[key] = Value::Null,
            }
            assert!(restored.restore(&bad).is_err()); assert_eq!(restored.save(), state);
        }
        let mut strict = SystemControl::new(config(None)).unwrap();
        assert!(strict.restore(&state).is_err()); assert!(d.restore(&strict.save()).is_err());
        let mut disabled = serial_clock_config();
        disabled.serial_clock_select_fff48700.as_mut().unwrap().enabled = false;
        assert!(SystemControl::new(disabled).is_err());
    }
    #[test]
    fn serial_clock_writes_do_not_depend_on_core_pc_or_time() {
        let mut states = Vec::new();
        for (core, pc, now) in [(0, 0x1234, 0), (3, 0x5678, 9876)] {
            let mut d = SystemControl::new(serial_clock_config()).unwrap();
            let mut c = Clock { now, ..Default::default() };
            for value in [4, 0, 0xc00] {
                let mut cx = Cx::new(&mut c, core, pc, 0);
                d.write(&mut cx, SERIAL_CLOCK_SELECT, 4, value);
                assert!(cx.take_refusal().is_none());
            }
            assert!(c.events.is_empty()); states.push(d.save());
        }
        assert_eq!(states[0], states[1]);
    }
    #[test]
    fn idle_event_is_a_noop() {
        let mut d = SystemControl::new(config(Some(2))).unwrap();
        let mut c = Clock::default();
        let before = d.save();
        d.event(&mut Cx::new(&mut c, 0, 0, 0), 1);
        assert_eq!(d.save(), before);
        assert!(c.events.is_empty() && c.records.is_empty());
    }
    #[test]
    fn required_inputs_and_bounds_are_explicit() {
        for v in [json!({}), json!({"profile": PROFILE}), json!({"profile":PROFILE,"control_f03c_reset":0,"extra":1})] {
            assert!(serde_json::from_value::<Config>(v).is_err());
        }
        let mut x = config(None); x.profile = "default".into(); assert!(SystemControl::new(x).is_err());
        let mut x = config(None); x.control_f03c_reset = 0x800; assert!(SystemControl::new(x).is_err());
        assert!(SystemControl::new(config(Some(0))).is_err());
        let mut x = config(Some(1)); x.graphics.as_mut().unwrap().initial_value=0x200;
        assert!(SystemControl::new(x).is_err());
    }
    #[test]
    fn control_retains_only_admitted_bits_and_phase_b_stays_absent() {
        let mut d=SystemControl::new(config(None)).unwrap(); let mut c=Clock::default();
        assert_eq!(read(&mut d,&mut c,CONTROL,4).unwrap(),7);
        write(&mut d,&mut c,CONTROL,4,0x7ff).unwrap();
        write(&mut d,&mut c,CONTROL,4,0x7f8).unwrap();
        assert_eq!(read(&mut d,&mut c,CONTROL,4).unwrap(),0x7f8);
        let before=d.save();
        for (a,n,v) in [(CONTROL,4,0x800),(CONTROL,1,0),(CONTROL+4,4,0),(VALUE,4,1),(REQUEST,4,2),(CONTROL,4,1u64<<32)] {
            assert!(write(&mut d,&mut c,a,n,v).is_err()); assert_eq!(d.save(),before);
        }
        assert!(read(&mut d,&mut c,CONTROL,8).is_err()); assert_eq!(d.windows(),vec![(CONTROL,CONTROL+4)]);
    }
    #[test]
    fn deadlines_are_time_driven_early_exact_late_and_poll_independent() {
        for ticks in [1,2,16,256,4096] {
            let mut d=SystemControl::new(config(Some(ticks))).unwrap(); let mut c=Clock::default();
            write(&mut d,&mut c,VALUE,4,0x102).unwrap(); write(&mut d,&mut c,REQUEST,4,2).unwrap();
            let before=d.save();
            for _ in 0..20 { assert_eq!(read(&mut d,&mut c,REQUEST,4).unwrap(),2); }
            assert_eq!(d.save(),before); assert_eq!(c.events,[(ticks,1)]);
            c.now=ticks-1; d.event(&mut Cx::new(&mut c,0,0,0),1); assert!(d.pending.is_some());
            c.now=ticks+u64::from(ticks==16); d.event(&mut Cx::new(&mut c,0,0,0),1);
            assert_eq!(read(&mut d,&mut c,REQUEST,4).unwrap(),0); assert_eq!(d.applied,0x102);
        }
    }
    #[test]
    fn pending_reissue_preserves_deadline_and_configuration_is_frozen() {
        let mut d=SystemControl::new(config(Some(8))).unwrap(); let mut c=Clock::default();
        assert!(write(&mut d,&mut c,REQUEST,4,2).is_err());
        write(&mut d,&mut c,VALUE,4,0x102).unwrap(); write(&mut d,&mut c,REQUEST,4,2).unwrap();
        let before=d.save(); c.now=3; write(&mut d,&mut c,REQUEST,4,2).unwrap(); assert_eq!(d.save(),before);
        for (a,v) in [(VALUE,0x102),(VALUE,0),(REQUEST,0),(REQUEST,3)] {
            assert!(write(&mut d,&mut c,a,4,v).is_err()); assert_eq!(d.save(),before);
        }
        assert_eq!(c.events,[(8,1)]);
    }
    #[test]
    fn unknown_phase_b_accesses_and_oversized_values_refuse() {
        let mut d=SystemControl::new(config(Some(2))).unwrap(); let mut c=Clock::default(); let before=d.save();
        for (a,n,v) in [(VALUE,4,0x200),(VALUE,2,0),(REQUEST,8,2),(REQUEST+4,4,0),(0xff701200,4,0)] {
            assert!(write(&mut d,&mut c,a,n,v).is_err()); assert_eq!(d.save(),before);
            if n!=4 || a!=VALUE { assert!(read(&mut d,&mut c,a,n).is_err()); }
        }
    }
    #[test]
    fn restore_roundtrip_late_delivery_and_stale_tokens() {
        let cfg=config(Some(4)); let mut d=SystemControl::new(cfg.clone()).unwrap(); let mut c=Clock::default();
        write(&mut d,&mut c,VALUE,4,0x102).unwrap(); write(&mut d,&mut c,REQUEST,4,2).unwrap();
        let mut resumed=SystemControl::new(cfg).unwrap(); resumed.restore(&d.save()).unwrap();
        c.now=9;
        for x in [&mut d,&mut resumed] { x.event(&mut Cx::new(&mut c,0,0,0),0); assert!(x.pending.is_some()); x.event(&mut Cx::new(&mut c,0,0,0),1); }
        assert_eq!(d.save(),resumed.save());
        resumed.restore(&d.save()).unwrap();
        write(&mut d,&mut c,REQUEST,4,2).unwrap(); let before=d.save();
        d.event(&mut Cx::new(&mut c,0,0,0),1); assert_eq!(d.save(),before);
    }
    #[test]
    fn restore_rejects_changed_inputs_and_invalid_masks_or_pending_state() {
        let mut d=SystemControl::new(config(Some(4))).unwrap(); let mut c=Clock::default();
        write(&mut d,&mut c,VALUE,4,1).unwrap(); write(&mut d,&mut c,REQUEST,4,2).unwrap(); let good=d.save();
        for (field,v) in [("control",json!(0x800)),("staged",json!(0x200)),("applied",json!(0x200)),("value_written",json!(false))] {
            let mut bad=good.clone(); bad[field]=v; assert!(d.restore(&bad).is_err()); assert_eq!(d.save(),good);
        }
        let mut bad=good.clone(); bad["pending"]["token"]=json!(0); assert!(d.restore(&bad).is_err());
        let mut bad=good.clone(); bad["pending"]["at"]=json!(0); assert!(d.restore(&bad).is_err());
        let mut fresh=SystemControl::new(config(Some(4))).unwrap();
        let mut bad=fresh.save(); bad["staged"]=json!(1); assert!(fresh.restore(&bad).is_err());
        assert!(SystemControl::new(config(Some(5))).unwrap().restore(&good).is_err());
        assert!(SystemControl::new(config(None)).unwrap().restore(&good).is_err());
    }
    #[test]
    fn deadline_and_generation_overflow_leave_state_unchanged() {
        let mut d=SystemControl::new(config(Some(2))).unwrap(); let mut c=Clock::default();
        write(&mut d,&mut c,VALUE,4,1).unwrap(); c.now=u64::MAX; let before=d.save();
        assert!(write(&mut d,&mut c,REQUEST,4,2).is_err()); assert_eq!(d.save(),before);
        c.now=0; d.generation=u64::MAX; let before=d.save();
        assert!(write(&mut d,&mut c,REQUEST,4,2).is_err()); assert_eq!(d.save(),before);
    }
    #[test]
    fn same_register_time_trace_is_independent_of_cpu_and_pc() {
        let mut outputs=Vec::new();
        for (cpu,pc) in [(0,0),(3,0xdead0000)] {
            let mut d=SystemControl::new(config(Some(2))).unwrap(); let mut c=Clock::default();
            d.write(&mut Cx::new(&mut c,cpu,pc,0),VALUE,4,0x102);
            d.write(&mut Cx::new(&mut c,cpu,pc,0),REQUEST,4,2);
            c.now=2; d.event(&mut Cx::new(&mut c,cpu,pc,0),1);
            outputs.push((d.save(),json!(c.records),c.events));
        }
        assert_eq!(outputs[0],outputs[1]);
    }
    fn banks_config(pattern: u32) -> Config {
        let value = |mask| match pattern { 0 => 0, 1 => mask, _ => mask & 0x55555555 };
        let a_reset = std::array::from_fn(|i| value(A_WORDS[i].1));
        let b_reset = std::array::from_fn(|i| value(B_WORDS[i].1));
        let mut cfg = config(None);
        cfg.control_f03c_reset = a_reset[15];
        cfg.retained_request_banks = Some(RequestBanks { a_reset, b_reset });
        cfg
    }
    #[test]
    fn every_request_word_obeys_explicit_reset_and_retention_for_three_profiles() {
        for pattern in 0..3 {
            let cfg = banks_config(pattern);
            let resets = cfg.retained_request_banks.as_ref().unwrap();
            let expected: Vec<_> = resets.a_reset.iter().chain(resets.b_reset.iter()).copied().collect();
            let mut d = SystemControl::new(cfg.clone()).unwrap();
            let mut c = Clock::default();
            let mut windows = d.windows(); windows.sort_unstable();
            assert_eq!(windows.len(), 31);
            assert!(windows.windows(2).all(|pair| pair[0].1 <= pair[1].0));
            for ((address, mask), reset) in A_WORDS.into_iter().chain(B_WORDS).zip(expected) {
                assert_eq!(read(&mut d, &mut c, address, 4).unwrap(), u64::from(reset));
                write(&mut d, &mut c, address, 4, u64::from(mask)).unwrap();
                assert_eq!(read(&mut d, &mut c, address, 4).unwrap(), u64::from(mask));
                write(&mut d, &mut c, address, 4, 0).unwrap();
                assert_eq!(read(&mut d, &mut c, address, 4).unwrap(), 0);
            }
            let saved = d.save();
            assert_eq!(saved["retained_banks"]["a"].as_array().unwrap().len(), 15);
            assert_eq!(saved["control"], 0); // A15 has exactly one mutable owner.
            let mut resumed = SystemControl::new(cfg).unwrap();
            resumed.restore(&saved).unwrap(); assert_eq!(resumed.save(), saved);
            assert!(c.events.is_empty());
        }
    }
    #[test]
    fn every_request_word_rejects_wrong_mask_width_and_unknown_addresses() {
        let mut d = SystemControl::new(banks_config(0)).unwrap();
        let mut c = Clock::default();
        read(&mut d, &mut c, CONTROL, 4).unwrap();
        for (address, mask) in A_WORDS.into_iter().chain(B_WORDS) {
            let before = d.save();
            let outside = 1u64 << (!mask).trailing_zeros();
            assert!(write(&mut d, &mut c, address, 4, outside).is_err());
            for size in [1, 2, 8] {
                assert!(read(&mut d, &mut c, address, size).is_err());
                assert!(write(&mut d, &mut c, address, size, 0).is_err());
            }
            assert!(read(&mut d, &mut c, address + 1, 4).is_err());
            assert_eq!(d.save(), before);
        }
        for address in [0xff70effc, 0xff70f040, 0xfff48338, 0xfff48400, 0xfff48604, 0xfff4860c] {
            assert!(read(&mut d, &mut c, address, 4).is_err());
            assert!(write(&mut d, &mut c, address, 4, 0).is_err());
        }
    }
    #[test]
    fn request_inputs_reject_missing_lengths_conflicts_unknown_and_duplicate_fields() {
        let good = json!(banks_config(0));
        for field in ["a_reset", "b_reset"] {
            let mut bad = good.clone(); bad["retained_request_banks"].as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<Config>(bad).is_err());
            for length in [0, 14, 17] {
                let mut bad = good.clone(); bad["retained_request_banks"][field] = json!(vec![0; length]);
                assert!(serde_json::from_value::<Config>(bad).is_err());
            }
        }
        let mut bad = good.clone(); bad["retained_request_banks"]["registers"] = json!([]);
        assert!(serde_json::from_value::<Config>(bad).is_err());
        let duplicate = r#"{"profile":"experimental-source-functional-v1","control_f03c_reset":0,"retained_request_banks":null,"retained_request_banks":null}"#;
        assert!(serde_json::from_str::<Config>(duplicate).is_err());
        let mut cfg = banks_config(0); cfg.control_f03c_reset = 1;
        assert!(SystemControl::new(cfg).is_err());
        for (is_a, words) in [(true, A_WORDS.as_slice()), (false, B_WORDS.as_slice())] {
            for (index, &(_, mask)) in words.iter().enumerate() {
                let mut cfg = banks_config(0);
                let banks = cfg.retained_request_banks.as_mut().unwrap();
                let outside = 1u32 << (!mask).trailing_zeros();
                if is_a { banks.a_reset[index] = outside; } else { banks.b_reset[index] = outside; }
                assert!(SystemControl::new(cfg).is_err());
            }
        }
    }
    #[test]
    fn request_restore_rejects_masks_presence_and_reset_profile_changes() {
        let mut d = SystemControl::new(banks_config(0)).unwrap();
        let good = d.save();
        for (field, words) in [("a", A_WORDS[..15].as_ref()), ("b", B_WORDS.as_slice())] {
            for (index, &(_, mask)) in words.iter().enumerate() {
                let mut bad = good.clone();
                bad["retained_banks"][field][index] = json!(1u32 << (!mask).trailing_zeros());
                assert!(d.restore(&bad).is_err()); assert_eq!(d.save(), good);
            }
        }
        let mut bad = good.clone(); bad["retained_banks"] = Value::Null;
        assert!(d.restore(&bad).is_err());
        assert!(SystemControl::new(banks_config(1)).unwrap().restore(&good).is_err());
        assert!(SystemControl::new(config(None)).unwrap().restore(&good).is_err());
    }
    #[test]
    fn request_trace_is_cpu_pc_independent_and_graphics_semantics_coexist() {
        let mut results = Vec::new();
        for (cpu, pc) in [(0, 0), (3, 0xdead0000)] {
            let mut cfg = banks_config(2);
            cfg.graphics = Some(Graphics { initial_value: 0, completion_ticks: 2 });
            let mut d = SystemControl::new(cfg).unwrap(); let mut c = Clock::default();
            assert_eq!(d.windows().len(), 33);
            for (address, mask) in A_WORDS.into_iter().chain(B_WORDS) {
                let mut cx = Cx::new(&mut c, cpu, pc, 0);
                d.write(&mut cx, address, 4, u64::from(mask)); assert!(cx.take_refusal().is_none());
            }
            write(&mut d, &mut c, VALUE, 4, 0x102).unwrap();
            write(&mut d, &mut c, REQUEST, 4, 2).unwrap();
            c.now = 2; d.event(&mut Cx::new(&mut c, cpu, pc, 0), 1);
            assert_eq!(read(&mut d, &mut c, REQUEST, 4).unwrap(), 0);
            results.push((d.save(), json!(c.records), c.events));
        }
        assert_eq!(results[0], results[1]);
        let mut old = SystemControl::new(config(None)).unwrap(); let mut c = Clock::default();
        for (address, _) in A_WORDS[..15].iter().chain(B_WORDS.iter()) {
            assert!(read(&mut old, &mut c, *address, 4).is_err());
        }
    }

    #[test]
    fn nested_configuration_deadline_event_and_restore_use_the_original_device_context() {
        let mut cfg = config(None);
        cfg.configuration_requests = Some(crate::configuration_request::Inputs {
            initial_values: vec![0; 52], completion_ticks: vec![8; 20], base_parameters: None,
        });
        let mut d = SystemControl::new(cfg.clone()).unwrap(); let mut c = Clock::default();
        write(&mut d, &mut c, 0xff701a40, 4, 0x100).unwrap();
        write(&mut d, &mut c, 0xff701a80, 4, 2).unwrap();
        assert_eq!(d.next_deadline(0), Some(8)); assert_eq!(c.events, [(8, 1)]);
        let good = d.save(); let mut resumed = SystemControl::new(cfg).unwrap();
        resumed.restore(&good).unwrap();
        let mut bad = good.clone(); bad["configuration_state"] = Value::Null;
        assert!(resumed.restore(&bad).is_err());
        let mut bad = good.clone(); bad["configuration_state"]["inputs"]["completion_ticks"][5] = json!(9);
        assert!(resumed.restore(&bad).is_err());
        let mut bad = good.clone(); bad["configuration_state"]["pending"][5]["values"][0] = json!(0x200);
        assert!(resumed.restore(&bad).is_err()); assert_eq!(resumed.save(), good);
        c.now = 7; resumed.event(&mut Cx::new(&mut c, 3, 0xdead0000, 0), 1);
        assert_eq!(read(&mut resumed, &mut c, 0xff701a80, 4).unwrap(), 2);
        c.now = 8; resumed.event(&mut Cx::new(&mut c, 3, 0xdead0000, 0), 1);
        assert_eq!(read(&mut resumed, &mut c, 0xff701a80, 4).unwrap(), 0);
        assert_eq!(resumed.next_deadline(8), None);
        assert_eq!(resumed.save()["configuration_state"]["applied"][19], 0x100);
        assert_eq!(resumed.control, 7); assert_eq!(resumed.generation, 0);
    }

    #[test]
    fn selection_state_routes_three_words_without_owning_or_coupling_b14() {
        let mut cfg = banks_config(0);
        cfg.configuration_requests = Some(crate::configuration_request::Inputs {
            initial_values: vec![0; 52], completion_ticks: vec![8; 20], base_parameters: None,
        });
        cfg.selection_special = Some(crate::selection_special::Inputs {
            selection_reset: 0, special52_reset: 2, special53_reset: 0,
        });
        let mut d = SystemControl::new(cfg.clone()).unwrap(); let mut c = Clock::default();
        assert_eq!(d.windows().len(), 106);
        write(&mut d, &mut c, 0xfff48600, 4, 0x50222).unwrap();
        write(&mut d, &mut c, 0xfff4860c, 4, 0x102).unwrap();
        write(&mut d, &mut c, 0xfff48604, 4, 0x110).unwrap();
        let selection = d.save()["selection_state"].clone();
        write(&mut d, &mut c, 0xfff48608, 4, 7).unwrap();
        assert_eq!(d.save()["selection_state"], selection);
        assert_eq!(read(&mut d, &mut c, 0xfff48608, 4).unwrap(), 7);
        assert_eq!(d.next_deadline(0), None); assert!(c.events.is_empty());
        let saved = d.save(); let mut resumed = SystemControl::new(cfg).unwrap(); resumed.restore(&saved).unwrap();
        let mut bad = saved.clone(); bad["selection_state"]["selection"] = json!(0xc00);
        assert!(resumed.restore(&bad).is_err());
        let mut bad = saved.clone(); bad["selection_state"] = Value::Null;
        assert!(resumed.restore(&bad).is_err()); assert_eq!(resumed.save(), saved);
        let mut absent = SystemControl::new(config(None)).unwrap();
        assert!(read(&mut absent, &mut c, 0xfff48600, 4).is_err());
    }

}
