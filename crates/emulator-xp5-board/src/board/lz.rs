//! Connect the selected XP5 LZ window to the register engine.
use fujimod_emulator_devices::graph::DeviceGraph;
use fujimod_emulator_devices::lz_osd::Decompressor;
use fujimod_emulator_xp5_display::lz::{AddressHigh, PlaneCodec};
use std::sync::{Arc, Mutex};
pub fn register(bus: &mut DeviceGraph, base: u64, size: u64) -> Result<(), String> {
    let high = Arc::new(Mutex::new([0; 2]));
    bus.register(0xff90e708, 8, Box::new(AddressHigh(high.clone())))?;
    bus.register(
        base,
        size,
        Box::new(Decompressor::new(
            base,
            201,
            high,
            Some(Box::new(PlaneCodec)),
        )),
    )?;
    Ok(())
}
