//! FFF02080 peripheral DMA channel used by the UART serial stream.
//!
//! Its register configuration selects UART transmission; payload bytes are
//! opaque transport data. Completion means transmission accepted, never external
//! recipient effects.
use fujimod_emulator_devices::WordMap;
use fujimod_emulator_devices::record::{Result, Unsupported};
pub const BASE: u32 = 0xfff02080;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct PeripheralDma {
    pub busy: bool,
    pub generation: u64,
}
impl PeripheralDma {
    pub fn control(&mut self, value: u32, registers: &mut WordMap) -> Result<bool> {
        match value {
            8 => {
                self.generation += 1;
                self.busy = false;
                registers.insert(BASE + 0x24, 0);
                Ok(false)
            }
            1 => Ok(true),
            _ => Err(Unsupported(format!(
                "Unsupported boot peripheral DMA control {value:#x}"
            ))),
        }
    }
    pub fn preflight(&self) -> Result<()> {
        if self.busy {
            Err(Unsupported::from("Boot peripheral DMA start while busy"))
        } else {
            Ok(())
        }
    }
    pub fn start(&mut self, registers: &mut WordMap) -> u64 {
        self.generation += 1;
        self.busy = true;
        registers.insert(super::super::uart::BASE + 0xc, 0);
        self.generation
    }
    pub fn complete(&mut self, generation: u64, registers: &mut WordMap) -> bool {
        if generation != self.generation {
            return false;
        }
        self.busy = false;
        registers.insert(super::super::uart::BASE + 0xc, 1);
        registers.insert(BASE + 0x24, 0x40);
        true
    }
}
