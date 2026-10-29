//! XP5 general-DMA global initialization words; channel pairs use the common XDMAC.
use fujimod_emulator_devices::{Cx, Device};
use serde_json::Value;
#[derive(Default)]
pub struct Global {
    regs: [u32; 7],
}
impl Device for Global {
    fn name(&self) -> &'static str {
        "general-dma-global"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        let o = address.wrapping_sub(0xff90e560);
        if n == 4 && o < 28 && o & 3 == 0 {
            self.regs[o as usize / 4] as u64
        } else {
            cx.unmodeled();
            0
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, v: u64) {
        let o = address.wrapping_sub(0xff90e560);
        if n == 4 && o < 28 && o & 3 == 0 {
            self.regs[o as usize / 4] = v as u32
        } else {
            cx.unmodeled();
        }
    }
    fn save(&self) -> Value {
        serde_json::json!(self.regs)
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        self.regs = serde_json::from_value(v.clone())
            .map_err(|e| format!("invalid DMA global checkpoint: {e}"))?;
        Ok(())
    }
}
