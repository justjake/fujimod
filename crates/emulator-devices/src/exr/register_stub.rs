//! Declared register stand-in for a block whose behaviour is not modeled yet.
//!
//! Writes are retained per 32-bit word, as the bus retains unowned MMIO. Reads apply declared rules:
//! `read = (retained & !clear) | set` for the word at `offset + n * stride` (n < count). A rule therefore
//! states a hardware response the firmware waits for (a self-clearing start bit, an idle state field),
//! visibly in the board definition, instead of patching firmware. It is not a measured device model.
use crate::bus::InterruptLine;
use crate::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Byte offset of the first word from `base`.
    pub offset: u32,
    /// Bits forced to 1 on read.
    #[serde(default)]
    pub set: u32,
    /// Bits forced to 0 on read.
    #[serde(default)]
    pub clear: u32,
    /// Distance between repeated words (0 for a single word).
    #[serde(default)]
    pub stride: u32,
    /// Number of repeated words.
    #[serde(default = "one")]
    pub count: u32,
}

fn one() -> u32 {
    1
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub base: u64,
    pub size: u64,
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Periodic interrupts standing in for unmodeled event sources (for example a sensor's frame sync).
    #[serde(default)]
    pub irqs: Vec<PeriodicIrq>,
    /// Interrupts raised a fixed delay after a write sets declared bits (for example a job start and its completion).
    #[serde(default)]
    pub triggers: Vec<WriteTrigger>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteTrigger {
    pub offset: u32,
    pub mask: u32,
    pub line: InterruptLine,
    pub delay_ticks: u64,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeriodicIrq {
    pub line: InterruptLine,
    /// Ticks between interrupts.
    pub period_ticks: u64,
    /// Offset of each interrupt within its period, so several events of one period arrive in order.
    #[serde(default)]
    pub phase_ticks: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    words: BTreeMap<u32, u32>,
    #[serde(default)]
    next_irq: Vec<Option<u64>>,
    #[serde(default)]
    irqs_raised: u64,
    /// Pending write-triggered interrupts: (due tick, trigger index).
    #[serde(default)]
    triggered: Vec<(u64, usize)>,
}

pub struct RegisterStub {
    options: Options,
    state: State,
}

impl RegisterStub {
    pub fn new(options: Options) -> Result<Self, String> {
        if options.size == 0 || options.size & 3 != 0 || options.base & 3 != 0 {
            return Err("register stub requires a word-aligned, non-empty window".into());
        }
        for rule in &options.rules {
            let last = u64::from(rule.offset) + u64::from(rule.count.max(1) - 1) * u64::from(rule.stride);
            if rule.offset & 3 != 0 || rule.stride & 3 != 0 || last + 4 > options.size || rule.count == 0 {
                return Err("register stub rule lies outside its window".into());
            }
        }
        if options.irqs.iter().any(|i| i.period_ticks == 0 || i.phase_ticks >= i.period_ticks) {
            return Err("register stub periodic interrupts need a period above their phase".into());
        }
        let next_irq = vec![None; options.irqs.len()];
        Ok(Self { options, state: State { next_irq, ..State::default() } })
    }

    fn word_offset(&self, address: u64) -> Option<u32> {
        let offset = address.checked_sub(self.options.base)?;
        (offset < self.options.size).then_some((offset & !3) as u32)
    }

    fn apply(&self, offset: u32, value: u32) -> u32 {
        let mut value = value;
        for rule in &self.options.rules {
            let hit = if rule.stride == 0 {
                offset == rule.offset
            } else {
                offset >= rule.offset
                    && (offset - rule.offset).is_multiple_of(rule.stride)
                    && (offset - rule.offset) / rule.stride < rule.count
            };
            if hit {
                value = (value & !rule.clear) | rule.set;
            }
        }
        value
    }
}

impl Device for RegisterStub {
    crate::checkpoint_fields!(state: State);

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "register-stub"
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(offset) = self.word_offset(address) else {
            cx.unmodeled();
            return 0;
        };
        let word = self.apply(offset, self.state.words.get(&offset).copied().unwrap_or(0));
        let shift = (address & 3) * 8;
        let mask = if size >= 4 { u64::from(u32::MAX) } else { (1u64 << (u64::from(size) * 8)) - 1 };
        (u64::from(word) >> shift) & mask
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = self.word_offset(address) else {
            cx.unmodeled();
            return;
        };
        let shift = (address & 3) * 8;
        let mask = if size >= 4 { u64::from(u32::MAX) } else { ((1u64 << (u64::from(size) * 8)) - 1) << shift };
        let old = u64::from(self.state.words.get(&offset).copied().unwrap_or(0));
        let new = (old & !mask) | ((value << shift) & mask);
        self.state.words.insert(offset, new as u32);
        for (i, t) in self.options.triggers.iter().enumerate() {
            if t.offset == offset && (new as u32) & t.mask != 0 {
                self.state.triggered.push((cx.now() + t.delay_ticks, i));
            }
        }
    }

    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.options
            .irqs
            .iter()
            .enumerate()
            .map(|(i, irq)| self.state.next_irq.get(i).copied().flatten().unwrap_or(now + irq.phase_ticks))
            .chain(self.state.triggered.iter().map(|&(due, _)| due))
            .min()
    }

    fn advance(&mut self, cx: &mut Cx<'_>) {
        let now = cx.now();
        let (due, later): (Vec<_>, Vec<_>) = self.state.triggered.drain(..).partition(|&(at, _)| at <= now);
        self.state.triggered = later;
        for (_, i) in due {
            let result = match self.options.triggers[i].line {
                InterruptLine::Core { core, line } => cx.raise_irq(core, line),
                InterruptLine::Shared(line) => {
                    cx.set_irq(line, true);
                    Ok(())
                }
            };
            if let Err(error) = result {
                cx.fail(error);
                return;
            }
            self.state.irqs_raised += 1;
        }
        self.state.next_irq.resize(self.options.irqs.len(), None);
        for (i, irq) in self.options.irqs.clone().into_iter().enumerate() {
            let due = *self.state.next_irq[i].get_or_insert(now - now % irq.period_ticks + irq.period_ticks + irq.phase_ticks);
            if now < due {
                continue;
            }
            let result = match irq.line {
                InterruptLine::Core { core, line } => cx.raise_irq(core, line),
                InterruptLine::Shared(line) => {
                    cx.set_irq(line, true);
                    Ok(())
                }
            };
            if let Err(error) = result {
                cx.fail(error);
                return;
            }
            self.state.irqs_raised += 1;
            self.state.next_irq[i] = Some(due + ((now - due) / irq.period_ticks + 1) * irq.period_ticks);
        }
    }

    fn save(&self) -> Value {
        json!({"state": self.state})
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        self.state = serde_json::from_value(value["state"].clone()).map_err(|error| format!("invalid register stub state: {error}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stub(rules: Vec<Rule>) -> RegisterStub {
        RegisterStub::new(Options { base: 0x1000, size: 0x400, rules, irqs: vec![], triggers: vec![] }).unwrap()
    }

    #[test]
    fn rules_force_bits_on_read_and_repeat_by_stride() {
        let s = stub(vec![
            Rule { offset: 0x7c, set: 0, clear: 1, stride: 0, count: 1 },
            Rule { offset: 0xa0, set: 3, clear: 0, stride: 0x20, count: 5 },
        ]);
        assert_eq!(s.apply(0x7c, 0x31), 0x30);
        assert_eq!(s.apply(0xa0 + 4 * 0x20, 0x10000), 0x10003);
        assert_eq!(s.apply(0xa0 + 5 * 0x20, 0), 0);
        assert_eq!(s.apply(0xa4, 0), 0);
    }

    #[test]
    fn rules_outside_the_window_refuse() {
        let bad = Options { base: 0x1000, size: 0x100, rules: vec![Rule { offset: 0xa0, set: 3, clear: 0, stride: 0x20, count: 5 }], irqs: vec![], triggers: vec![] };
        assert!(RegisterStub::new(bad).is_err());
        let half = Options { base: 0x1000, size: 0x100, rules: vec![], irqs: vec![PeriodicIrq { line: InterruptLine::Core { core: 0, line: 35 }, period_ticks: 10, phase_ticks: 10 }], triggers: vec![] };
        assert!(RegisterStub::new(half).is_err());
    }
}
