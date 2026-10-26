//! General-DMA address-high and request-selection registers. Section5
//! 227d0/227f8 address paired words; 22820 writes two masked 6-bit lanes.
//! Views share routing/address state with the XDMAC channel pairs.
use fujimod_emulator_ip::socionext::xdmac::{Routing, SharedRouting};
use fujimod_emulator_devices::{Cx, Device};
use serde_json::Value;
use std::sync::{Arc, Mutex};
pub type Banks = [SharedRouting; 8];
pub fn banks() -> Banks {
    std::array::from_fn(|_| Arc::new(Mutex::new(Routing::default())))
}

pub struct AddressHigh {
    banks: Banks,
}
impl AddressHigh {
    pub fn new(banks: Banks) -> Self {
        Self { banks }
    }
    fn words(&self) -> [u32; 16] {
        std::array::from_fn(|i| self.banks[i / 2].lock().unwrap().high(i % 2))
    }
}
impl Device for AddressHigh {
    fn name(&self) -> &'static str {
        "dma-address-high"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(0xff90e748);
        if size == 4 && offset < 64 && offset & 3 == 0 {
            u64::from(self.words()[offset as usize / 4])
        } else {
            cx.unmodeled();
            0
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(0xff90e748);
        if size == 4 && offset < 64 && offset & 3 == 0 {
            let slot = offset as usize / 4;
            if value > u32::MAX as u64
                || self.banks[slot / 2]
                    .lock()
                    .unwrap()
                    .set_high(slot % 2, value as u32)
                    .is_err()
            {
                cx.unmodeled();
            }
        } else {
            cx.unmodeled();
        }
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        let offset = address.wrapping_sub(0xff90e748);
        size == 4 && offset < 64 && offset & 3 == 0
    }
    fn save(&self) -> Value {
        serde_json::json!(self.words())
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let words: [u32; 16] = serde_json::from_value(value.clone())
            .map_err(|e| format!("DMA address-high checkpoint: {e}"))?;
        let mut candidates: Vec<_> = self
            .banks
            .iter()
            .map(|b| b.lock().unwrap().clone())
            .collect();
        for (slot, word) in words.into_iter().enumerate() {
            candidates[slot / 2].set_high(slot % 2, word)?;
        }
        for (bank, candidate) in self.banks.iter().zip(candidates) {
            *bank.lock().unwrap() = candidate;
        }
        Ok(())
    }
}
pub struct RequestRouting {
    banks: Banks,
}
impl RequestRouting {
    pub fn new(banks: Banks) -> Self {
        Self { banks }
    }
    fn words(&self) -> [u32; 8] {
        std::array::from_fn(|i| {
            let bank = self.banks[i].lock().unwrap();
            bank.request(0) as u32 | ((bank.request(1) as u32) << 8)
        })
    }
}
impl Device for RequestRouting {
    fn name(&self) -> &'static str {
        "dma-request-routing"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(0xff90e500);
        if size == 4 && offset < 32 && offset & 3 == 0 {
            u64::from(self.words()[offset as usize / 4])
        } else {
            cx.unmodeled();
            0
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(0xff90e500);
        if size != 4 || offset >= 32 || offset & 3 != 0 || value & !0x01013f3f != 0 {
            cx.unmodeled();
            return;
        }
        let mut bank = self.banks[offset as usize / 4].lock().unwrap();
        let mut candidate = bank.clone();
        for (lane, enable, shift) in [(0, 0x10000, 0), (1, 0x1000000, 8)] {
            if value & enable != 0
                && candidate
                    .set_request(lane, ((value >> shift) & 63) as u8)
                    .is_err()
            {
                cx.unmodeled();
                return;
            }
        }
        *bank = candidate;
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        let offset = address.wrapping_sub(0xff90e500);
        size == 4 && offset < 32 && offset & 3 == 0
    }
    fn save(&self) -> Value {
        serde_json::json!(self.words())
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let words: [u32; 8] = serde_json::from_value(value.clone())
            .map_err(|e| format!("DMA routing checkpoint: {e}"))?;
        if words.iter().any(|v| v & !0x3f3f != 0) {
            return Err("DMA routing checkpoint has reserved bits".into());
        }
        let mut candidates: Vec<_> = self
            .banks
            .iter()
            .map(|b| b.lock().unwrap().clone())
            .collect();
        for (slot, word) in words.into_iter().enumerate() {
            candidates[slot].set_request(0, (word & 63) as u8)?;
            candidates[slot].set_request(1, ((word >> 8) & 63) as u8)?;
        }
        for (bank, candidate) in self.banks.iter().zip(candidates) {
            *bank.lock().unwrap() = candidate;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    #[test]
    fn lanes_are_independently_masked_and_latches_restore_without_events() {
        let mut host = RecordingHost::default();
        let mut route = RequestRouting::new(banks());
        let mut cx = Cx::new(&mut host, 4, 0, 0);
        route.write(&mut cx, 0xff90e500, 4, 0x1003f);
        route.write(&mut cx, 0xff90e500, 4, 0x1001200);
        assert_eq!(route.read(&mut cx, 0xff90e500, 4), 0x123f);
        route.write(&mut cx, 0xff90e500, 4, 0);
        assert_eq!(route.read(&mut cx, 0xff90e500, 4), 0x123f);
        let saved = route.save();
        let mut restored = RequestRouting::new(banks());
        restored.restore(&saved).unwrap();
        assert_eq!(restored.save(), saved);
        assert!(
            restored
                .restore(&serde_json::json!([0, 0, 0, 0, 0, 0, 0, 0]))
                .is_ok()
        );
        route.write(&mut cx, 0xff90e500, 4, 0x80000000);
        assert!(cx.is_unmodeled());
        let mut high = AddressHigh::new(banks());
        for slot in 0..16 {
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            high.write(&mut cx, 0xff90e748 + slot * 4, 4, slot * 0x10001);
            assert_eq!(high.read(&mut cx, 0xff90e748 + slot * 4, 4), slot * 0x10001);
        }
        let saved = high.save();
        let mut restored = AddressHigh::new(banks());
        restored.restore(&saved).unwrap();
        assert_eq!(restored.save(), saved);
        assert!(restored.restore(&serde_json::json!([0, 1])).is_err());
        assert_eq!(high.next_deadline(0), None);
        assert_eq!(route.next_deadline(0), None);
        assert!(host.events.is_empty());
        assert!(host.irqs.is_empty());
    }
}
