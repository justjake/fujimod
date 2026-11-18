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
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.deadline
    }
    fujimod_emulator_devices::checkpoint_fields!(registers: BTreeMap<u64, u32>, generation: u64, deadline: Option<u64>);
    fn save(&self) -> Value {
        serde_json::to_value(self).unwrap()
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let retain_words = self.retain_words;
        *self =
            serde_json::from_value(value.clone()).map_err(|e| format!("PAF checkpoint: {e}"))?;
        self.retain_words = retain_words;
        Ok(())
    }
}
