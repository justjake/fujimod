//! XP5-specific scanout model configuration and host output wiring.
use crate::output::Outputs;
use fujimod_emulator_xp5_display::display::Config;
use fujimod_framebuffer::raster::PixelOrder;
use std::path::PathBuf;

pub(crate) fn configure(
    outputs: &Outputs,
        base: u64,
        options: &toml::Table,
    ) -> Result<Config, String> {
        let valid = [
            "frame_dir",
            "frame_period_ns",
            "raw_format0",
            "raw_format1",
            "raw_format2",
            "raw_format3",
        ];
        if let Some(key) = options.keys().find(|k| !valid.contains(&k.as_str())) {
            return Err(format!("unknown display option {key}"));
        }
        let bank = match base {
            0xff990000 => 0,
            0xff9a0000 => 1,
            _ => return Err("invalid display output base".into()),
        };
        let number = |key: &str| {
            options
                .get(key)
                .map(|v| {
                    v.as_integer()
                        .and_then(|n| u64::try_from(n).ok())
                        .ok_or_else(|| format!("{key} must be a nonnegative integer"))
                })
                .transpose()
        };
        let mut config = Config {
            period_ns: number("frame_period_ns")?,
            ..Default::default()
        };
        for (n, format) in config.raw_formats.iter_mut().enumerate() {
            if let Some(v) = options.get(&format!("raw_format{n}")) {
                *format = Some(PixelOrder::parse(
                    v.as_str().ok_or("raw format must be a string")?,
                )?);
            }
        }
        let Some(directory) = options.get("frame_dir") else {
            // Observers can consume raw scanout through the session.
            return Ok(config);
        };
        let directory = PathBuf::from(directory.as_str().ok_or("frame_dir must be a path")?);
    outputs.add(bank, directory)?;
    Ok(config)
}
