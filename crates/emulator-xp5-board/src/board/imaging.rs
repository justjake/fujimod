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
            // +0x18 of each RAW DMA unit is its pitch: still capture programs 0x3d80 for 7776
            // 16-bit samples, movie live view 11664 for 7776 packed 12-bit samples.
            line_stride: true,
            // INTID 165: VD callback service id 2 (0x1530748, logical 0x77). Its X-T5 4.31
            // users include the display module callback 0x2ffafa8, which arms the movie
            // V_TIMING one-shot (0x3003c88 -> 0x15332b0(0x33)) once per frame.
            frame_start_pulse: Some(165),
        }),
        (0x59, 4),
    );
    // XT5_STILL_PATH=real: the modelled still data path (sensor RAW -> RIFP -> RESZ), see
    // fujimod_emulator_xp5_imaging::imaging::still. A host input, like XT5_SCENE.
    let still_path = fujimod_emulator_imaging::frame::still_path_real();
    let sensor = if still_path {
        sensor.with_still_channel(sensor::StillChannel {
            channel: 1,
            sixteen_bit_codes: still_raw16_codes()?,
            scene: fujimod_emulator_imaging::frame::still_scene,
            optical_black: (24, 66),
        })
    } else {
        sensor
    };
    let yc_output = fujimod_emulator_xp5_imaging::imaging::still::YcOutput::default();
    // XT5_MOVIE_PATH=real: the modelled movie data path (MOVDC -> RIFP inline -> RESZ -> TNR),
    // see fujimod_emulator_xp5_imaging::imaging::movie.
    let movie_path = fujimod_emulator_imaging::frame::movie_path_real();
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
    // AE/AWB statistics measure the host scene at the sensor's programmed exposure.
    let statistics = statistics.with_sensor_scene(sensor.exposure());
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
            let device = preview::Preview::new(w.kind == "imaging-rifp", io.clone(), &w.options)?
                .with_statistics(statistics.clone());
            Box::new(if still_path || movie_path {
                device.with_still_path(preview::StillPath {
                    sensor: sensor.clone(),
                    output: yc_output.clone(),
                    develop: still_path,
                    movie: movie_path,
                })
            } else {
                device
            })
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
    // RESZ 0xffd0c000 and RZL2 0xff984000: the completion stubs of 55-develop-stubs.toml that
    // also scale with the still path.
    for w in windows.iter().filter(|w| matches!(w.kind.as_str(), "imaging-resz" | "imaging-rzl2")) {
        use fujimod_emulator_xp5_imaging::imaging::still;
        let (layout, base) = match w.kind.as_str() {
            "imaging-resz" => (&still::RESZ, 0xffd0c000),
            _ => (&still::RZL2, 0xff984000),
        };
        if w.base != base || w.size != 0x1000 {
            return Err(format!("invalid {} window", w.kind));
        }
        let options: fujimod_emulator_xp5_io::completion_stub::Options =
            toml::Value::Table(w.options.clone())
                .try_into()
                .map_err(|e| format!("{} options: {e}", w.kind))?;
        // XP5 board time is in nanoseconds.
        let stub =
            fujimod_emulator_xp5_io::completion_stub::CompletionStub::new(w.base, w.size, options, 1000)?;
        bus.register(
            w.base,
            w.size,
            Box::new({
                let resz = still::Resz::with_layout(
                    layout,
                    w.base,
                    stub,
                    still_path.then(|| yc_output.clone()),
                );
                if movie_path {
                    resz.with_movie_path(yc_output.clone())
                } else {
                    resz
                }
            }),
        )?;
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
            "imaging-reset" | "movdc" | "imaging-resize" | "imaging-ibuf"
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
            // Live view follows the sensor's programmed exposure.
            "movdc" => work::Work::movdc(
                w.base,
                number("frame_period_us")?,
                u8::try_from(number("source_luma")?).map_err(|e| e.to_string())?,
            )?
            .with_exposure(sensor.exposure()),
            "imaging-ibuf" => work::Work::ibuf(w.base, number("completion_us")?)?,
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

/// Format codes (+0xdc bits 18..16) of the still RAW channel taken as 16-bit containers: code 5
/// (X-T5 4.31 still capture: pitch 0x3d80 = 7872 x 2 bytes for 7776 samples a line), code 0
/// (assumed), and the codes `XT5_STILL_RAW16` lists (comma-separated, 0..7), a host input for
/// bring-up. A pitch of 1.5 up to 2 bytes a sample (0x2d90 for 7776 samples after a MOVIE ->
/// STILL switch) is packed 12-bit whatever the code; see `sensor::StillChannel`.
fn still_raw16_codes() -> Result<u8, String> {
    let mut codes = 1u8 | 1 << 5;
    for code in std::env::var("XT5_STILL_RAW16").unwrap_or_default().split(',') {
        let code = code.trim();
        if code.is_empty() {
            continue;
        }
        match code.parse::<u8>() {
            Ok(n) if n < 8 => codes |= 1 << n,
            _ => return Err(format!("XT5_STILL_RAW16: {code} is not a format code 0..7")),
        }
    }
    Ok(codes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The X-T5 4.31 manifest's RESZ and RZL2 windows register as their scaling stubs.
    #[test]
    fn the_xt5_resz_and_rzl2_windows_register() {
        #[derive(serde::Deserialize)]
        struct Fragment {
            device: Vec<DeviceWindow>,
        }
        let fragment: Fragment = toml::from_str(include_str!(
            "../../../../catalog/firmware/xt5/4.31/xp5-boot/manifest.d/55-develop-stubs.toml"
        ))
        .unwrap();
        let windows: Vec<_> = fragment
            .device
            .into_iter()
            .filter(|w| matches!(w.kind.as_str(), "imaging-resz" | "imaging-rzl2"))
            .collect();
        assert_eq!(
            windows.iter().map(|w| (w.kind.as_str(), w.base)).collect::<Vec<_>>(),
            [("imaging-resz", 0xffd0c000), ("imaging-rzl2", 0xff984000)]
        );
        let mut bus = DeviceGraph::new();
        register_all(&mut bus, &windows, &fujimod_emulator_devices::xp5::pins::Pins::shared()).unwrap();
        let mut moved = windows[1].clone();
        moved.base = 0xff983000;
        assert!(register_all(&mut DeviceGraph::new(), &[moved], &fujimod_emulator_devices::xp5::pins::Pins::shared()).is_err());
    }
}
