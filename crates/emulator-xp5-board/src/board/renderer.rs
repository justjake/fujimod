//! XP5 renderer windows from the selected processor configuration.
use fujimod_emulator_devices::graph::DeviceGraph;
use fujimod_emulator_ip_dmp::smaph::Renderer;
use fujimod_emulator_xp5_display::renderer::AddressHigh;
pub fn register(bus: &mut DeviceGraph, base: u64, size: u64) -> Result<(), String> {
    if base != 0xff93c000 || size != 0x3000 {
        return Err("DMP renderer requires ff93c000+3000".into());
    }
    bus.register(base, size, Box::new(Renderer::new(base, 153)?))?;
    bus.register(0xff90e73c, 4, Box::new(AddressHigh))?;
    Ok(())
}
