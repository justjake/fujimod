//! Connect the XP5 JPEG encoder window to the shared X-Processor encoder
//! under its wide register contract (`fujimod_emulator_ip::jpeg`).
use fujimod_emulator_devices::graph::{DeviceGraph, DeviceWindow};
use fujimod_emulator_ip::jpeg::{Encoder, Registers, WINDOW};

pub fn register(bus: &mut DeviceGraph, window: &DeviceWindow) -> Result<(), String> {
    if window.size != WINDOW
        || window
            .options
            .keys()
            .any(|key| !matches!(key.as_str(), "irq" | "run_ticks"))
    {
        return Err("invalid JPEG encoder window or options".into());
    }
    let number = |key: &str| {
        window
            .options
            .get(key)
            .and_then(toml::Value::as_integer)
            .and_then(|n| u64::try_from(n).ok())
            .ok_or_else(|| format!("JPEG encoder requires {key}"))
    };
    let irq = u32::try_from(number("irq")?).map_err(|e| e.to_string())?;
    bus.register(
        window.base,
        window.size,
        Box::new(Encoder::new(
            "xp5-jpeg-encoder",
            window.base,
            irq,
            number("run_ticks")?,
            Registers::Wide,
        )?),
    )?;
    Ok(())
}
