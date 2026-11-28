//! Shared imaging session. Register only explicit owner-fragment windows.
use fujimod_emulator_devices::graph::DeviceGraph;
use fujimod_emulator_devices::graph::DeviceWindow;
use fujimod_emulator_sensor_input::statistics;
use fujimod_emulator_sensor_input::{sensor, slvs};
use fujimod_emulator_xp5_imaging::imaging::buffer;
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
    let sensor_window = match windows
        .iter()
        .filter(|w| w.kind == "imaging-sensor")
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => None,
        [window] => Some(*window),
        _ => return Err("more than one imaging-sensor window".into()),
    };
    // The sensor chip's gates and decoded readout are the body's image-sensor part, which the
    // catalog puts on the imaging-sensor window (`read_gate`, `standby_gate`,
    // `frame_timing_readout` and `frame_timing_line_ns`). An option left out is a gate the
    // chip does not have, or a readout whose frames keep the stand-in periods of the receiver
    // and preview windows.
    let gates = match sensor_window {
        Some(w) => sensor::Gates {
            read: sensor_gate(&w.options, "read_gate")?,
            standby: sensor_gate(&w.options, "standby_gate")?,
        },
        None => sensor::Gates::default(),
    };
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
        gates,
    );
    // Frame timing of the decoded sensor readout (sensor::FrameTiming).
    let sensor = match sensor_window {
        Some(w)
            if w.options.contains_key("frame_timing_readout")
                || w.options.contains_key("frame_timing_line_ns") =>
        {
            let number = |key: &str, max: u64| {
                w.options
                    .get(key)
                    .and_then(toml::Value::as_integer)
                    .and_then(|n| u64::try_from(n).ok())
                    .filter(|n| (1..=max).contains(n))
                    .ok_or_else(|| {
                        format!("imaging-sensor {key} must be a positive bounded integer")
                    })
            };
            sensor.with_frame_timing(sensor::FrameTiming {
                readout: number("frame_timing_readout", 0xff)? as u8,
                line_ticks: number("frame_timing_line_ns", 1_000_000)?,
            })
        }
        _ => sensor,
    };
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
    // A RESZ block that takes the RIFP RAW movie stream as an inline source (its catalog
    // `inline_source` fact) completes the RAW movie path: the sensor's two interleaved packed
    // RAW12 DMA lanes carry the host scene, RIFP develops the frame they complete, and RESZ
    // scales it into the two-plane picture the preview and the HDMI output scan out.
    let inline_source = |w: &DeviceWindow| -> Result<bool, String> {
        w.options
            .get("inline_source")
            .map(|value| {
                value
                    .as_bool()
                    .ok_or_else(|| "inline_source must be a boolean".to_owned())
            })
            .transpose()
            .map(|value| value.unwrap_or(false))
    };
    let mut raw_movie = false;
    for w in windows.iter().filter(|w| w.kind == "imaging-resz") {
        raw_movie |= inline_source(w)?;
    }
    let sensor = if raw_movie {
        sensor.with_interleaved_raw12(fujimod_emulator_imaging::frame::still_scene)
    } else {
        sensor
    };
    let yc_output = fujimod_emulator_xp5_imaging::imaging::still::YcOutput::default();
    // XT5_MOVIE_PATH=real: the modelled movie data path (MOVDC -> RIFP inline -> RESZ -> TNR,
    // or sensor RAW -> RIFP -> RESZ), see fujimod_emulator_xp5_imaging::imaging::movie.
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
                if window
                    .options
                    .get("retain_words")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
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
    // A statistics window marked `ram` (the catalog marks one that lies in the run's declared
    // RAM, such as boot SRAM) registers no device: the banks are guest memory, the imaging DMA
    // writes them through the bus, and the guest reads them back from RAM. Without any
    // statistics window the banks are guest memory too.
    let mut in_memory = true;
    for w in windows.iter().filter(|w| w.kind == "imaging-statistics") {
        if w.base != STATISTICS_BASE || w.size != (STATISTICS_BANK_SIZE * 2) as u64 {
            return Err("invalid statistics SRAM window".into());
        }
        match w.options.get("ram") {
            None => in_memory = false,
            Some(toml::Value::Boolean(ram)) => in_memory &= *ram,
            Some(_) => return Err("imaging-statistics ram must be a boolean".into()),
        }
    }
    let statistics = if in_memory {
        statistics::Statistics::in_memory(STATISTICS_BASE, STATISTICS_BANK_SIZE)?
    } else {
        statistics::Statistics::new(STATISTICS_BASE, STATISTICS_BANK_SIZE)?
    };
    // AE/AWB statistics measure the host scene at the sensor's programmed exposure.
    let statistics = statistics.with_sensor_scene(sensor.exposure());
    if !in_memory {
        for w in windows.iter().filter(|w| w.kind == "imaging-statistics") {
            bus.register(w.base, w.size, Box::new(statistics.clone()))?;
        }
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
        // The RIFP window may also hold the pages of its sub-blocks (0xffd03000..0xffd0bfff).
        let pages = if w.kind == "imaging-rifp" {
            1..=10
        } else {
            1..=1
        };
        if w.base != expected || w.size % 0x1000 != 0 || !pages.contains(&(w.size / 0x1000)) {
            return Err("invalid preview/RIFP window".into());
        }
        let device: Box<dyn fujimod_emulator_devices::Device> = if w.kind == "imaging-rifp-io" {
            // Register-list DMA (see preview::Io): ticks from START to its done status.
            // Opt-in; a command-image scene adds source_luma to this window too.
            if w.options
                .keys()
                .any(|key| !matches!(key.as_str(), "queue_completion_us" | "source_luma"))
            {
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
                .with_span(w.size)
                .with_statistics(statistics.clone())
                .with_frame_clock(sensor.clone());
            let device = if raw_movie {
                device.with_raw_movie()
            } else {
                device
            };
            Box::new(if still_path || movie_path || raw_movie {
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
        // Option `completion_us`: a +0x40 start completes with interrupt 169 after that time.
        let device = match w.options.get("completion_us") {
            None => buffer::Buffer::default(),
            Some(_) => {
                buffer::Buffer::default().with_completion(ticks(&w.options, "completion_us")?)?
            }
        };
        bus.register(w.base, w.size, Box::new(device))?;
    }
    // RESZ 0xffd0c000 and RZL2 0xff984000: the completion stubs of 55-develop-stubs.toml that
    // also scale with the still path.
    // The advanced-filter scaler 0xff983000 is the same stub; it scales at every start, with
    // or without the still path (its live-view user reads the destination as the panel picture).
    for w in windows.iter().filter(|w| {
        matches!(
            w.kind.as_str(),
            "imaging-resz" | "imaging-rzl2" | "imaging-filter-scaler"
        )
    }) {
        use fujimod_emulator_xp5_imaging::imaging::still;
        let (layout, base) = match w.kind.as_str() {
            "imaging-resz" => (&still::RESZ, 0xffd0c000),
            "imaging-rzl2" => (&still::RZL2, 0xff984000),
            _ => (&still::FILTER, 0xff983000),
        };
        let always = w.kind == "imaging-filter-scaler";
        if w.base != base || w.size != 0x1000 {
            return Err(format!("invalid {} window", w.kind));
        }
        let mut options = w.options.clone();
        options.remove("inline_source");
        let options: fujimod_emulator_xp5_io::completion_stub::Options =
            toml::Value::Table(options)
                .try_into()
                .map_err(|e| format!("{} options: {e}", w.kind))?;
        // XP5 board time is in nanoseconds.
        let stub = fujimod_emulator_xp5_io::completion_stub::CompletionStub::new(
            w.base, w.size, options, 1000,
        )?;
        bus.register(
            w.base,
            w.size,
            Box::new({
                let resz = still::Resz::with_layout(
                    layout,
                    w.base,
                    stub,
                    (still_path || always).then(|| yc_output.clone()),
                );
                let resz = if movie_path && !always {
                    resz.with_movie_path(yc_output.clone())
                } else {
                    resz
                };
                if inline_source(w)? {
                    resz.with_raw_movie_path(yc_output.clone())
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
        let mut device = input::Input::default();
        if w.options.contains_key("frame_period_us") || w.options.contains_key("frame_active_us") {
            device = device.with_cadence(
                ticks(&w.options, "frame_period_us")?,
                ticks(&w.options, "frame_active_us")?,
            )?;
        }
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
        // Option `busy_status`: word 15 bit 31 reads as the engine's busy status (see `Input`).
        let device = match w.options.get("busy_status") {
            None => device,
            Some(value) => match value.as_bool() {
                Some(true) => device.with_busy_status(),
                Some(false) => device,
                None => return Err("busy_status must be a boolean".into()),
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
            // Live view follows the sensor's programmed exposure.
            "movdc" => work::Work::movdc(
                w.base,
                number("frame_period_us")?,
                u8::try_from(number("source_luma")?).map_err(|e| e.to_string())?,
            )?
            .with_exposure(sensor.exposure()),
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

/// The `{register, mask}` table of the imaging-sensor window option `key`, if present.
fn sensor_gate(options: &toml::Table, key: &str) -> Result<Option<(u16, u8)>, String> {
    let Some(value) = options.get(key) else {
        return Ok(None);
    };
    let field = |name: &str| {
        value
            .as_table()
            .and_then(|table| table.get(name))
            .and_then(toml::Value::as_integer)
    };
    let register = field("register").and_then(|n| u16::try_from(n).ok());
    let mask = field("mask")
        .and_then(|n| u8::try_from(n).ok())
        .filter(|mask| *mask != 0);
    match (register, mask, value.as_table().map(toml::Table::len)) {
        (Some(register), Some(mask), Some(2)) => Ok(Some((register, mask))),
        _ => Err(format!(
            "imaging-sensor {key} must be a {{register, mask}} table with a nonzero 8-bit mask"
        )),
    }
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
    for code in std::env::var("XT5_STILL_RAW16")
        .unwrap_or_default()
        .split(',')
    {
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
        let mut windows = crate::boot::xt5_windows(&["imaging-resz", "imaging-rzl2"]);
        windows.sort_by(|a, b| a.kind.cmp(&b.kind));
        assert_eq!(
            windows
                .iter()
                .map(|w| (w.kind.as_str(), w.base))
                .collect::<Vec<_>>(),
            [("imaging-resz", 0xffd0c000), ("imaging-rzl2", 0xff984000)]
        );
        let mut bus = DeviceGraph::new();
        register_all(
            &mut bus,
            &windows,
            &fujimod_emulator_devices::xp5::pins::Pins::shared(),
        )
        .unwrap();
        let mut moved = windows[1].clone();
        moved.base = 0xff983000;
        assert!(
            register_all(
                &mut DeviceGraph::new(),
                &[moved],
                &fujimod_emulator_devices::xp5::pins::Pins::shared()
            )
            .is_err()
        );
    }

    /// A statistics window marked `ram` registers no device over the guest's RAM; an unmarked
    /// one is the statistics device.
    #[test]
    fn a_ram_statistics_window_registers_no_device() {
        let windows = crate::boot::xt5_windows(&["imaging-statistics"]);
        assert_eq!(windows.len(), 1);
        let pins = fujimod_emulator_devices::xp5::pins::Pins::shared();
        let mut graph = DeviceGraph::new();
        register_all(&mut graph, &windows, &pins).unwrap();
        assert!(graph.at(STATISTICS_BASE).is_ok());
        let mut ram = windows.clone();
        ram[0]
            .options
            .insert("ram".into(), toml::Value::Boolean(true));
        let mut graph = DeviceGraph::new();
        register_all(&mut graph, &ram, &pins).unwrap();
        assert!(graph.at(STATISTICS_BASE).is_err());
        ram[0].options.insert("ram".into(), toml::Value::Integer(1));
        assert!(register_all(&mut DeviceGraph::new(), &ram, &pins).is_err());
    }

    /// The sensor gates come only from `{register, mask}` tables on the window: no option is
    /// no gate, and the retired boolean escape options are refused.
    #[test]
    fn sensor_gates_are_the_window_tables() {
        let options: toml::Table = toml::from_str(
            "read_gate = { register = 0x59, mask = 4 }\nstandby_gate = false\n\
             wide = { register = 0x10000, mask = 1 }\nzero = { register = 0, mask = 0 }\n\
             extra = { register = 0, mask = 1, value = 1 }",
        )
        .unwrap();
        assert_eq!(sensor_gate(&options, "read_gate"), Ok(Some((0x59, 4))));
        assert_eq!(sensor_gate(&options, "absent"), Ok(None));
        for key in ["standby_gate", "wide", "zero", "extra"] {
            assert!(sensor_gate(&options, key).is_err(), "{key}");
        }
        let mut windows = crate::boot::xt5_windows(&["imaging-sensor"]);
        windows[0]
            .options
            .insert("read_gate".into(), toml::Value::Boolean(false));
        let pins = fujimod_emulator_devices::xp5::pins::Pins::shared();
        assert!(register_all(&mut DeviceGraph::new(), &windows, &pins).is_err());
    }

    /// The X-T5 4.31 manifest's 0xff983000 window registers as the advanced-filter scaler, and
    /// only at that base.
    #[test]
    fn the_xt5_filter_scaler_window_registers() {
        let windows = crate::boot::xt5_windows(&["imaging-filter-scaler"]);
        assert_eq!(
            windows.iter().map(|w| w.base).collect::<Vec<_>>(),
            [0xff983000]
        );
        assert_eq!(
            windows[0]
                .options
                .get("irq")
                .and_then(toml::Value::as_integer),
            Some(204)
        );
        let pins = fujimod_emulator_devices::xp5::pins::Pins::shared();
        register_all(&mut DeviceGraph::new(), &windows, &pins).unwrap();
        let mut moved = windows[0].clone();
        moved.base = 0xff984000;
        assert!(register_all(&mut DeviceGraph::new(), &[moved], &pins).is_err());
    }
}
