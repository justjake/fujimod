//! Pure XP5 device construction. Firmware transport, CPU execution and host
//! output publication are supplied by the application, outside these models.
use fujimod_emulator_devices::graph::{DeviceGraph, DeviceWindow};
use fujimod_emulator_devices::parts::controls::Controls;
use fujimod_emulator_xp5_io::{
    adc, connection_status, discarded_read, gates, gpio, mailbox, memory_configuration,
    reset_handshake, timer, write_only_registers,
};
use fujimod_emulator_xp5_display::{display, panel_clock, panel_serial};
use fujimod_emulator_xp5_storage::{
    flash_storage,
    flash_storage_controller,
    flash_storage_dma,
    nand_controller,
};
use fujimod_emulator_xp5_imaging::serial_sink;
use std::collections::BTreeMap;
use std::sync::Arc;
pub mod audio;
pub mod clocks;
pub mod codec;
pub mod general_dma;
pub mod i2c;
pub mod imaging;
pub mod jpeg;
pub mod lz;
pub mod renderer;
pub mod serial;
pub mod sd;
pub mod uart;
pub mod usb;

pub struct Companion {
    pub pins: mailbox::Pins,
    pub channel: mailbox::Channel,
}
pub struct Resources {
    pub sd: BTreeMap<u64, sd::Media>,
    pub controls: Arc<dyn Controls>,
    pub pins: fujimod_emulator_devices::xp5::pins::SharedPins,
    pub clocks: fujimod_emulator_devices::xp5::panel_clocks::Clocks,
    pub companion: Option<Companion>,
    /// Verified and decoded image backing, shared by aperture, controller and DMA.
    pub storage: Option<flash_storage::Medium>,
    pub displays: BTreeMap<u64, display::Config>,
    /// Extended-timer PWM words, read by the IBIS stage model behind input capture.
    pub pwm: fujimod_emulator_xp5_io::timer::PwmOutputs,
}
fn register_with_resources(
    bus: &mut DeviceGraph,
    window: &DeviceWindow,
    resources: &mut Resources,
    unknown: &mut dyn FnMut(&DeviceWindow) -> Result<(), String>,
) -> Result<(), String> {
    let clocks = &resources.clocks;
    let companion = resources.companion.as_ref().map(|c| c.pins.companion);
    match window.kind.as_str() {
        "unmodeled-bus-drain" => {
            if !window.options.is_empty() { return Err("unmodeled bus drain takes no options".into()); }
            bus.register(window.base, window.size, Box::new(UnmodeledBusDrain))?;
        }
        "sd-interface" => {
            if window.size != 8 || !window.options.is_empty() {return Err("Invalid SD interface window".into());}
            bus.register(window.base,window.size,Box::new(fujimod_emulator_xp5_io::sd_interface::Interface::for_slot(window.base, resources.sd.get(&(window.base & !0xfff)).map(|slot| slot.contacts.clone()))?))?;
        }
        "sdhci" => sd::register(bus, window, &mut resources.sd)?,
        "sdhci-uhs2" => sd::register_uhs2(bus, window, &resources.sd)?,
        "boot-control" => {
            if window.base != 0xff90e000 || window.size != 4 {
                return Err("invalid boot-control source window".into());
            }
            let inputs: fujimod_emulator_xp5_io::boot_control::Inputs =
                toml::Value::Table(window.options.clone()).try_into().map_err(|e|format!("boot-control inputs: {e}"))?;
            bus.register_windows(vec![
                fujimod_emulator_devices::graph::Mapping { base:0xff90e000,size:4 },
                fujimod_emulator_devices::graph::Mapping { base:0xff90e400,size:8 },
                fujimod_emulator_devices::graph::Mapping { base:0xfa140000,size:4 },
            ],Box::new(fujimod_emulator_xp5_io::boot_control::BootControl::new(inputs)))?;
        }
        "interprocessor-mailbox" => {
            if window.base != 0xff90e100 || window.size != 0x24 || !window.options.is_empty() {
                return Err("invalid interprocessor mailbox window".into());
            }
            let companion = resources
                .companion
                .as_ref()
                .ok_or("mailbox requires a companion processor")?;
            bus.register(
                window.base,
                window.size,
                Box::new(mailbox::Mailbox::new(
                    window.base,
                    companion.pins,
                    companion.channel.clone(),
                )),
            )?;
        }
        "discarded-read" => {
            if !window.options.is_empty() {
                return Err("discarded-read takes no options".into());
            }
            bus.register(
                window.base,
                window.size,
                Box::new(discarded_read::DiscardedRead::new(window.base, window.size)?),
            )?;
        }
        "usb-connection-status" => {
            if !window.options.is_empty() {
                return Err("USB connection-status takes no options".into());
            }
            // Bit 0 is set while a USB cable or jig is attached.
            bus.register(
                window.base,
                window.size,
                Box::new(connection_status::ConnectionStatus::new(
                    "usb-connection-status",
                    window.base,
                    window.size,
                    true,
                )?),
            )?;
        }
        "usb-device-controller" => {
            bus.register(window.base, window.size, Box::new(usb::controller(window)?))?;
        }
        "usb-phy" => {
            bus.register(window.base, window.size, Box::new(usb::phy(window)?))?;
        }
        "hdmi-hot-plug-status" => {
            if window.options.keys().any(|key| key != "connected_level") {
                return Err("unknown HDMI hot-plug status option".into());
            }
            let connected_level = match window
                .options
                .get("connected_level")
                .and_then(toml::Value::as_integer)
            {
                Some(0) => false,
                Some(1) => true,
                _ => return Err("HDMI hot-plug status requires connected_level 0 or 1".into()),
            };
            bus.register(
                window.base,
                window.size,
                Box::new(connection_status::ConnectionStatus::new(
                    "hdmi-hot-plug-status",
                    window.base,
                    window.size,
                    connected_level,
                )?),
            )?;
        }
        "i2c-controller" => {
            bus.register(
                window.base,
                window.size,
                Box::new(i2c::build(window.base, window.size, &window.options)?),
            )?;
        }
        "soc-temperature" => {
            if window.base != 0xff905000 || window.size != 0x1000 {
                return Err("invalid SoC temperature window".into());
            }
            bus.register(
                window.base,
                window.size,
                Box::new(fujimod_emulator_xp5_io::temperature::Monitor::default()),
            )?;
        }
        "display-output-link" => {
            if window.base != 0xff9a1000 || window.size != 0x1c {
                return Err("invalid display output link window".into());
            }
            bus.register(
                window.base,
                window.size,
                Box::new(fujimod_emulator_xp5_io::output_link::Link::default()),
            )?;
        }
        "reset-handshake" => {
            if window.options.keys().any(|key| key != "trigger_offset") {
                return Err("unknown reset-handshake option".into());
            }
            let trigger_offset = window
                .options
                .get("trigger_offset")
                .and_then(toml::Value::as_integer)
                .and_then(|value| u64::try_from(value).ok())
                .ok_or("reset-handshake requires an unsigned trigger_offset")?;
            if trigger_offset & 3 != 0
                || trigger_offset
                    .checked_add(4)
                    .is_none_or(|end| end > window.size)
            {
                return Err("invalid reset-handshake trigger offset".into());
            }
            bus.register(
                window.base,
                window.size,
                Box::new(reset_handshake::ResetHandshake::new(
                    window.base,
                    trigger_offset,
                )?),
            )?;
        }
        "memory-configuration" => {
            if window.size != 0x1000
                || window.options.keys().any(|key| key != "commit_ticks")
            {
                return Err("invalid memory-configuration window or options".into());
            }
            let commit_ticks = window
                .options
                .get("commit_ticks")
                .and_then(toml::Value::as_integer)
                .and_then(|value| u64::try_from(value).ok())
                .filter(|value| *value > 0)
                .ok_or("memory-configuration requires positive commit_ticks")?;
            bus.register(
                window.base,
                window.size,
                Box::new(memory_configuration::MemoryConfiguration::new(
                    window.base,
                    commit_ticks,
                )?),
            )?;
        }
        "pl011" => {
            bus.register(window.base, window.size, Box::new(uart::port(window)?))?;
        }
        "bringup-stub" => {
            let options: fujimod_emulator_xp5_io::bringup_stub::Options =
                toml::Value::Table(window.options.clone()).try_into().map_err(|e| format!("bring-up stub options: {e}"))?;
            bus.register(
                window.base,
                window.size,
                Box::new(fujimod_emulator_xp5_io::bringup_stub::BringupStub::new(window.base, window.size, options)?),
            )?;
        }
        "completion-stub" => {
            let options: fujimod_emulator_xp5_io::completion_stub::Options =
                toml::Value::Table(window.options.clone()).try_into().map_err(|e| format!("completion stub options: {e}"))?;
            // XP5 board time is in nanoseconds.
            let stub = fujimod_emulator_xp5_io::completion_stub::CompletionStub::new(
                window.base,
                window.size,
                options,
                1000,
            )?;
            if window.base == fujimod_emulator_xp5_imaging::imaging::mix::MIX_BASE && window.size == 0x1000 {
                // MIX 0xffd22000: the same stub that, with XT5_STILL_PATH=real, also writes the
                // HDR blend and composite (emulator-xp5-imaging imaging_mix.rs). Same device
                // name and checkpoint, so the manifest and snapshots are unchanged.
                let mix = fujimod_emulator_xp5_imaging::imaging::mix::Mix::new(stub);
                bus.register(
                    window.base,
                    window.size,
                    Box::new(if fujimod_emulator_imaging::frame::still_path_real() {
                        mix.with_still_path()
                    } else {
                        mix
                    }),
                )?;
            } else {
                bus.register(window.base, window.size, Box::new(stub))?;
            }
        }
        "imaging-fcnv" => {
            // FCNV 0xffd26000: the completion stub that also converts YCbCr 4:2:2 to RGB
            // (emulator-xp5-imaging imaging_fcnv.rs).
            if window.base != 0xffd26000 || window.size != 0x1000 {
                return Err("invalid imaging-fcnv window".into());
            }
            let options: fujimod_emulator_xp5_io::completion_stub::Options =
                toml::Value::Table(window.options.clone()).try_into().map_err(|e| format!("imaging-fcnv options: {e}"))?;
            let stub = fujimod_emulator_xp5_io::completion_stub::CompletionStub::new(
                window.base,
                window.size,
                options,
                1000,
            )?;
            bus.register(window.base, window.size, Box::new(fujimod_emulator_xp5_imaging::imaging::fcnv::Fcnv::new(stub)))?;
        }
        "imaging-tnr" => {
            // TNR 0xffd24000: the completion stub that, with XT5_MOVIE_PATH=real, also copies
            // the current picture to the encoder input (emulator-xp5-imaging imaging_movie.rs).
            // Registered here, in manifest order, so the device graph matches snapshots taken
            // when the window was a plain completion stub.
            if window.base != 0xffd24000 || window.size != 0x1000 {
                return Err("invalid imaging-tnr window".into());
            }
            let options: fujimod_emulator_xp5_io::completion_stub::Options =
                toml::Value::Table(window.options.clone()).try_into().map_err(|e| format!("imaging-tnr options: {e}"))?;
            let stub = fujimod_emulator_xp5_io::completion_stub::CompletionStub::new(
                window.base,
                window.size,
                options,
                1000,
            )?;
            let tnr = fujimod_emulator_xp5_imaging::imaging::movie::Tnr::new(stub);
            bus.register(
                window.base,
                window.size,
                Box::new(if fujimod_emulator_imaging::frame::movie_path_real() {
                    tnr.with_movie_path()
                } else {
                    tnr
                }),
            )?;
        }
        "write-only-registers" => {
            if !window.options.is_empty() {
                return Err("write-only-registers takes no options".into());
            }
            bus.register(
                window.base,
                window.size,
                Box::new(write_only_registers::WriteOnlyRegisters::new(
                    window.base,
                    window.size,
                )?),
            )?;
        }
        "audio-spi-sink" => {
            if window.base != 0xff918000 || window.size != 0x1000 {
                return Err("invalid virtual audio SPI window".into());
            }
            bus.register(
                window.base,
                window.size,
                Box::new(serial_sink::Sink::new(window.base)),
            )?;
        }
        "renderer" => renderer::register(bus, window.base, window.size)?,
        "display" => {
            if window.size != 0x1000 || !window.options.is_empty() {
                return Err("invalid display window or options".into());
            }
            let config = resources
                .displays
                .remove(&window.base)
                .ok_or("display requires explicit scanout configuration")?;
            bus.register(
                window.base,
                window.size,
                Box::new(display::Display::new(window.base, clocks.clone(), config)?),
            )?;
        }
        "clock-banks" => clocks::register(bus, window.base, window.size)?,
        "lz" => lz::register(bus, window.base, window.size)?,
        "jpeg-encoder" => jpeg::register(bus, window)?,
        "jpeg-decoder" => jpeg::register_decoder(bus, window)?,
        "xp5-codec" => codec::register(bus, window)?,
        "panel-spi" => {
            if window.base != 0xff919000 || window.size != 0x1000 {
                return Err("invalid SPI9 panel window".into());
            }
            bus.register(
                window.base,
                window.size,
                Box::new(panel_serial::PanelSerial::new(
                    resources.pins.clone(),
                    (
                        window
                            .options
                            .get("select_port")
                            .and_then(toml::Value::as_integer)
                            .and_then(|n| usize::try_from(n).ok())
                            .ok_or("panel SPI requires select_port")?,
                        window
                            .options
                            .get("select_pin")
                            .and_then(toml::Value::as_integer)
                            .and_then(|n| usize::try_from(n).ok())
                            .ok_or("panel SPI requires select_pin")?,
                    ),
                    window
                        .options
                        .get("source_hz")
                        .and_then(toml::Value::as_integer)
                        .and_then(|n| u64::try_from(n).ok())
                        .ok_or("panel SPI requires source_hz")?,
                )?),
            )?;
        }
        "panel-pll" => {
            bus.register(
                window.base,
                window.size,
                Box::new(panel_clock::PanelPll::new(window.base, clocks.clone())?),
            )?;
        }
        "clock-gates" | "power-gates" => {
            if window
                .options
                .keys()
                .any(|key| window.kind != "clock-gates" || key != "configuration_word")
            {
                return Err("unknown gate option".into());
            }
            let configuration_word = window
                .options
                .get("configuration_word")
                .map(|value| {
                    value
                        .as_integer()
                        .and_then(|value| u32::try_from(value).ok())
                        .ok_or("configuration_word must be an unsigned 32-bit integer")
                })
                .transpose()?;
            let gates =
                gates::Gates::with_panel_clocks(window.kind == "power-gates", Some(clocks.clone()))
                    .with_companion(companion)
                    .with_configuration_word(configuration_word);
            bus.register(window.base, window.size, Box::new(gates))?;
        }
        "timer-extended" => {
            if window.base != 0xff909000 || window.size != 0x1000 {
                return Err("invalid extended timer window".into());
            }
            bus.register(
                window.base,
                window.size,
                Box::new(timer::Timer::extended(window.base).with_pwm_outputs(resources.pwm.clone())),
            )?;
        }
        "timer" | "timer-focus-capture" => {
            if window.base != 0xff908000 || window.size != 0x1000 || !window.options.is_empty() {
                return Err("invalid general timer window or options".into());
            }
            let mut timer = timer::Timer::new(window.base).with_companion(companion);
            if window.kind == "timer-focus-capture" {
                timer = timer.with_focus_capture(resources.pins.clone());
            }
            bus.register(window.base, window.size, Box::new(timer))?;
        }
        "pin-control" => {
            bus.register(
                window.base,
                window.size,
                Box::new(fujimod_emulator_devices::keyed_io::KeyedIoGroup::new(window.base)),
            )?;
        }
        "adc" => {
            if window.base != adc::BASE
                || window.size != 0x1000
                || window.options.keys().any(|k| k != "samples")
            {
                return Err("invalid ADC window or options".into());
            }
            bus.register(
                window.base,
                window.size,
                Box::new(adc::Adc::new(
                    resources.controls.clone(),
                    serde_json::from_value(
                        serde_json::to_value(
                            window
                                .options
                                .get("samples")
                                .ok_or("ADC requires samples")?,
                        )
                        .map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| format!("ADC samples: {e}"))?,
                )?),
            )?;
        }
        "gpio" | "gpio-port3" => {
            if window
                .options
                .keys()
                .any(|k| {
                    !matches!(
                        k.as_str(),
                        "input_irqs" | "feedback" | "initial_outputs" | "initial_inputs" | "pad_model"
                    )
                })
            {
                return Err("unknown GPIO option".into());
            }
            if let Some(model) = window.options.get("pad_model") {
                if window.kind != "gpio" {
                    return Err("pad_model belongs on the gpio window".into());
                }
                let model: fujimod_emulator_devices::xp5::pins::PadModel =
                    serde_json::from_value(serde_json::to_value(model).map_err(|e| e.to_string())?)
                        .map_err(|e| format!("GPIO pad_model: {e}"))?;
                resources.pins.lock().unwrap().pad_model = model;
            }
            let feedback = window
                .options
                .get("feedback")
                .map(|v| {
                    serde_json::from_value(serde_json::to_value(v).map_err(|e| e.to_string())?)
                        .map_err(|e| format!("GPIO feedback: {e}"))
                })
                .transpose()?
                .unwrap_or_default();
            let initial_inputs: Vec<gpio::InitialInput> = window
                .options
                .get("initial_inputs")
                .map(|v| {
                    serde_json::from_value(serde_json::to_value(v).map_err(|e| e.to_string())?)
                        .map_err(|e| format!("initial GPIO inputs: {e}"))
                })
                .transpose()?
                .unwrap_or_default();
            let feedback: Vec<fujimod_emulator_devices::parts::gpio::Feedback> = feedback;
            if initial_inputs.iter().any(|input| {
                feedback
                    .iter()
                    .any(|wire| (wire.input_port, wire.input_pin) == (input.port, input.pin))
            }) {
                return Err("a GPIO pad driven by feedback cannot declare an initial input".into());
            }
            bus.register(
                window.base,
                window.size,
                Box::new(
                    gpio::Gpio::new(
                        window.base,
                        resources.pins.clone(),
                        window.kind == "gpio-port3",
                        window
                            .options
                            .get("input_irqs")
                            .map(|v| {
                                serde_json::from_value(
                                    serde_json::to_value(v).map_err(|e| e.to_string())?,
                                )
                                .map_err(|e| format!("GPIO input routes: {e}"))
                            })
                            .transpose()?
                            .unwrap_or_default(),
                    )?
                    .with_initial_outputs(
                        window
                            .options
                            .get("initial_outputs")
                            .map(|v| {
                                serde_json::from_value(
                                    serde_json::to_value(v).map_err(|e| e.to_string())?,
                                )
                                .map_err(|e| format!("initial GPIO outputs: {e}"))
                            })
                            .transpose()?
                            .unwrap_or_default(),
                    )?
                    .with_initial_inputs(initial_inputs)?
                    .with_feedback(feedback)?,
                ),
            )?;
        }
        "spi-controller" => {
            bus.register(
                window.base,
                window.size,
                Box::new(serial::build(
                    window.base,
                    resources.pins.clone(),
                    &window.options,
                    resources.controls.clone(),
                )?),
            )?;
        }
        _ => unknown(window)?,
    }
    Ok(())
}

/// Construct the declared graph. Unknown kinds are delegated to the caller's
/// explicit strict/discovery policy; they never acquire a fallback model.
pub fn register_all(
    bus: &mut DeviceGraph,
    windows: &[DeviceWindow],
    resources: &mut Resources,
    unknown: &mut dyn FnMut(&DeviceWindow) -> Result<(), String>,
) -> Result<(), String> {
    imaging::register_all(bus, windows, &resources.pins)?;
    let mut aperture = false;
    for w in windows.iter().filter(|w| w.kind == "flash-storage") {
        if aperture || w.base != flash_storage::BASE || w.size != flash_storage::SIZE {
            return Err("invalid or duplicate flash storage aperture".into());
        }
        if w.options
            .keys()
            .any(|k| !matches!(k.as_str(), "erased_tail" | "seed" | "medium"))
        {
            return Err("unknown flash storage option".into());
        }
        let number = |key: &str| {
            w.options
                .get(key)
                .and_then(toml::Value::as_integer)
                .and_then(|n| u64::try_from(n).ok())
                .ok_or_else(|| format!("flash storage requires {key}"))
        };
        if number("erased_tail")? != 255 {
            return Err("only explicit erased_tail=255 is supported".into());
        }
        let medium_name = w
            .options
            .get("medium")
            .and_then(toml::Value::as_str)
            .unwrap_or("nor");
        let storage = resources
            .storage
            .as_ref()
            .ok_or("aperture requires explicit flash storage")?;
        match (medium_name, storage) {
            ("nor", flash_storage::Medium::Nor(_)) => {}
            ("serial-nand", flash_storage::Medium::Nand(_)) => {}
            _ => return Err("flash storage medium differs from the declared storage".into()),
        }
        if let Some(seeds) = w.options.get("seed").and_then(toml::Value::as_array) {
            for seed in seeds {
                let offset = seed
                    .get("offset")
                    .and_then(toml::Value::as_integer)
                    .and_then(|n| u64::try_from(n).ok())
                    .ok_or("invalid flash storage seed offset")?;
                let value = seed
                    .get("value")
                    .and_then(toml::Value::as_integer)
                    .and_then(|n| u8::try_from(n).ok())
                    .ok_or("invalid flash storage seed byte")?;
                storage.seed(offset, value)?;
            }
        }
        bus.register(
            w.base,
            w.size,
            Box::new(flash_storage::Aperture(storage.clone())),
        )?;
        aperture = true;
    }
    for w in windows
        .iter()
        .filter(|w| {
            w.kind != "flash-storage" && !general_dma::owns(&w.kind) && !audio::owns(&w.kind)
        })
    {
        match w.kind.as_str() {
            "imaging-serial-sink"
            | "focus-actuator"
            | "iris-actuator"
            | "imaging-sensor"
            | "imaging-receiver"
            | "imaging-phy"
            | "imaging-rifp"
            | "imaging-rifp-io"
            | "imaging-preview"
            | "imaging-buffer"
            | "imaging-table"
            | "imaging-input"
            | "imaging-reset"
            | "movdc"
            | "imaging-resize"
            | "imaging-statistics"
            | "imaging-paf"
            | "imaging-resz"
            | "imaging-rzl2"
            | "imaging-filter-scaler" => continue,
            "clock-banks" => {
                let specialized: Vec<u64> = windows
                    .iter()
                    .filter(|w| w.kind == "panel-pll")
                    .map(|w| w.base)
                    .collect();
                clocks::register_except(bus, w.base, w.size, &specialized)?;
            }
            "flash-storage-controller" => {
                if w.base != 0xff90a000 || w.size != 0x1000 {
                    return Err("invalid serial-flash storage window".into());
                }
                let jedec = w
                    .options
                    .get("jedec_id")
                    .and_then(toml::Value::as_integer)
                    .and_then(|n| u32::try_from(n).ok())
                    .filter(|n| *n <= 0xffffff)
                    .ok_or("flash storage-controller requires jedec_id")?;
                match resources
                    .storage
                    .clone()
                    .ok_or("controller requires flash storage aperture")?
                {
                    flash_storage::Medium::Nor(storage) => {
                        let part = flash_storage_controller::Part::from_jedec(jedec)?;
                        bus.register(
                            w.base,
                            w.size,
                            Box::new(flash_storage_controller::Controller::new(storage, part)),
                        )?;
                    }
                    flash_storage::Medium::Nand(storage) => {
                        // The X-E5 part identifies as W25N01JW-family 0xEFDC21.
                        if jedec != 0xefdc21 {
                            return Err("only 0xefdc21 serial-NAND geometry is supported".into());
                        }
                        bus.register(
                            w.base,
                            w.size,
                            Box::new(nand_controller::NandController::new(storage, jedec)),
                        )?;
                    }
                }
            }
            "flash-storage-dma" => {
                if w.base != 0xff939000 || w.size != 0x1000 {
                    return Err("invalid flash storage DMA window".into());
                }
                bus.register(
                    w.base,
                    w.size,
                    Box::new(flash_storage_dma::Dma::new(
                        resources
                            .storage
                            .clone()
                            .ok_or("DMA requires flash storage aperture")?,
                    )),
                )?;
            }
            _ => register_with_resources(bus, w, resources, unknown)?,
        }
    }
    general_dma::register_all(
        bus,
        windows,
        resources.companion.as_ref().map(|c| c.pins.companion),
        resources.pins.clone(),
        resources.pwm.clone(),
        // Only a declared aperture exposes the flash backing on the bus.
        resources.storage.as_ref().filter(|_| aperture),
    )?;
    audio::register_all(bus, windows)?;
    // Sensor command completion changes the receiver's active source. No
    // periodic polling deadline is needed to observe that causal edge.
    let sensors: Vec<_> = windows
        .iter()
        .filter(|w| w.kind == "imaging-sensor")
        .map(|w| bus.at(w.base))
        .collect::<Result<_, _>>()?;
    for w in windows.iter().filter(|w| w.kind == "imaging-receiver") {
        bus.depend_on(bus.at(w.base)?, sensors.clone())?;
    }
    // Pin-producing devices wake GPIO input routing, including self-writes
    // through a different GPIO register bank sharing the same pins.
    let pin_sources: Vec<_> = windows
        .iter()
        .filter(|w| {
            matches!(
                w.kind.as_str(),
                "gpio" | "gpio-port3" | "panel-spi" | "focus-actuator" | "imaging-serial-sink"
            )
        })
        .map(|w| bus.at(w.base))
        .collect::<Result<_, _>>()?;
    for w in windows
        .iter()
        .filter(|w| matches!(w.kind.as_str(), "gpio" | "gpio-port3"))
    {
        let id = bus.at(w.base)?;
        bus.depend_on(
            id,
            pin_sources.iter().copied().filter(|s| *s != id).collect(),
        )?;
    }
    Ok(())
}

// An explicitly unmodeled window can be present in a board graph. Every access
// still refuses; its presence supplies no completion or register value.
struct UnmodeledBusDrain;
impl fujimod_emulator_devices::Device for UnmodeledBusDrain {
    fn name(&self) -> &'static str { "unmodeled-bus-drain" }
    fn read(&mut self, cx: &mut fujimod_emulator_devices::Cx<'_>, _: u64, _: u8) -> u64 {
        cx.unmodeled(); 0
    }
    fn write(&mut self, cx: &mut fujimod_emulator_devices::Cx<'_>, _: u64, _: u8, _: u64) {
        cx.unmodeled();
    }
}
