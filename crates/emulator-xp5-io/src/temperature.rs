use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// On-die temperature monitor, distinct from the external ADC thermistors.
/// The wait condition is +830 bit0; the 12-bit value is sign-extended by
/// sample is multiplied by125 millidegrees Celsius.
#[derive(Default, Serialize, Deserialize)]
pub struct Monitor {
    registers: BTreeMap<u64, u32>,
}
impl Device for Monitor {
    fn name(&self) -> &'static str {
        "soc-temperature"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(0xff905000);
        if size != 4 {
            cx.unmodeled();
            return 0;
        }
        match offset {
            0x830 => u64::from(
                self.registers.get(&0x810) == Some(&1) && self.registers.get(&0x814) == Some(&1),
            ),
            0x834 => 200, // 200 *125 mC =25 C; no fabricated guest thermal state
            0xc | 0x28 | 0x810 | 0x814 | 0x818 | 0x820 | 0x828 | 0x840 | 0x844 | 0x848 | 0x84c
            | 0x850 => self.registers.get(&offset).copied().unwrap_or(0) as u64,
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(0xff905000);
        if size != 4 {
            cx.unmodeled();
            return;
        }
        match offset {
            0xc | 0x28 | 0x810 | 0x814 | 0x818 | 0x828 | 0x840 | 0x844 | 0x848 | 0x84c | 0x850 => {
                self.registers.insert(offset, value as u32);
            }
            0x824 => {
                //14a755c acknowledges threshold interrupts;25 C is inside
                // the configured normal range, so none is synthesized.
                *self.registers.entry(0x820).or_default() &= !(value as u32);
            }
            _ => cx.unmodeled(),
        }
    }
    fn save(&self) -> Value {
        json!(self)
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        *self = serde_json::from_value(state.clone())
            .map_err(|e| format!("invalid temperature checkpoint: {e}"))?;
        Ok(())
    }
}
