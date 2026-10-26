//! Shared imaging session. Register only explicit owner-fragment windows.
use fujimod_emulator_devices::graph::DeviceGraph;
use fujimod_emulator_devices::graph::DeviceWindow;
use fujimod_emulator_sensor_input::statistics;
use fujimod_emulator_sensor_input::{sensor, slvs};
use fujimod_emulator_xp5_imaging::imaging::focus;
use fujimod_emulator_xp5_imaging::imaging::input;
use fujimod_emulator_xp5_imaging::imaging::iris;
use fujimod_emulator_xp5_imaging::imaging::paf;
use fujimod_emulator_xp5_imaging::imaging::preview;
use fujimod_emulator_xp5_imaging::imaging::work;
use fujimod_emulator_xp5_imaging::imaging::{STATISTICS_BANK_SIZE, STATISTICS_BASE};

pub fn register_all(
    bus: &mut DeviceGraph,
    windows: &[DeviceWindow],
    pins: &fujimod_emulator_devices::xp5::pins::SharedPins,
) -> Result<(), String> {
    for w in windows.iter().filter(|w| {
        matches!(
            w.kind.as_str(),
            "imaging-serial-sink" | "focus-actuator" | "iris-actuator"
        )
    }) {
        if !(0xff913000..0xff918000).contains(&w.base) || w.base & 0xfff != 0 || w.size != 0x1000 {
            return Err("invalid virtual AF/hot-shoe-flash serial window".into());
        }
        let sink = fujimod_emulator_xp5_imaging::serial_sink::Sink::new(w.base);
        let sink = if w.base == 0xff914000 {
            sink.with_finder(pins.clone())
        } else {
            sink
        };
        if !w.options.is_empty() {
            return Err("serial actuator takes no options".into());
        }
        if w.kind == "focus-actuator" {
            if w.base != 0xff914000 {
                return Err("invalid focus actuator base".into());
            }
            bus.register(
                w.base,
                w.size,
                Box::new(focus::Focus::new(sink, pins.clone())),
            )?;
        } else if w.kind == "iris-actuator" {
            if w.base != 0xff915000 {
                return Err("invalid iris actuator base".into());
            }
            bus.register(w.base, w.size, Box::new(iris::Iris::new(sink)))?;
        } else {
            bus.register(w.base, w.size, Box::new(sink))?;
        }
    }
    let sensor = sensor::Sensor::new(
        Some(sensor::ImageRoutes {
            dma: Some([162, 163]),
            timing: [166, 164],
        }),
        (0x59, 4),
    );
    let ticks = |options: &toml::Table, key: &str| -> Result<u64, String> {
        options
            .get(key)
            .and_then(toml::Value::as_integer)
            .and_then(|n| u64::try_from(n).ok())
            .filter(|n| *n > 0)
            .and_then(|n| n.checked_mul(1000))
            .ok_or_else(|| format!("imaging requires bounded positive {key}"))
    };
    for window in windows.iter().filter(|w| w.kind == "imaging-sensor") {
        if window.base != 0xff948000 || window.size != 0x1000 {
            return Err("invalid imaging sensor window".into());
        }
        bus.register(
            window.base,
            window.size,
            Box::new(sensor::Command::new(
                window.base,
                sensor.clone(),
                ticks(&window.options, "command_latency_us")?,
            )?),
        )?;
    }
    for window in windows
        .iter()
        .filter(|w| matches!(w.kind.as_str(), "imaging-receiver" | "imaging-phy"))
    {
        if window.size != 0x1000 {
            return Err("invalid receiver/PHY window size".into());
        }
        match window.kind.as_str() {
            "imaging-receiver" => {
                if !windows.iter().any(|w| w.kind == "imaging-sensor") {
                    return Err("receiver requires sensor session".into());
                }
                let (normal_irqs, error_irqs, output_channels, timing_output) = match window.base {
                    0xff940000 => (vec![154, 156], vec![155, 157], vec![0, 1], true),
                    0xff941000 => (vec![158], vec![159], vec![1], false),
                    _ => return Err("invalid SLVS-EC receiver base".into()),
                };
                let period = ticks(&window.options, "frame_period_us")?;
                let active = if window.options.contains_key("frame_active_us") {
                    ticks(&window.options, "frame_active_us")?
                } else {
                    10_000_000
                }
                .min(period / 2)
                .max(1);
                bus.register(
                    window.base,
                    window.size,
                    Box::new(slvs::Receiver::new(
                        sensor.clone(),
                        slvs::Wiring {
                            base: window.base,
                            normal_irqs,
                            error_irqs,
                            output_channels,
                            timing_output,
                            control: slvs::Control {
                                run_mask: 0x11,
                                run_value: 0x11,
                                reset_pulse: 0,
                                reset_ticks: 0,
                            },
                        },
                        period,
                        active,
                    )?),
                )?;
            }
            _ => {
                if window.base != 0xff944000 {
                    return Err("invalid SLVS-EC PHY base".into());
                }
                bus.register(window.base, window.size, Box::new(input::Phy::default()))?;
            }
        }
    }
    let io = preview::Io::default();
    let statistics = statistics::Statistics::new(STATISTICS_BASE, STATISTICS_BANK_SIZE)?;
    for w in windows.iter().filter(|w| w.kind == "imaging-statistics") {
        if w.base != STATISTICS_BASE || w.size != (STATISTICS_BANK_SIZE * 2) as u64 {
            return Err("invalid statistics SRAM window".into());
        }
        bus.register(w.base, w.size, Box::new(statistics.clone()))?;
    }
    for w in windows.iter().filter(|w| {
        matches!(
            w.kind.as_str(),
            "imaging-rifp" | "imaging-rifp-io" | "imaging-preview"
        )
    }) {
        let expected = match w.kind.as_str() {
            "imaging-rifp" => 0xffd02000,
            "imaging-rifp-io" => 0xffd01000,
            _ => 0xff974000,
        };
        if w.base != expected || w.size != 0x1000 {
            return Err("invalid preview/RIFP window".into());
        }
        let device: Box<dyn fujimod_emulator_devices::Device> = if w.kind == "imaging-rifp-io" {
            Box::new(io.clone())
        } else {
            Box::new(
                preview::Preview::new(w.kind == "imaging-rifp", io.clone(), &w.options)?
                    .with_statistics(statistics.clone()),
            )
        };
        bus.register(w.base, w.size, device)?;
    }
    for w in windows.iter().filter(|w| w.kind == "imaging-input") {
        if w.base != 0xff946000 || w.size != 0x1000 {
            return Err("invalid input-engine window".into());
        }
        bus.register(w.base, w.size, Box::new(input::Input::default()))?;
    }
    for w in windows.iter().filter(|w| {
        matches!(
            w.kind.as_str(),
            "imaging-reset" | "movdc" | "imaging-resize"
        )
    }) {
        if w.size != 0x1000 {
            return Err("invalid imaging work window size".into());
        }
        let number = |key: &str| {
            w.options
                .get(key)
                .and_then(toml::Value::as_integer)
                .and_then(|n| u64::try_from(n).ok())
                .ok_or_else(|| format!("{} requires {key}", w.kind))
        };
        let work = match w.kind.as_str() {
            "imaging-reset" => work::Work::reset(w.base)?,
            "movdc" => work::Work::movdc(
                w.base,
                number("frame_period_us")?,
                u8::try_from(number("source_luma")?).map_err(|e| e.to_string())?,
            )?,
            _ => work::Work::resize(w.base, number("completion_us")?)?,
        };
        bus.register(w.base, w.size, Box::new(work))?;
    }
    for w in windows.iter().filter(|w| w.kind == "imaging-paf") {
        if w.base != 0xff976000 || w.size != 0x1000 {
            return Err("invalid PAF window".into());
        }
        bus.register(w.base, w.size, Box::new(paf::Paf::default()))?;
    }
    Ok(())
}
