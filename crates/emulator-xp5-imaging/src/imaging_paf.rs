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
}
impl Device for Paf {
    fn name(&self) -> &'static str {
        "imaging-paf"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(0xff976000);
        self.advance(cx);
        if size != 4 || !matches!(offset, 0 | 0x300 | 0x308) {
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
            cx.unmodeled();
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
    fn save(&self) -> Value {
        serde_json::to_value(self).unwrap()
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        *self =
            serde_json::from_value(value.clone()).map_err(|e| format!("PAF checkpoint: {e}"))?;
        Ok(())
    }
}
