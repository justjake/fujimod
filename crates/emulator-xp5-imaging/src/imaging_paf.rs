//! PAF reset completion used by2f1712c, not a phase-detection algorithm.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Default, Serialize, Deserialize)]
pub struct Paf {
    registers: BTreeMap<u64, u32>,
    generation: u64,
    deadline: Option<u64>,
    /// A started run (+0x0 bit 0) that has not finished yet.
    #[serde(default)]
    run: Option<u64>,
    /// The run-finished interrupt is raised and not yet cleared (+0x204).
    #[serde(default)]
    pending: bool,
    /// Manifest option `completion_irq`: the interrupt a finished run raises. The GFX100RF 1.13
    /// driver starts a run at 0x2f300dc (+0x300/+0x308 bit 8, then +0x0 bit 0) after enabling the
    /// interrupt at +0x208 (0x2f300b8), and its handler 0x2f2f270 (logical source 0x83, INTID 195
    /// in table 0x19d9d98) clears it at +0x204 and calls the phase AF task back. No phase data is
    /// produced: the result buffers stay as the firmware left them.
    #[serde(skip)]
    completion_irq: Option<u32>,
    /// Device configuration from the manifest, not guest state. Retained words live in
    /// `registers`.
    #[serde(skip)]
    retain_words: bool,
}
impl Paf {
    /// Bring-up option: every aligned word with no modeled meaning reads back its last write.
    pub fn with_retained_words(mut self) -> Self {
        self.retain_words = true;
        self
    }
    pub fn with_completion_irq(mut self, line: u32) -> Self {
        self.completion_irq = Some(line);
        self
    }
    fn irq(&self, cx: &mut Cx<'_>) {
        if let Some(line) = self.completion_irq {
            let enabled = self.registers.get(&0x208).copied().unwrap_or(0) & 1 != 0;
            cx.set_irq(line, self.pending && enabled);
        }
    }
    fn retains(&self, offset: u64, size: u8) -> bool {
        self.retain_words && size == 4 && offset & 3 == 0 && offset < 0x1000
    }
}
impl Device for Paf {
    fn name(&self) -> &'static str {
        "imaging-paf"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(0xff976000);
        self.advance(cx);
        if size != 4 || !matches!(offset, 0 | 0x300 | 0x308) {
            if self.retains(offset, size) {
                return u64::from(self.registers.get(&offset).copied().unwrap_or(0));
            }
            cx.unmodeled();
            return 0;
        }
        u64::from(
            self.registers
                .get(&offset)
                .copied()
                .unwrap_or(if offset == 0 { 0x10000 } else { 0 }),
        )
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(0xff976000);
        self.advance(cx);
        if size == 4 && self.completion_irq.is_some() && matches!(offset, 0x204 | 0x208) {
            if offset == 0x204 {
                if value & 1 != 0 {
                    self.pending = false;
                }
            } else {
                self.registers.insert(offset, value as u32);
            }
            self.irq(cx);
            return;
        }
        if size != 4 || !matches!(offset, 0 | 0x300 | 0x308) {
            if self.retains(offset, size) {
                self.registers.insert(offset, value as u32);
            } else {
                cx.unmodeled();
            }
            return;
        }
        self.registers.insert(offset, value as u32);
        if offset == 0 && value & 1 == 0 {
            self.generation = self.generation.wrapping_add(1);
            self.deadline = Some(cx.now() + 100_000);
            cx.schedule(self.deadline.unwrap(), self.generation);
            self.run = None;
        } else if offset == 0 && self.completion_irq.is_some() {
            self.generation = self.generation.wrapping_add(1);
            self.run = Some(cx.now() + 1_000_000);
            cx.schedule(self.run.unwrap(), self.generation);
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token == self.generation {
            self.advance(cx);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.deadline.is_some_and(|at| at <= cx.now()) {
            self.deadline = None;
            *self.registers.entry(0).or_default() |= 0x10000;
        }
        if self.run.is_some_and(|at| at <= cx.now()) {
            self.run = None;
            self.pending = true;
            self.irq(cx);
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        match (self.deadline, self.run) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
    fujimod_emulator_devices::checkpoint_fields!(registers: BTreeMap<u64, u32>, generation: u64, deadline: Option<u64>, run: Option<u64>, pending: bool);
    fn save(&self) -> Value {
        serde_json::to_value(self).unwrap()
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let (retain_words, completion_irq) = (self.retain_words, self.completion_irq);
        *self =
            serde_json::from_value(value.clone()).map_err(|e| format!("PAF checkpoint: {e}"))?;
        self.retain_words = retain_words;
        self.completion_irq = completion_irq;
        Ok(())
    }
}
