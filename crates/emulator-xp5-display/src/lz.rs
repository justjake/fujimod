//! XP5 DMA high-address registers and an explicitly selected OSD transport.
//! The register-level engine is shared in `crate::lz_osd`.
use crate::osd as codec;
use fujimod_emulator_devices::{Cx, Device};
use serde_json::Value;
use std::sync::{Arc, Mutex};

pub struct PlaneCodec;
impl fujimod_emulator_devices::lz_osd::OsdCodec for PlaneCodec {
    fn pixel_bytes(&self, width: u32, height: u32) -> Result<usize, String> {
        Ok(codec::Header::new(width, height)?.bytes)
    }
    fn secondary_bytes(&self) -> usize {
        codec::HEADER_BYTES
    }
    fn encode(
        &self,
        primary: u64,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<Vec<u8>, String> {
        Ok(codec::encode(primary, width, height, pixels)?.to_vec())
    }
}

pub struct AddressHigh(pub Arc<Mutex<[u32; 2]>>);
impl Device for AddressHigh {
    fn name(&self) -> &'static str {
        "lz-osd-dma-high"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        let o = address.wrapping_sub(0xff90e708);
        if n != 4 || !matches!(o, 0 | 4) {
            cx.unmodeled();
            return 0;
        }
        self.0.lock().unwrap()[o as usize / 4] as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, v: u64) {
        let o = address.wrapping_sub(0xff90e708);
        if n != 4 || !matches!(o, 0 | 4) {
            cx.unmodeled();
            return;
        }
        if v > 7 {
            cx.fail("DMA high address exceeds 35 bits");
            return;
        }
        self.0.lock().unwrap()[o as usize / 4] = v as u32;
    }
    fn save(&self) -> Value {
        serde_json::json!(*self.0.lock().unwrap())
    }
    fn restore(&mut self, s: &Value) -> Result<(), String> {
        let value = serde_json::from_value(s.clone())
            .map_err(|e| format!("invalid OSD DMA address checkpoint: {e}"))?;
        *self.0.lock().unwrap() = value;
        Ok(())
    }
}
