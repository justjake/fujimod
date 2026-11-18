//! Shared imaging session. Register only explicit owner-fragment windows.
use fujimod_emulator_devices::graph::DeviceGraph;
use fujimod_emulator_devices::graph::DeviceWindow;
use fujimod_emulator_sensor_input::statistics;
use fujimod_emulator_sensor_input::{sensor, slvs};
use fujimod_emulator_xp5_imaging::imaging::{buffer, table};
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
            line_stride: false,
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
            Box::new({
                let command = sensor::Command::new(
                    window.base,
                    sensor.clone(),
                    ticks(&window.options, "command_latency_us")?,
                )?;
                // Bring-up option: unmodeled words of the window keep their last write.
                if window.options.get("retain_words").and_then(|v| v.as_bool()).unwrap_or(false) {
                    command.with_retained_window()
                } else {
                    command
                }
            }),
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
    // Without an imaging-statistics window (X-T5 4.31: 0xff800000 is boot SRAM in the SoC profile) the
    // AE statistics banks are written into guest memory, where the stock code reads them.
    let has_statistics_window = windows.iter().any(|w| w.kind == "imaging-statistics");
    let statistics = if has_statistics_window {
        statistics::Statistics::new(STATISTICS_BASE, STATISTICS_BANK_SIZE)?
    } else {
        statistics::Statistics::in_memory(STATISTICS_BASE, STATISTICS_BANK_SIZE)?
    };
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
            // Register-list DMA (see preview::Io): ticks from START to its done status.
            // Opt-in; a command-image scene adds source_luma to this window too.
            if w.options.keys().any(|key| !matches!(key.as_str(), "queue_completion_us" | "source_luma")) {
                return Err("unknown imaging-rifp-io option".into());
            }
            match w.options.get("queue_completion_us") {
                None => Box::new(io.clone()),
                Some(value) => Box::new(
                    io.clone().with_queue_completion_ticks(
                        value
                            .as_integer()
                            .and_then(|n| u64::try_from(n).ok())
                            .and_then(|n| n.checked_mul(1000))
                            .ok_or("imaging-rifp-io queue_completion_us must be a bounded count")?,
                    ),
                ),
            }
        } else {
            Box::new(
                preview::Preview::new(w.kind == "imaging-rifp", io.clone(), &w.options)?
                    .with_statistics(statistics.clone()),
            )
        };
        bus.register(w.base, w.size, device)?;
    }
    for w in windows.iter().filter(|w| w.kind == "imaging-buffer") {
        if w.base != buffer::BASE || w.size != 0x1000 {
            return Err("invalid input-buffer window".into());
        }
        bus.register(w.base, w.size, Box::new(buffer::Buffer::default()))?;
    }
    for w in windows.iter().filter(|w| w.kind == "imaging-table") {
        let Some(device) = table::Table::reset(w.base).filter(|_| w.size == 0x1000) else {
            return Err("invalid imaging table window".into());
        };
        bus.register(w.base, w.size, Box::new(device))?;
    }
    for w in windows.iter().filter(|w| w.kind == "imaging-input") {
        if w.base != 0xff946000 || w.size != 0x1000 {
            return Err("invalid input-engine window".into());
        }
        let device = input::Input::default();
        let device = if retain_words(&w.options)? {
            device.with_retained_words()
        } else {
            device
        };
        // Option `idle_status`: word 15 reads as the engine's idle status (see `Input`).
        let device = match w.options.get("idle_status") {
            None => device,
            Some(value) => match value.as_bool() {
                Some(true) => device.with_idle_status(),
                Some(false) => device,
                None => return Err("idle_status must be a boolean".into()),
            },
        };
        bus.register(w.base, w.size, Box::new(device))?;
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
        let work = if retain_words(&w.options)? {
            work.with_retained_words()
        } else {
            work
        };
        bus.register(w.base, w.size, Box::new(work))?;
    }
    for w in windows.iter().filter(|w| w.kind == "imaging-paf") {
        if w.base != 0xff976000 || w.size != 0x1000 {
            return Err("invalid PAF window".into());
        }
        let device = paf::Paf::default();
        let device = if retain_words(&w.options)? {
            device.with_retained_words()
        } else {
            device
        };
        bus.register(w.base, w.size, Box::new(device))?;
    }
    Ok(())
}

/// Bring-up option `retain_words`: unmodeled aligned words of the window read back their last
/// write instead of stopping as unmodeled. Off unless the manifest sets it.
fn retain_words(options: &toml::Table) -> Result<bool, String> {
    match options.get("retain_words") {
        None => Ok(false),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| "retain_words must be a boolean".to_string()),
    }
}
