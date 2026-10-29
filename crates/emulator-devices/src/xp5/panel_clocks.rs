//! XP5 panel pixel-clock PLL state that the panel PLL registers and the clock gates share.
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct Pll {
    pub n: u32,
    pub fraction: u32,
    pub dividers: [u32; 4],
    pub request: u32,
    pub generation: u64,
}
#[derive(Default, Serialize, Deserialize)]
pub struct State {
    pub plls: [Pll; 4],
    pub selector: u32,
}
#[derive(Clone, Default)]
pub struct Clocks(pub Arc<Mutex<State>>);
impl Clocks {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn selector(&self, value: u32) {
        self.0.lock().unwrap().selector = value & 1;
    }
    /// Reproduce14f3520's float32 arithmetic (returned MHz), including its
    /// low-byte divider reads. Fractional PLL words are retained but the
    /// frequency query does not incorporate them.
    pub fn pixel_hz(&self, bank: usize) -> Result<f64, String> {
        let s = self.0.lock().unwrap();
        let ratio = |p: Pll| p.n as f32 / ((p.dividers[0] & 255) + 1) as f32;
        let mhz = if bank == 1 {
            ratio(s.plls[0]) * 12.0
        } else {
            let factor = if s.selector == 0 { 24.0 } else { 48.0 };
            ratio(s.plls[2]) * factor * ratio(s.plls[3])
        };
        if !mhz.is_finite() || mhz <= 0.0 {
            return Err(format!("bank{bank} pixel clock not programmed"));
        }
        Ok(mhz as f64 * 1_000_000.0)
    }
}
