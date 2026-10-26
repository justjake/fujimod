//! The four registers of a lens motor bridge, written over I2C and read by the bridge model.
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct Registers(Arc<Mutex<[u8; 4]>>);
impl Registers {
    pub fn write_register(&self, address: u8, value: u8) -> Result<(), String> {
        if address >= 4 {
            return Err("unknown lens bridge register".into());
        }
        self.0.lock().unwrap()[address as usize] = value;
        Ok(())
    }
    pub fn read_register(&self, address: u8) -> Result<u8, String> {
        self.0
            .lock()
            .unwrap()
            .get(address as usize)
            .copied()
            .ok_or_else(|| "unknown lens bridge register".into())
    }
    /// All four registers at once, as the bridge model reads them.
    pub fn snapshot(&self) -> [u8; 4] {
        *self.0.lock().unwrap()
    }
}
