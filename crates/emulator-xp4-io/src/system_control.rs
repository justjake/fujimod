//! Opt-in functional hypotheses for three source-bound XP4 control words.
//! Retention, reset values and completion latency are declared experimental
//! inputs, not measured clock/reset laws. No IRQ, DMA or CPU state is used.
use fujimod_emulator_devices::{Cx, Device, Record, Value as Field};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const CONTROL: u64 = 0xff70f03c;
pub const VALUE: u64 = 0xff701240;
pub const REQUEST: u64 = 0xff701280;
pub const PROFILE: &str = "experimental-source-functional-v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Graphics {
    pub initial_value: u32,
    pub completion_ticks: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub profile: String,
    pub control_f03c_reset: u32,
    #[serde(default)]
    pub graphics: Option<Graphics>,
}
impl Config {
    fn validate(&self) -> Result<(), String> {
        if self.profile != PROFILE || self.control_f03c_reset & !0x7ff != 0
            || self.graphics.as_ref().is_some_and(|g| g.initial_value & !0x1ff != 0 || g.completion_ticks == 0)
        {
            return Err("Invalid experimental XP4 functional system-control input".into());
        }
        Ok(())
    }
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
        Ok(Self { control: config.control_f03c_reset, config, staged: initial,
            applied: initial, value_written: false, pending: None, generation: 0, admitted: false })
    }
    pub fn windows(&self) -> Vec<(u64, u64)> {
        let mut spans = vec![(CONTROL, CONTROL + 4)];
        if self.config.graphics.is_some() { spans.extend([(VALUE, VALUE + 4), (REQUEST, REQUEST + 4)]); }
        spans
    }
    fn known(&self, address: u64, size: u8) -> bool {
        size == 4 && (address == CONTROL || self.config.graphics.is_some() && matches!(address, VALUE | REQUEST))
    }
    fn admit(&mut self, cx: &mut Cx<'_>) {
        if !self.admitted {
            cx.record(Record::new("XP4 experimental functional system-control assumptions", [
                ("profile", Field::constant(PROFILE)),
                ("reset_control", Field::int(self.config.control_f03c_reset)),
                ("graphics_enabled", Field::int(u32::from(self.config.graphics.is_some()))),
                ("initial_graphics_value", Field::int(self.config.graphics.as_ref().map_or(0, |g| g.initial_value))),
                ("completion_ticks_unmeasured", Field::int(self.config.graphics.as_ref().map_or(0, |g| g.completion_ticks))),
                ("law", Field::constant("Retained control; optional delayed command-2 acknowledgment; no physical clock/reset claim")),
            ]));
            self.admitted = true;
        }
    }
}
impl Device for SystemControl {
    fn name(&self) -> &'static str { "xp4-functional-system-control" }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if !self.known(address, size) { cx.unmodeled(); return 0; }
        self.admit(cx);
        u64::from(match address { CONTROL => self.control, VALUE => self.staged, _ => if self.pending.is_some() { 2 } else { 0 } })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if !self.known(address, size) { cx.unmodeled(); return; }
        if value > u32::MAX as u64 { cx.fail("XP4 functional control requires a 32-bit value"); return; }
        let value = value as u32;
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
    fn next_deadline(&self, _: u64) -> Option<u64> { self.pending.as_ref().map(|p| p.at) }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
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
        let no_graphics = state.config.graphics.is_none();
        let initial = state.config.graphics.as_ref().map_or(0, |g| g.initial_value);
        if (!state.value_written && (state.staged != initial || state.applied != initial || state.generation != 0))
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
        Config { profile: PROFILE.into(), control_f03c_reset: 7,
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
}
