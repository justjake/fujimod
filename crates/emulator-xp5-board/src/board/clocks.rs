use fujimod_emulator_devices::graph::DeviceGraph;
use fujimod_emulator_xp5_io::clocks::{BANKS, Bank};

pub fn register(bus: &mut DeviceGraph, base: u64, size: u64) -> Result<(), String> {
    register_except(bus, base, size, &[])
}
/// Explicit specialized banks (e.g. display pixel clocks) retain their owner.
pub fn register_except(
    bus: &mut DeviceGraph,
    base: u64,
    size: u64,
    specialized: &[u64],
) -> Result<(), String> {
    if base != 0xffa00000 || size != 0x20000 {
        return Err("clock-banks requires ffa00000..ffa20000".into());
    }
    if specialized
        .iter()
        .any(|base| !BANKS.iter().any(|(b, _)| b == base))
    {
        return Err("specialized clock bank outside stock table".into());
    }
    for (base, _) in BANKS {
        if specialized.contains(&base) {
            continue;
        }
        bus.register(base, 0x1000, Box::new(Bank::new(base)?))?;
    }
    Ok(())
}
