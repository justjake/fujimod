//! Shared SDHCI register storage.
use serde::{Deserialize, Serialize};

/// Fixed register storage shared with SDIO. No map allocation on MMIO.
#[derive(Serialize, Deserialize)]
pub struct Registers(#[serde(with = "fujimod_emulator_devices::checkpoint::array")] pub [u16; 132]);
impl Registers {
    pub fn new() -> Self {
        Self([0; 132])
    }
    pub fn get(&self, offset: usize) -> u16 {
        self.0[offset / 2]
    }
    pub fn set(&mut self, offset: usize, value: u16) {
        self.0[offset / 2] = value;
    }
    pub fn word(&self, offset: usize) -> u32 {
        self.get(offset) as u32 | (self.get(offset + 2) as u32) << 16
    }
    pub fn put_word(&mut self, offset: usize, value: u32) {
        self.set(offset, value as u16);
        self.set(offset + 2, (value >> 16) as u16);
    }
    pub fn valid(offset: usize, size: u8) -> bool {
        size == 2
            && matches!(
                offset,
                4 | 6
                    | 8
                    | 10
                    | 12
                    | 14
                    | 16
                    | 18
                    | 20
                    | 22
                    | 24
                    | 26
                    | 28
                    | 30
                    | 0x28
                    | 0x2e
                    | 0x30
                    | 0x32
                    | 0x34
                    | 0x36
                    | 0x38
                    | 0x3a
                    | 0x3e
                    | 0x100
                    | 0x104
            )
            || size == 4 && matches!(offset, 0 | 12 | 0x20 | 0x24)
    }
    pub fn asserted(&self) -> bool {
        self.get(0x30) & self.get(0x34) & self.get(0x38) != 0
            || self.get(0x32) & self.get(0x36) & self.get(0x3a) != 0
    }
}

impl Default for Registers {
    fn default() -> Self {
        Self::new()
    }
}
