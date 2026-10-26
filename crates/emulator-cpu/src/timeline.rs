//! Deterministic guest time: instruction-derived fractional ticks and stable
//! device deadlines. No host clock, sleep, sampling or wall-time conversion.
use serde_json::{Value, json};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Instructions per board tick. The representation is retained because a
/// checkpoint's fractional phase uses units of 1/numerator ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ratio {
    numerator: u64,
    denominator: u64,
}
impl<'de> serde::Deserialize<'de> for Ratio {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (numerator, denominator) =
            <(u64, u64) as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(numerator, denominator).map_err(serde::de::Error::custom)
    }
}
impl Ratio {
    pub fn new(numerator: u64, denominator: u64) -> Result<Self, String> {
        if numerator == 0 || denominator == 0 || numerator > 1 << 32 || denominator > 1 << 32 {
            return Err("instructions-per-tick ratio must have positive bounded components".into());
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }
    pub fn parts(self) -> (u64, u64) {
        (self.numerator, self.denominator)
    }
    /// Whole ticks and retained phase, using a wide intermediate so a bulk
    /// block and individually observed instructions produce the same result.
    pub fn advance_phase(self, phase: u64, entries: u64) -> Result<(u128, u64), String> {
        if phase >= self.numerator {
            return Err("clock phase exceeds its ratio".into());
        }
        if entries == 1 && self.denominator <= self.numerator {
            let phase = phase + self.denominator;
            return Ok(if phase >= self.numerator {
                (1, phase - self.numerator)
            } else {
                (0, phase)
            });
        }
        let fraction = u128::from(phase) + u128::from(entries) * u128::from(self.denominator);
        Ok((
            fraction / u128::from(self.numerator),
            (fraction % u128::from(self.numerator)) as u64,
        ))
    }
    /// Entries until the first tick at/after a deadline. With a ratio below
    /// one, an entry can cross several ticks; the scheduler must service those
    /// deadlines rather than attributing the whole span to one late callback.
    pub fn budget(self, now: u64, phase: u64, deadline: u64) -> Result<u64, String> {
        if phase >= self.numerator {
            return Err("clock phase exceeds its ratio".into());
        }
        if now >= deadline {
            return Ok(0);
        }
        let need = u128::from(deadline - now) * u128::from(self.numerator) - u128::from(phase);
        Ok(need
            .div_ceil(u128::from(self.denominator))
            .min(u128::from(u64::MAX)) as u64)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    DeviceEvent(usize),
    DeviceDeadline(usize),
    ProcessorDeadline(usize),
}
impl Target {
    fn fields(self) -> (&'static str, usize) {
        match self {
            Self::DeviceEvent(id) => ("device-event", id),
            Self::DeviceDeadline(id) => ("device-deadline", id),
            Self::ProcessorDeadline(id) => ("processor-deadline", id),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Event {
    pub at: u64,
    pub sequence: u64,
    pub target: Target,
    pub token: u64,
}
/// The framework owns this queue and calls a device at the timestamp it
/// removed. Model event tokens handle cancellation; queue order is stable.
#[derive(Clone)]
pub struct Timeline {
    now: u64,
    phase: u64,
    ratio: Ratio,
    sequence: u64,
    queue: BinaryHeap<Reverse<Event>>,
}
impl Timeline {
    pub fn new(ratio: Ratio) -> Result<Self, String> {
        if ratio.numerator < ratio.denominator {
            return Err("timeline requires at least one instruction per board tick".into());
        }
        Ok(Self {
            now: 0,
            phase: 0,
            ratio,
            sequence: 0,
            queue: BinaryHeap::new(),
        })
    }
    pub fn now(&self) -> u64 {
        self.now
    }
    pub fn phase(&self) -> u64 {
        self.phase
    }
    pub fn ratio(&self) -> Ratio {
        self.ratio
    }
    pub fn validate_targets(&self, devices: usize, processors: usize) -> Result<(), String> {
        for event in &self.queue {
            let valid = match event.0.target {
                Target::DeviceEvent(id) | Target::DeviceDeadline(id) => id < devices,
                Target::ProcessorDeadline(id) => id < processors,
            };
            if !valid {
                return Err("timeline snapshot targets an absent device/processor".into());
            }
        }
        Ok(())
    }
    pub fn next_event(&self) -> Option<Event> {
        self.queue.peek().map(|e| e.0)
    }
    /// Replaceable native deadlines use a distinct queue target from explicit
    /// model tokens, so replacing one never cancels a model's other events.
    pub fn cancel_target(&mut self, target: Target) {
        self.queue.retain(|e| e.0.target != target);
    }
    pub fn schedule(&mut self, target: Target, at: u64, token: u64) -> Result<(), String> {
        if at < self.now {
            return Err("device deadline precedes guest time".into());
        }
        let next = self
            .sequence
            .checked_add(1)
            .ok_or("event sequence exhausted")?;
        self.queue.push(Reverse(Event {
            at,
            sequence: self.sequence,
            target,
            token,
        }));
        self.sequence = next;
        Ok(())
    }
    pub fn budget(&self, limit: u64) -> Result<u64, String> {
        let at = self.next_event().map_or(u64::MAX, |e| e.at);
        Ok(limit.min(self.ratio.budget(self.now, self.phase, at)?))
    }
    /// Account an exactly measured entry count. A run may not step across an
    /// unserviced deadline. A canceled run with unknown progress cannot call it.
    pub fn advance_instructions(&mut self, entries: u64) -> Result<(), String> {
        let (ticks, phase) = self.ratio.advance_phase(self.phase, entries)?;
        let ticks = u64::try_from(ticks).map_err(|_| "guest tick span overflow")?;
        let now = self
            .now
            .checked_add(ticks)
            .ok_or("guest timeline exhausted")?;
        if self.next_event().is_some_and(|e| e.at < now) {
            return Err("CPU entries crossed an unserviced device deadline".into());
        }
        self.now = now;
        self.phase = phase;
        Ok(())
    }
    /// Only the scheduler, after proving every released CPU is in an
    /// architectural wait, may skip unexecuted ticks. Busy loops never qualify.
    pub fn advance_waiting(
        &mut self,
        all_waiting: bool,
        until: Option<u64>,
    ) -> Result<bool, String> {
        if !all_waiting {
            return Err("guest idle advance requires all released CPUs waiting".into());
        }
        let at = match (self.next_event(), until) {
            (Some(e), Some(cap)) => e.at.min(cap),
            (Some(e), None) => e.at,
            (None, Some(cap)) => cap,
            (None, None) => return Ok(false),
        };
        if at < self.now {
            return Err("idle boundary precedes guest time".into());
        }
        self.now = at;
        Ok(true)
    }
    pub fn pop_due(&mut self) -> Option<Event> {
        if self.queue.peek().is_some_and(|e| e.0.at <= self.now) {
            self.queue.pop().map(|e| e.0)
        } else {
            None
        }
    }
    /// Full queue, including stable insertion order; inspection has no effects.
    pub fn save(&self) -> Value {
        let mut events: Vec<_> = self.queue.iter().map(|e| e.0).collect();
        events.sort();
        json!({"version":1,"now":self.now,"phase":self.phase,"ratio":self.ratio.parts(),"sequence":self.sequence,
            "events":events.into_iter().map(|e|{let (kind,id)=e.target.fields();json!([e.at,e.sequence,kind,id,e.token])}).collect::<Vec<_>>()})
    }
    pub fn restore(state: &Value) -> Result<Self, String> {
        let integer = |key| {
            state[key]
                .as_u64()
                .ok_or_else(|| format!("invalid timeline {key}"))
        };
        if integer("version")? != 1 {
            return Err("unsupported timeline checkpoint version".into());
        }
        let now = integer("now")?;
        let phase = integer("phase")?;
        let sequence = integer("sequence")?;
        let ratio = state["ratio"]
            .as_array()
            .filter(|r| r.len() == 2)
            .ok_or("invalid timeline ratio")?;
        let ratio = Ratio::new(
            ratio[0].as_u64().ok_or("invalid ratio numerator")?,
            ratio[1].as_u64().ok_or("invalid ratio denominator")?,
        )?;
        Self::new(ratio)?;
        ratio.advance_phase(phase, 0)?;
        let mut queue = BinaryHeap::new();
        let mut seen = std::collections::BTreeSet::new();
        for row in state["events"]
            .as_array()
            .ok_or("invalid timeline events")?
        {
            let row = row
                .as_array()
                .filter(|r| r.len() == 5)
                .ok_or("invalid timeline event")?;
            let at = row[0].as_u64().ok_or("invalid event time")?;
            let sequence_value = row[1].as_u64().ok_or("invalid event sequence")?;
            let id = usize::try_from(row[3].as_u64().ok_or("invalid event target")?)
                .map_err(|_| "event target exceeds host width")?;
            let target = match row[2].as_str() {
                Some("device-event") => Target::DeviceEvent(id),
                Some("device-deadline") => Target::DeviceDeadline(id),
                Some("processor-deadline") => Target::ProcessorDeadline(id),
                _ => return Err("invalid event target kind".into()),
            };
            let token = row[4].as_u64().ok_or("invalid event token")?;
            if at < now || sequence_value >= sequence || !seen.insert(sequence_value) {
                return Err("invalid timeline event ordering".into());
            }
            queue.push(Reverse(Event {
                at,
                sequence: sequence_value,
                target,
                token,
            }));
        }
        Ok(Self {
            now,
            phase,
            ratio,
            sequence,
            queue,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacing_a_model_deadline_preserves_explicit_and_processor_events() {
        let mut t = Timeline::new(Ratio::new(1, 1).unwrap()).unwrap();
        t.schedule(Target::DeviceEvent(0), 7, 1).unwrap();
        t.schedule(Target::DeviceDeadline(0), 7, 2).unwrap();
        t.schedule(Target::ProcessorDeadline(0), 7, 3).unwrap();
        let mut t = Timeline::restore(&t.save()).unwrap();
        t.cancel_target(Target::DeviceDeadline(0));
        t.advance_waiting(true, None).unwrap();
        assert_eq!(t.pop_due().unwrap().target, Target::DeviceEvent(0));
        assert_eq!(t.pop_due().unwrap().target, Target::ProcessorDeadline(0));
        assert!(t.pop_due().is_none());
    }
    #[test]
    fn fractional_clock_is_invariant_under_batching_and_checkpoint_splits() {
        for (n, d) in [(1, 1), (24, 1), (37, 11), (1 << 32, (1 << 32) - 1)] {
            let rate = Ratio::new(n, d).unwrap();
            let mut bulk = Timeline::new(rate).unwrap();
            let mut single = Timeline::new(rate).unwrap();
            bulk.advance_instructions(1057).unwrap();
            for _ in 0..1057 {
                single.advance_instructions(1).unwrap();
            }
            assert_eq!(bulk.save(), single.save());
            let mut split = Timeline::new(rate).unwrap();
            split.advance_instructions(213).unwrap();
            let mut restored = Timeline::restore(&split.save()).unwrap();
            restored.advance_instructions(844).unwrap();
            assert_eq!(restored.save(), bulk.save());
        }
    }
    #[test]
    fn equal_tick_deadlines_keep_insertion_order_across_restore() {
        let mut t = Timeline::new(Ratio::new(7, 2).unwrap()).unwrap();
        t.schedule(Target::DeviceEvent(9), 5, 3).unwrap();
        t.schedule(Target::DeviceEvent(2), 5, 9).unwrap();
        t.schedule(Target::DeviceEvent(8), 4, 1).unwrap();
        let before = t.save();
        assert_eq!(t.save(), before);
        assert_eq!(t.next_event().unwrap().at, 4);
        assert!(t.advance_waiting(false, None).is_err());
        assert_eq!(t.save(), before);
        assert!(t.advance_waiting(true, None).unwrap());
        assert_eq!(t.now(), 4);
        assert_eq!(t.pop_due().unwrap().target, Target::DeviceEvent(8));
        assert!(t.pop_due().is_none());
        let mut restored = Timeline::restore(&t.save()).unwrap();
        restored.advance_waiting(true, None).unwrap();
        assert_eq!(restored.pop_due().unwrap().target, Target::DeviceEvent(9));
        assert_eq!(restored.pop_due().unwrap().target, Target::DeviceEvent(2));
        assert_eq!(restored.pop_due(), None);
        assert!(!restored.advance_waiting(true, None).unwrap());
    }
    #[test]
    fn budgets_preserve_phase_and_do_not_hide_late_events() {
        assert!(Timeline::new(Ratio::new(1, 2).unwrap()).is_err());
        let mut t = Timeline::new(Ratio::new(7, 2).unwrap()).unwrap();
        t.advance_instructions(2).unwrap();
        assert_eq!((t.now(), t.phase()), (0, 4));
        t.schedule(Target::DeviceEvent(0), 3, 1).unwrap();
        let budget = t.budget(100).unwrap();
        assert_eq!(budget, 9);
        t.advance_instructions(budget).unwrap();
        assert_eq!(t.now(), 3);
        assert_eq!(t.budget(100).unwrap(), 0);
        assert!(t.pop_due().is_some());
        assert!(t.schedule(Target::DeviceEvent(0), 2, 0).is_err());
        t.schedule(Target::DeviceEvent(0), 4, 0).unwrap();
        let before = t.save();
        assert!(t.advance_instructions(100).is_err());
        assert_eq!(t.save(), before);
        let mut overflow = before;
        overflow["now"] = Value::from(u64::MAX);
        overflow["events"] = json!([]);
        let mut overflow = Timeline::restore(&overflow).unwrap();
        let before = overflow.save();
        assert!(overflow.advance_instructions(100).is_err());
        assert_eq!(overflow.save(), before);
    }
}
