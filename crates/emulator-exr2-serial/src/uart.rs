//! UART register bank at FFF46080: retained control, W1C transmit status.
//! Direct writes and all receiver data remain unresolved; only the DMA
//! transport of opaque DMA-addressed bytes is modeled.
use fujimod_emulator_devices::WordMap;
use fujimod_emulator_devices::record::{Result, Unsupported};
pub const BASE: u32 = 0xfff46080;
pub fn read(offset: u32, value: u32) -> Result<u32> {
    if offset == 4 {
        Err(Unsupported::from("Boot serial receiver is not modeled"))
    } else {
        Ok(value)
    }
}
pub fn write(offset: u32, value: u32, registers: &mut WordMap) -> Result<()> {
    if offset == 0xc {
        registers.insert(BASE + offset, registers.get_or_zero(BASE + offset) & !value);
        return Ok(());
    }
    // Retain the value before refusing unsupported direct transmit writes.
    registers.insert(BASE + offset, value);
    if offset == 4 {
        Err(Unsupported::from(
            "Boot serial direct writes are not modeled",
        ))
    } else {
        Ok(())
    }
}
