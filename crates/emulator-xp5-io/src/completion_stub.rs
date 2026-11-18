//! Generic start/complete register stub for bring-up of unmodeled processing blocks.
//!
//! Many XP5 develop blocks follow one driver pattern: program a set of words,
//! write a start bit, wait for an interrupt (or poll a status word), then
//! acknowledge the status. A completion stub window answers that pattern
//! without modelling the block's work:
//!
//! * `registers` lists per-offset behaviours exactly as `bringup-stub` does
//!   ([`crate::bringup_stub::Mode`]); every other aligned 32-bit word stores its
//!   last write and reads it back.
//! * A write that matches one of `starts` schedules a completion `delay_us`
//!   later (one at a time: a start while one is due is ignored).
//! * When the completion fires, each `completion` entry sets and clears bits
//!   in a word, and the optional `irq` line rises unless the optional mask bit
//!   holds it low.
//! * A write to `ack` clears the status bits and lowers the line.
//!
//! This is a bring-up aid, not a model: the block's real start, status and
//! acknowledge words are whatever the manifest guesses them to be.
use crate::bringup_stub::{Mode, Register};
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// A write to `offset` starts the block when `written & mask` is non-zero, or,
/// with `value`, when `written & mask == value`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Start {
    pub offset: u64,
    pub mask: u32,
    #[serde(default)]
    pub value: Option<u32>,
}

/// When the completion fires, the word at `offset` becomes `(word | set) & !clear`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Completion {
    pub offset: u64,
    #[serde(default)]
    pub set: u32,
    #[serde(default)]
    pub clear: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AckMode {
    /// The written ones clear the matching status bits; the write is not stored
    /// when the acknowledge word is the status word.
    W1c,
    /// Any write clears all acknowledged status bits.
    WriteAny,
}

/// A write to `offset` clears `bits` of the word at `status_offset` (defaults:
/// the acknowledge word itself, and every bit the completion sets there, or all
/// bits when it sets none). The line falls once none of `bits` is left set.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Ack {
    pub offset: u64,
    pub mode: AckMode,
    #[serde(default)]
    pub status_offset: Option<u64>,
    #[serde(default)]
    pub bits: Option<u32>,
}

fn default_delay_us() -> u64 {
    100
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(default)]
    pub registers: Vec<Register>,
    /// GIC INTID raised when a completion fires. The output is a level; an
    /// edge-configured INTID latches on its rising edge.
    #[serde(default)]
    pub irq: Option<u32>,
    /// Drive the line high and straight back low at each unmasked completion
    /// instead of holding it until the acknowledge (for edge-configured INTIDs
    /// whose driver has no acknowledge word).
    #[serde(default)]
    pub irq_pulse: bool,
    /// Word and bit number whose state masks the interrupt.
    #[serde(default)]
    pub irq_mask_offset: Option<u64>,
    #[serde(default)]
    pub irq_mask_bit: Option<u32>,
    /// true (default): a set mask bit masks the line; false: a set bit enables it.
    #[serde(default = "yes")]
    pub irq_mask_set_masks: bool,
    #[serde(default)]
    pub starts: Vec<Start>,
    #[serde(default = "default_delay_us")]
    pub delay_us: u64,
    #[serde(default)]
    pub completion: Vec<Completion>,
    #[serde(default)]
    pub ack: Option<Ack>,
    /// true (default): an unlisted word never written reads 0; false: such a
    /// read stops the run as unmodeled.
    #[serde(default = "yes")]
    pub retain_unwritten_reads_zero: bool,
}

/// Saved device state. `words` holds every written word (a present zero is a
/// written zero), so it also records which words were written.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    phase: BTreeMap<u64, bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    due: Option<u64>,
    /// A completion fired and has not been acknowledged.
    pending: bool,
    /// The level this device drives on `irq`.
    line: bool,
    starts: u64,
    completions: u64,
}

pub struct CompletionStub {
    base: u64,
    size: u64,
    options: Options,
    delay_ticks: u64,
    ack_bits: u32,
    /// Diagnostic: the first start was logged. Not device state.
    logged: bool,
    state: State,
}

impl CompletionStub {
    /// `ticks_per_us` is the board's time unit per microsecond (1000 on XP5).
    pub fn new(base: u64, size: u64, options: Options, ticks_per_us: u64) -> Result<Self, String> {
        if size == 0 || size & 3 != 0 || base & 3 != 0 || base.checked_add(size).is_none() {
            return Err("invalid completion stub window".into());
        }
        let word = |offset: u64, what: &str| {
            if offset & 3 != 0 || offset >= size {
                Err(format!("completion stub {what} offset {offset:#x} is not a word of the window"))
            } else {
                Ok(())
            }
        };
        for register in &options.registers {
            word(register.offset, "register")?;
            if register.value > u64::from(u32::MAX) {
                return Err("completion stub register value exceeds 32 bits".into());
            }
        }
        for start in &options.starts {
            word(start.offset, "start")?;
            if start.mask == 0 || start.value.is_some_and(|v| v & !start.mask != 0) {
                return Err("completion stub start needs a non-zero mask covering its value".into());
            }
        }
        for completion in &options.completion {
            word(completion.offset, "completion")?;
        }
        if let Some(irq) = options.irq
            && !(32..1020).contains(&irq)
        {
            return Err(format!("completion stub irq {irq} is not a shared interrupt"));
        }
        match (options.irq_mask_offset, options.irq_mask_bit) {
            (None, None) => {}
            (Some(offset), Some(bit)) if bit < 32 => word(offset, "irq mask")?,
            _ => return Err("completion stub needs both irq_mask_offset and irq_mask_bit < 32".into()),
        }
        let mut ack_bits = 0;
        if let Some(ack) = &options.ack {
            word(ack.offset, "ack")?;
            let status = ack.status_offset.unwrap_or(ack.offset);
            word(status, "ack status")?;
            ack_bits = ack.bits.unwrap_or_else(|| {
                let set = options
                    .completion
                    .iter()
                    .filter(|c| c.offset == status)
                    .fold(0, |bits, c| bits | c.set);
                if set == 0 { u32::MAX } else { set }
            });
        }
        let delay_ticks = options
            .delay_us
            .checked_mul(ticks_per_us)
            .ok_or("completion stub delay overflows")?;
        Ok(Self {
            base,
            size,
            options,
            delay_ticks,
            ack_bits,
            logged: false,
            state: State::default(),
        })
    }

    fn word(&self, offset: u64) -> u32 {
        self.state.words.get(&offset).copied().unwrap_or(0)
    }

    fn offset(&self, address: u64, size: u8) -> Option<u64> {
        address
            .checked_sub(self.base)
            .filter(|offset| *offset < self.size && offset & 3 == 0 && size == 4)
    }

    fn masked(&self) -> bool {
        match (self.options.irq_mask_offset, self.options.irq_mask_bit) {
            (Some(offset), Some(bit)) => {
                (self.word(offset) >> bit & 1 != 0) == self.options.irq_mask_set_masks
            }
            _ => false,
        }
    }

    fn level(&self) -> bool {
        self.options.irq.is_some() && !self.options.irq_pulse && self.state.pending && !self.masked()
    }

    fn update_line(&mut self, cx: &mut Cx<'_>) {
        let level = self.level();
        if let Some(irq) = self.options.irq
            && level != self.state.line
        {
            self.state.line = level;
            cx.set_irq(irq, level);
        }
    }

    fn started(&self, offset: u64, value: u32) -> bool {
        self.options.starts.iter().any(|start| {
            start.offset == offset
                && match start.value {
                    Some(expected) => value & start.mask == expected,
                    None => value & start.mask != 0,
                }
        })
    }

    fn check(&self, state: &State) -> Result<(), String> {
        let inside = |offset: &u64| offset & 3 == 0 && *offset < self.size;
        if !state.words.keys().all(inside)
            || !state.phase.keys().all(|offset| {
                self.options
                    .registers
                    .iter()
                    .any(|r| r.offset == *offset && r.mode == Mode::Toggle)
            })
            || (state.line && self.options.irq.is_none())
        {
            return Err("invalid completion stub checkpoint".into());
        }
        Ok(())
    }
}

impl Device for CompletionStub {
    fn name(&self) -> &'static str {
        "completion-stub"
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        let written = self.state.words.contains_key(&offset);
        if let Some(register) = self.options.registers.iter().find(|r| r.offset == offset) {
            return if register.mode == Mode::Toggle {
                register.read(self.state.phase.entry(offset).or_insert(false), written)
            } else {
                register.read(&mut false, written)
            };
        }
        if !written && !self.options.retain_unwritten_reads_zero {
            cx.unmodeled();
            return 0;
        }
        u64::from(self.word(offset))
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let (Some(offset), Ok(value)) = (self.offset(address, size), u32::try_from(value)) else {
            cx.unmodeled();
            return;
        };
        let mut store = true;
        if let Some(ack) = self.options.ack.as_ref().filter(|ack| ack.offset == offset) {
            let status = ack.status_offset.unwrap_or(offset);
            let cleared = match ack.mode {
                AckMode::W1c => value & self.ack_bits,
                AckMode::WriteAny => self.ack_bits,
            };
            let word = self.word(status) & !cleared;
            self.state.words.insert(status, word);
            self.state.pending = word & self.ack_bits != 0;
            store = status != offset;
        }
        if store {
            self.state.words.insert(offset, value);
        }
        if self.started(offset, value) {
            if !self.logged {
                self.logged = true;
                cx.log(&format!(
                    "completion-stub {:#x}: first start +{offset:#x} = {value:#x} (pc {:#x})",
                    self.base, cx.pc
                ));
            }
            if self.state.due.is_none() {
                match cx.now().checked_add(self.delay_ticks) {
                    Some(due) => {
                        self.state.due = Some(due);
                        self.state.starts += 1;
                    }
                    None => return cx.fail("completion stub deadline overflow"),
                }
            }
        }
        self.update_line(cx);
    }

    fn advance(&mut self, cx: &mut Cx<'_>) {
        if !self.state.due.is_some_and(|due| due <= cx.now()) {
            return;
        }
        self.state.due = None;
        self.state.completions += 1;
        for completion in &self.options.completion {
            let word = self.state.words.get(&completion.offset).copied().unwrap_or(0);
            self.state
                .words
                .insert(completion.offset, (word | completion.set) & !completion.clear);
        }
        self.state.pending = true;
        if let Some(irq) = self.options.irq
            && self.options.irq_pulse
            && !self.masked()
        {
            cx.set_irq(irq, true);
            cx.set_irq(irq, false);
        }
        self.update_line(cx);
    }

    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.due
    }

    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.check(&state)?;
        self.state = state;
        Ok(())
    }

    fujimod_emulator_devices::checkpoint_state!(state: State, Self::check);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;

    const BASE: u64 = 0xff97_5000;
    const IRQ: u32 = 206;

    fn stub(extra: &str) -> CompletionStub {
        let text = format!(
            "irq = {IRQ}\n\
             registers = [{{ offset = 0x04, mode = \"constant\", value = 0x55 }}]\n\
             starts = [{{ offset = 0x0, mask = 1 }}]\n\
             completion = [{{ offset = 0x10, set = 1 }}, {{ offset = 0x0, clear = 1 }}]\n\
             ack = {{ offset = 0x10, mode = \"w1c\" }}\n{extra}"
        );
        let options: Options = toml::from_str(&text).unwrap();
        CompletionStub::new(BASE, 0x1000, options, 1000).unwrap()
    }
    fn write(s: &mut CompletionStub, h: &mut RecordingHost, offset: u64, value: u32) {
        let mut cx = Cx::new(h, 0, 0, 0);
        s.write(&mut cx, BASE + offset, 4, value.into());
        assert!(cx.refusal().is_none());
    }
    fn read(s: &mut CompletionStub, h: &mut RecordingHost, offset: u64) -> u32 {
        let mut cx = Cx::new(h, 0, 0, 0);
        let value = s.read(&mut cx, BASE + offset, 4) as u32;
        assert!(cx.refusal().is_none());
        value
    }
    fn fire(s: &mut CompletionStub, h: &mut RecordingHost) {
        let due = s.next_deadline(h.now).expect("a completion is due");
        h.now = due;
        let mut cx = Cx::new(h, usize::MAX, 0, 0);
        s.advance(&mut cx);
        assert!(cx.refusal().is_none());
        assert_eq!(s.next_deadline(h.now), None);
    }

    #[test]
    fn a_start_completes_interrupts_and_is_acknowledged() {
        let mut s = stub("");
        let mut h = RecordingHost::default();
        write(&mut s, &mut h, 0x20, 0x1234);
        assert_eq!(read(&mut s, &mut h, 0x20), 0x1234);
        assert_eq!(read(&mut s, &mut h, 0x24), 0);
        assert_eq!(read(&mut s, &mut h, 0x04), 0x55);
        assert_eq!(s.next_deadline(0), None);
        h.now = 7;
        write(&mut s, &mut h, 0x0, 1);
        assert_eq!(s.next_deadline(h.now), Some(7 + 100_000));
        assert_eq!(read(&mut s, &mut h, 0x0), 1);
        assert!(h.irqs.is_empty());
        fire(&mut s, &mut h);
        assert_eq!(read(&mut s, &mut h, 0x10), 1);
        assert_eq!(read(&mut s, &mut h, 0x0), 0);
        assert_eq!(h.irqs, vec![(IRQ, true)]);
        // The checkpoint keeps the pending status and the line level.
        let saved = s.checkpoint().unwrap();
        let mut copy = stub("");
        copy.restore_checkpoint(&saved).unwrap();
        assert_eq!(copy.state, s.state);
        // Writing zero to the w1c status changes nothing.
        write(&mut s, &mut h, 0x10, 0);
        assert_eq!(read(&mut s, &mut h, 0x10), 1);
        write(&mut s, &mut h, 0x10, 1);
        assert_eq!(read(&mut s, &mut h, 0x10), 0);
        assert_eq!(h.irqs, vec![(IRQ, true), (IRQ, false)]);
        assert!(!s.state.pending && !s.state.line);
    }

    #[test]
    fn a_pending_deadline_survives_a_checkpoint() {
        let mut s = stub("delay_us = 3");
        let mut h = RecordingHost::default();
        write(&mut s, &mut h, 0x0, 1);
        let saved = s.checkpoint().unwrap();
        let mut copy = stub("delay_us = 3");
        copy.restore_checkpoint(&saved).unwrap();
        assert_eq!(copy.next_deadline(0), Some(3000));
        fire(&mut copy, &mut h);
        assert_eq!(h.irqs, vec![(IRQ, true)]);
    }

    #[test]
    fn the_mask_bit_holds_the_line_low_until_cleared() {
        let mut s = stub("irq_mask_offset = 0x14\nirq_mask_bit = 0");
        let mut h = RecordingHost::default();
        write(&mut s, &mut h, 0x14, 1);
        write(&mut s, &mut h, 0x0, 1);
        fire(&mut s, &mut h);
        assert_eq!(read(&mut s, &mut h, 0x10), 1);
        assert!(h.irqs.is_empty());
        write(&mut s, &mut h, 0x14, 0);
        assert_eq!(h.irqs, vec![(IRQ, true)]);
        write(&mut s, &mut h, 0x10, 1);
        assert_eq!(h.irqs, vec![(IRQ, true), (IRQ, false)]);
    }

    #[test]
    fn a_start_value_must_match_and_write_any_acknowledges() {
        let options: Options = toml::from_str(
            "irq = 199\n\
             starts = [{ offset = 0x8, mask = 0x3, value = 0x2 }]\n\
             completion = [{ offset = 0xc, set = 0x100 }]\n\
             ack = { offset = 0x18, mode = \"write-any\", status_offset = 0xc }\n\
             retain_unwritten_reads_zero = false",
        )
        .unwrap();
        let mut s = CompletionStub::new(BASE, 0x1000, options, 1000).unwrap();
        let mut h = RecordingHost::default();
        write(&mut s, &mut h, 0x8, 0x1);
        write(&mut s, &mut h, 0x8, 0x3);
        assert_eq!(s.next_deadline(0), None);
        write(&mut s, &mut h, 0x8, 0x6);
        fire(&mut s, &mut h);
        assert_eq!(read(&mut s, &mut h, 0xc), 0x100);
        assert_eq!(h.irqs, vec![(199, true)]);
        write(&mut s, &mut h, 0x18, 0);
        assert_eq!(read(&mut s, &mut h, 0xc), 0);
        assert_eq!(read(&mut s, &mut h, 0x18), 0);
        assert_eq!(h.irqs, vec![(199, true), (199, false)]);
        // An unwritten unlisted word stops the run when retention is off.
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        s.read(&mut cx, BASE + 0x40, 4);
        assert!(cx.refusal().is_some());
    }

    #[test]
    fn a_pulse_rises_and_falls_at_the_completion() {
        let mut s = stub("irq_pulse = true");
        let mut h = RecordingHost::default();
        write(&mut s, &mut h, 0x0, 1);
        fire(&mut s, &mut h);
        assert_eq!(h.irqs, vec![(IRQ, true), (IRQ, false)]);
        assert!(s.state.pending && !s.state.line);
    }

    #[test]
    fn invalid_options_are_refused() {
        let options = |text: &str| -> Options { toml::from_str(text).unwrap() };
        assert!(CompletionStub::new(BASE, 0x1000, options("starts = [{ offset = 0x2, mask = 1 }]"), 1000).is_err());
        assert!(CompletionStub::new(BASE, 0x1000, options("irq = 5"), 1000).is_err());
        assert!(CompletionStub::new(BASE, 0x1000, options("irq_mask_offset = 0x4"), 1000).is_err());
        assert!(CompletionStub::new(BASE, 0x1000, options("starts = [{ offset = 0, mask = 1, value = 2 }]"), 1000).is_err());
        assert!(toml::from_str::<Options>("unknown = 1").is_err());
    }
}
