//! A host that records scheduled events, interrupt edges and core resets over
//! a flat RAM window.
use crate::{Host, Record};

#[derive(Default)]
pub struct RecordingHost {
    pub now: u64,
    pub events: Vec<(usize, u64, u64)>,
    pub irqs: Vec<(u32, bool)>,
    pub core_irqs: Vec<(usize, u32, bool)>,
    pub ram: Vec<u8>,
    pub irq: bool,
    pub ram_base: u64,
    pub resets: Vec<(usize, bool)>,
}
impl Host for RecordingHost {
    fn now(&self) -> u64 {
        self.now
    }
    fn schedule(&mut self, device: usize, at: u64, token: u64) {
        self.events.push((device, at, token));
    }
    fn set_irq(&mut self, line: u32, level: bool) {
        self.irqs.push((line, level));
        self.irq = level;
    }
    fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
        Err("no per-core interrupt controller".into())
    }
    fn set_core_irq(&mut self, cpu: usize, line: u32, high: bool) -> Result<(), String> {
        self.core_irqs.push((cpu, line, high));
        Ok(())
    }
    fn reset_core(&mut self, cpu: usize, asserted: bool) -> Result<(), String> {
        self.resets.push((cpu, asserted));
        Ok(())
    }
    fn ram_read(&self, address: u64, out: &mut [u8]) -> Result<(), String> {
        let start = address.checked_sub(self.ram_base).ok_or("below RAM")? as usize;
        out.copy_from_slice(
            self.ram
                .get(start..start + out.len())
                .ok_or("outside RAM")?,
        );
        Ok(())
    }
    fn ram_write(&mut self, address: u64, bytes: &[u8]) -> Result<(), String> {
        let start = address.checked_sub(self.ram_base).ok_or("below RAM")? as usize;
        self.ram
            .get_mut(start..start + bytes.len())
            .ok_or("outside RAM")?
            .copy_from_slice(bytes);
        Ok(())
    }
    fn record(&mut self, _: Record) {}
}
impl RecordingHost {
    pub fn bytes(&self, address: u64, size: usize) -> Result<Vec<u8>, String> {
        let mut bytes = vec![0; size];
        self.ram_read(address, &mut bytes)?;
        Ok(bytes)
    }
}
