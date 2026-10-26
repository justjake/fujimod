//! A small RAM/clock host for storage device unit tests.
use crate::{Cx, Host, Record};
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    Write(u64, usize),
    Irq(u32, bool),
}
pub struct Ram {
    pub now: u64,
    pub bytes: Vec<u8>,
    pub effects: Vec<Effect>,
}
impl Ram {
    pub fn new(size: usize) -> Self {
        Self {
            now: 0,
            bytes: vec![0; size],
            effects: Vec::new(),
        }
    }
    pub fn cx(&mut self) -> Cx<'_> {
        Cx::new(self, 0, 0, 0)
    }
}
impl Host for Ram {
    fn now(&self) -> u64 {
        self.now
    }
    fn schedule(&mut self, _: usize, _: u64, _: u64) {}
    fn set_irq(&mut self, line: u32, level: bool) {
        self.effects.push(Effect::Irq(line, level));
    }
    fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
        panic!("storage drives level IRQs")
    }
    fn set_core_irq(&mut self, _: usize, line: u32, level: bool) -> Result<(), String> {
        self.set_irq(line, level);
        Ok(())
    }
    fn ram_read(&self, address: u64, out: &mut [u8]) -> Result<(), String> {
        let p = address as usize & 0x3fffffff;
        let s = self
            .bytes
            .get(p..p + out.len())
            .ok_or("test DMA read out of RAM")?;
        out.copy_from_slice(s);
        Ok(())
    }
    fn ram_write(&mut self, address: u64, bytes: &[u8]) -> Result<(), String> {
        let p = address as usize & 0x3fffffff;
        self.bytes
            .get_mut(p..p + bytes.len())
            .ok_or("test DMA write out of RAM")?
            .copy_from_slice(bytes);
        self.effects.push(Effect::Write(address, bytes.len()));
        Ok(())
    }
    fn record(&mut self, _: Record) {}
}
