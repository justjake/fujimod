use super::*;
use fujimod_catalog_configuration::Exr2RunScene;

/// The reference X70's NAND sections in the catalog.
fn reference_camera() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../catalog/bodies/x70/cameras/reference"
    ))
}

#[test]
fn clock_override_updates_the_device_clock_and_requires_one_rtc() {
    let mut plan = json!({"nodes":[{"endpoint":{"clock":{
        "initial_seconds": 0, "maximum_seconds": u32::MAX,
        "ticks_per_second": 12_000_000, "started": false
    }}}]});
    apply_clock(&mut plan, 845_000_000).unwrap();
    assert_eq!(
        plan["nodes"][0]["endpoint"]["clock"]["initial_seconds"],
        845_000_000
    );
    assert_eq!(plan["nodes"][0]["endpoint"]["clock"]["started"], false);
    assert!(apply_clock(&mut json!({}), 1).is_err());
    let mut two = json!([plan.clone(), plan]);
    assert!(apply_clock(&mut two, 1).is_err());
}

fn exr2_inputs(resolved: &mut ResolvedConfiguration) -> &mut Exr2RunInputs {
    match resolved.configuration.inputs.as_mut().unwrap() {
        RunInputs::Exr2(inputs) => inputs,
        RunInputs::Xp4(_) | RunInputs::Xp5(_) => panic!("unexpected input model"),
    }
}

fn run_scene(resolved: &mut ResolvedConfiguration) -> &mut Exr2RunScene {
    match resolved.configuration.scene.as_mut().unwrap() {
        RunScene::Exr2(scene) => scene,
        RunScene::Xp4(_) | RunScene::Xp5(_) => panic!("unexpected scene model"),
    }
}

fn model<'a>(prepared: &'a Prepared, name: &str) -> &'a serde_json::Value {
    prepared.devices_value["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["name"] == name)
        .unwrap_or_else(|| panic!("missing prepared node {name}"))
        .get("model")
        .unwrap()
}

#[test]
fn startup_presentation_uses_active_outputs_and_rejects_inactive_buffers() {
    let mut state = json!({"instances":[
        {"instance":0,"graphics":[]},
        {"instance":1,"graphics":[{"channels":[
            {"enabled":true,"configured":true,"width":720,"height":480}
        ]}]}
    ]});
    assert_eq!(presented_instances(&state), [1]);
    state["instances"][1]["graphics"][0]["channels"][0]["enabled"] = json!(false);
    assert!(presented_instances(&state).is_empty());
    state["instances"][1]["graphics"][0]["channels"][0]["enabled"] = json!(true);
    state["instances"][1]["graphics"][0]["channels"][0]["configured"] = json!(false);
    assert!(presented_instances(&state).is_empty());
}

#[test]
fn runtime_catalog_builds_checked_graph_without_language_adapters() {
    let spec = prepare("x70-110-rec", &[]).unwrap();
    let graph = &spec.execution;
    graph.validate().unwrap();
    let plan = Plan::parse(&serde_json::to_vec(&spec.devices_value).unwrap()).unwrap();
    plan.validate_reset_outputs(
        &graph
            .processors
            .iter()
            .map(|p| p.released)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    plan.validate_memory_outputs(
        &graph
            .decoders
            .iter()
            .map(|d| MemoryChoices {
                initial: d.initial,
                choices: d.choices.len(),
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let mut assets = BTreeMap::from([("nand_base".into(), vec![255; 128 << 20])]);
    for asset in &spec.generated_assets {
        assets.insert(
            asset.name.clone(),
            asset.pattern.repeat(asset.size / asset.pattern.len()),
        );
    }
    fujimod_emulator_exr2_board::register_board_builder();
    fujimod_emulator_models::register();
    let resources = serde_json::from_value::<preparation::Spec>(spec.scene.clone())
        .unwrap()
        .build(&assets)
        .unwrap();
    plan.build(resources, 0, &mut RefuseAdapters).unwrap();
    assert!(prepare("unknown", &[]).is_err());
}

#[test]
fn checked_run_configuration_selects_runtime_and_supplies_scene_and_quantum() {
    let spec = prepare("x70-110-rec", &[]).unwrap();
    let configuration = &spec.resolved.configuration;
    assert_eq!(spec.resolved.reference.body.id, configuration.body_id);
    assert_eq!(
        spec.resolved.reference.firmware.id,
        configuration.firmware_id
    );
    let execution = configuration.execution.unwrap();
    let scene = match configuration.scene.as_ref().unwrap() {
        RunScene::Exr2(scene) => scene,
        RunScene::Xp4(_) | RunScene::Xp5(_) => panic!("unexpected scene model"),
    };
    assert_eq!(spec.quantum, execution.quantum);
    assert_eq!(
        spec.execution_value["instructions_per_tick"],
        serde_json::json!([
            execution.instructions_per_tick.numerator,
            execution.instructions_per_tick.denominator
        ])
    );
    assert_eq!(spec.generated_assets.len(), scene.generated_assets.len());
    assert_eq!(
        spec.scene["images"]["live_image"]["asset"],
        scene.images.live_image.asset
    );
    assert_eq!(
        spec.scene["statistics"]["live_statistics"][0]["asset"],
        scene.statistics.live_statistics[0].asset
    );
    let live_frames = spec.devices_value["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["name"] == "live_frames")
        .unwrap();
    assert_eq!(
        live_frames["model"]["options"]["period_ticks"],
        scene.frame_period_ticks
    );
    let json = spec.to_json();
    assert_eq!(json["configuration_id"], "x70-110-rec");
    assert_eq!(json["overlays"], serde_json::json!([]));
    assert_eq!(json["resolved"]["reference"]["body"]["id"], "x70");
    assert!(json["inputs"]["mcu"]["route"].is_number());
    let settings = &json["resolved"]["configuration"]["inputs"]["settings"];
    assert_eq!(settings["camera"], "reference");
    assert_eq!(settings["requested"]["setup"], "done");
}

#[test]
fn rec_configuration_starts_with_manual_lever_and_normal_lcd() {
    let spec = prepare("x70-110-rec", &[]).unwrap();
    let levels = &model(&spec, "serial_gpio")["options"]["input_levels"];
    // The manual lever is asserted (low); the LCD tilt switch is released (high).
    assert_eq!(levels[2].as_u64().unwrap() & (1 << 11), 0);
    assert_ne!(levels[1].as_u64().unwrap() & (1 << 1), 0);
    let controls: fujimod_catalog_controls::FirmwareControlIds =
        serde_json::from_value(spec.resolved.reference.control_ids.clone().unwrap()).unwrap();
    for (control, key, connection) in [
        (
            "auto_mode_selector_lever",
            "KEY_RECL_MANUAL",
            "controls-gpio-2-11",
        ),
        ("lcd_tilt_switch", "KEY_LCD_TILT", "controls-gpio-1-1"),
    ] {
        assert!(controls.inputs.iter().any(|input| {
            input.target == InputTarget::Control { control_id: control.into() }
                && input.debug_key.is_none()
                && controls.native_key_names.as_ref().unwrap().rows.iter().any(|name| name.name == key && name.event == input.event)
                && matches!(&input.electrical, Some(ElectricalInput::Gpio { connection_id, expected_level: false, .. }) if connection_id == connection)
        }));
    }
}

#[test]
fn unsupported_run_configuration_start_is_refused_and_usb_overlay_is_applied() {
    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    resolved.configuration.start = RunStart::Loader;
    assert!(prepared(resolved).err().unwrap().contains("loader-start"));

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    resolved.configuration.start = RunStart::SdCard;
    assert!(prepared(resolved).err().unwrap().contains("SD-card-start"));

    for (purpose, expected) in [
        (RunPurpose::ServiceSession, "service-session"),
        (RunPurpose::Rehearsal, "rehearsal"),
        (RunPurpose::BootAccess, "boot-access"),
        (RunPurpose::Shutdown, "shutdown"),
    ] {
        let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
        resolved.configuration.purpose = purpose;
        assert!(prepared(resolved).err().unwrap().contains(expected));
    }

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    resolved.configuration.inputs = None;
    assert!(
        prepared(resolved)
            .err()
            .unwrap()
            .contains("requires declared startup inputs")
    );

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    resolved.configuration.scene = None;
    assert!(
        prepared(resolved)
            .err()
            .unwrap()
            .contains("requires a declared generated scene")
    );

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    resolved.configuration.execution = None;
    assert!(
        prepared(resolved)
            .err()
            .unwrap()
            .contains("requires a declared execution policy")
    );

    let spec = prepare("x70-110-rec", &["x70-110-rec-usb".into()]).unwrap();
    assert_eq!(spec.resolved.configuration.overlays, ["x70-110-rec-usb"]);
    assert_eq!(
        spec.devices_value["wires"]["usb_cable"]["logic-level"],
        true
    );

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    resolved.configuration.execution.as_mut().unwrap().quantum = 0;
    assert!(prepared(resolved).is_err());
}

#[test]
fn settings_validate_marker_checksum_and_route_extent() {
    let mut spec = prepare("x70-110-rec", &[]).unwrap();
    let mut input = vec![0; 16];
    input[14] = 0xaa;
    input[15] = 0x56;
    settings_packet(&mut spec.devices_value, &spec.mcu, &input).unwrap();
    input[0] = 1;
    assert!(settings_packet(&mut spec.devices_value, &spec.mcu, &input).is_err());
    input[0] = 0;
    input[14] = 0;
    input[15] = 0;
    assert!(packet(&input).is_err());
    assert!(packet(&input[..15]).is_err());
    input[14] = 0xaa;
    input[15] = 0x56;
    assert!(settings_packet(&mut json!({}), &spec.mcu, &input).is_err());
}

#[test]
fn selected_files_are_bounded_and_hash_checked() {
    let path = std::env::temp_dir().join(format!("fujimod-boot-input-{}", std::process::id()));
    std::fs::write(&path, b"abc").unwrap();
    assert_eq!(checked(&path, &Content::identify(b"abc")).unwrap(), b"abc");
    assert!(read(&path, 2).is_err());
    assert!(checked(&path, &Content::identify(b"abd")).is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn configured_start_inputs_change_prepared_devices_or_refuse() {
    let base = prepare("x70-110-rec", &[]).unwrap();

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    exr2_inputs(&mut resolved).adc.samples[0] = 1;
    let adc = prepared(resolved).unwrap();
    assert_eq!(model(&adc, "adc")["options"]["samples"][0], 1);

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    exr2_inputs(&mut resolved).mcu.packet[0] = 1;
    {
        let inputs = exr2_inputs(&mut resolved);
        let last = inputs.mcu.packet.len() - 1;
        inputs.mcu.packet[last] = inputs.mcu.packet[last].wrapping_sub(1);
    }
    let packet = prepared(resolved).unwrap();
    let packet_value = model(&packet, "serial_spi1")["routes"][packet.mcu.route]["endpoint"]
        ["options"]["registers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row[0] == packet.mcu.first_register)
        .unwrap()[1]
        .as_u64()
        .unwrap();
    assert_eq!(packet_value, 1);

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    exr2_inputs(&mut resolved).mcu.rtc.ticks_per_second = 24_000_000;
    exr2_inputs(&mut resolved).mcu.rtc.maximum_seconds = 999_999_999;
    exr2_inputs(&mut resolved).mcu.rtc.started = 17;
    let rtc = prepared(resolved).unwrap();
    let rtc_route = usize::from(
        rtc.resolved
            .reference
            .body
            .microcontroller
            .as_ref()
            .unwrap()
            .channel,
    );
    let clock = &model(&rtc, "serial_spi1")["routes"][rtc_route]["endpoint"]["options"]["clock"];
    assert_eq!(
        clock["initial_seconds"],
        fujimod_catalog_configuration::DEFAULT_CLOCK_SECONDS_2000
    );
    assert_eq!(clock["ticks_per_second"], 24_000_000);
    assert_eq!(clock["maximum_seconds"], 999_999_999);
    assert_eq!(clock["started"], 17);

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    exr2_inputs(&mut resolved)
        .gpio
        .keys
        .iter_mut()
        .find(|state| state.connection_id == "s1")
        .unwrap()
        .pressed = true;
    let pressed = prepared(resolved).unwrap();
    let s1 = pressed
        .resolved
        .reference
        .body
        .connections
        .iter()
        .find(|connection| connection.id == "s1")
        .unwrap();
    let (port, pin, active) = s1
        .signals
        .iter()
        .find_map(|signal| match signal {
            fujimod_catalog::reference::ConnectionSignal::Gpio {
                port,
                pin,
                active_level,
            } => Some((usize::from(*port), *pin, *active_level)),
            _ => None,
        })
        .unwrap();
    let input_level = model(&pressed, "serial_gpio")["options"]["input_levels"][port]
        .as_u64()
        .unwrap();
    assert_eq!(input_level & (1 << pin) != 0, active);
    assert_ne!(
        model(&base, "serial_gpio")["options"]["input_levels"][port],
        model(&pressed, "serial_gpio")["options"]["input_levels"][port]
    );

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    exr2_inputs(&mut resolved).gpio.outputs[0].high = false;
    let output = prepared(resolved).unwrap();
    assert_ne!(
        model(&base, "serial_gpio")["options"]["output_levels"],
        model(&output, "serial_gpio")["options"]["output_levels"]
    );

    let usb = prepare("x70-110-rec", &["x70-110-rec-usb".into()]).unwrap();
    assert_eq!(usb.devices_value["wires"]["usb_cable"]["logic-level"], true);

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    exr2_inputs(&mut resolved).encoders[0].phase = 3;
    let encoder = prepared(resolved).unwrap();
    assert_eq!(
        model(&encoder, "encoder_control_ring")["options"]["rest_phase"],
        3
    );

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    run_scene(&mut resolved).frame_period_ticks = 432_100;
    let scene = prepared(resolved).unwrap();
    assert_eq!(
        model(&scene, "live_frames")["options"]["period_ticks"],
        432_100
    );
    // PDAF extraction latency is a per-block hardware time, not a frame period.
    assert_eq!(
        model(&scene, "phase_detection")["options"]["data_ticks"],
        1_800
    );
    // RAW channel DMA latency is the measured transfer time, not a frame period.
    assert_eq!(
        model(&scene, "image_input")["options"]["raw_dma_ticks"],
        153_600
    );

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    exr2_inputs(&mut resolved)
        .gpio
        .connections
        .iter_mut()
        .find(|state| state.connection_id == "power")
        .unwrap()
        .connected = false;
    let power = prepared(resolved).unwrap();
    assert_ne!(
        model(&base, "serial_gpio")["options"]["input_levels"],
        model(&power, "serial_gpio")["options"]["input_levels"]
    );

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    exr2_inputs(&mut resolved).power.powered_on = false;
    assert!(
        prepared(resolved)
            .err()
            .unwrap()
            .contains("powered-off supply")
    );

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    resolved
        .configuration
        .execution
        .as_mut()
        .unwrap()
        .instructions_per_tick
        .numerator = 11;
    resolved
        .configuration
        .execution
        .as_mut()
        .unwrap()
        .instructions_per_tick
        .denominator = 3;
    let rate = prepared(resolved).unwrap();
    assert_eq!(
        rate.execution_value["instructions_per_tick"],
        serde_json::json!([11, 3])
    );

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    exr2_inputs(&mut resolved).power.boot_reply = 1;
    assert!(
        prepared(resolved)
            .err()
            .unwrap()
            .contains("boot reply zero")
    );

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    exr2_inputs(&mut resolved).power.spi_status.push(
        fujimod_catalog_configuration::RunRegisterValue {
            register: 9,
            value: 1,
        },
    );
    assert!(
        prepared(resolved)
            .err()
            .unwrap()
            .contains("SPI status inputs")
    );

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    exr2_inputs(&mut resolved).card.present = true;
    assert!(prepared(resolved).is_err());
}

/// Boots the same run through `open` and through a serialized preparation, then
/// compares retired instructions, guest time, the kernel image and the board
/// fingerprint. Requires the selected files: FUJIMOD_TEST_UPDATE (the update
/// container) and FUJIMOD_TEST_BOOT_ROM (the ROM dump).
#[test]
fn shipped_preparation_opens_the_same_board_as_selected_files() {
    let (Some(update), Some(boot_rom)) = (
        std::env::var_os("FUJIMOD_TEST_UPDATE"),
        std::env::var_os("FUJIMOD_TEST_BOOT_ROM"),
    ) else {
        eprintln!("skipped: set FUJIMOD_TEST_UPDATE and FUJIMOD_TEST_BOOT_ROM");
        return;
    };
    const CONFIGURATION: &str = "x70-110-rec";
    const INSTRUCTIONS: u64 = 5_000_000;
    // Kernel 0 occupies 0x40000..0x40000 + 0xdba0 in the mapped update image.
    const KERNEL0: (u64, usize) = (0x40000, 0xdba0);

    let quantum = prepare(CONFIGURATION, &[]).unwrap().quantum;
    let snapshot = |mut session: Session| {
        session
            .board
            .run_for_instructions(INSTRUCTIONS, quantum)
            .unwrap();
        let mut kernel = vec![0; KERNEL0.1];
        session.board.read_physical(KERNEL0.0, &mut kernel).unwrap();
        (
            session.definition.clone(),
            session.board.instruction_counts(),
            session.board.now(),
            Content::identify(&kernel).sha256,
            session.board.fingerprint().unwrap(),
        )
    };

    let direct = snapshot(
        open(Options {
            configuration_id: CONFIGURATION.into(),
            overlays: vec![],
            update: PathBuf::from(&update),
            boot_rom: PathBuf::from(&boot_rom),
            nand: None,
            settings: None,
            clock_seconds_2000: None,
            preferred_languages: vec![],
            camera: Some(reference_camera()),
        })
        .unwrap(),
    );
    assert!(direct.1.iter().sum::<u64>() >= INSTRUCTIONS);

    let document = prepare(CONFIGURATION, &[]).unwrap().to_json();
    let text = serde_json::to_string(&document).unwrap();
    let restored = Prepared::from_json(&serde_json::from_str(&text).unwrap()).unwrap();
    let inputs = InputBytes {
        update: std::fs::read(&update).unwrap(),
        boot_rom: std::fs::read(&boot_rom).unwrap(),
        nand: None,
        settings: None,
        clock_seconds_2000: None,
        preferred_languages: vec![],
        camera: camera_sections(&restored, &reference_camera()).unwrap(),
    };
    let shipped = snapshot(open_prepared(restored, inputs).unwrap());
    assert_eq!(shipped, direct);
}

#[test]
fn prepared_document_must_match_the_bundled_catalog_and_its_own_preparation() {
    let document = prepare("x70-110-rec", &[]).unwrap().to_json();
    assert!(Prepared::from_json(&document).is_ok());

    let mut moved = document.clone();
    moved["quantum"] = json!(1);
    assert!(
        Prepared::from_json(&moved)
            .err()
            .unwrap()
            .contains("/quantum")
    );

    let mut missing = document.clone();
    missing.as_object_mut().unwrap().remove("resolved");
    assert!(Prepared::from_json(&missing).is_err());

    let mut renamed = document;
    renamed["resolved"]["configuration"]["id"] = json!("x70-110-other");
    assert!(Prepared::from_json(&renamed).is_err());
}

#[test]
fn input_bytes_are_refused_before_any_board_when_they_differ_from_the_prepared_identity() {
    let prepared = prepare("x70-110-rec", &[]).unwrap();
    let inputs = InputBytes {
        update: vec![0; 4],
        boot_rom: vec![0; 4],
        nand: None,
        settings: None,
        clock_seconds_2000: None,
        preferred_languages: vec![],
        camera: BTreeMap::new(),
    };
    let error = open_prepared(prepared, inputs).err().unwrap();
    assert!(error.starts_with("update:"), "{error}");
}

#[test]
fn browser_camera_overlay_declares_a_virtual_card_host_on_the_native_plan() {
    let base = prepare("x70-110-rec", &[]).unwrap();
    assert!(
        base.devices_value["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["name"] != "sd"),
        "the base run has no card host"
    );

    let prepared = prepare("x70-110-rec", &["x70-110-browser-camera".into()]).unwrap();
    assert_eq!(
        model(&prepared, "avc_codec")["options"]["backend"],
        "software"
    );
    assert_eq!(
        prepared
            .resolved
            .configuration
            .preset
            .as_ref()
            .unwrap()
            .factory_options
            .video_encoder_backend,
        Some(fujimod_catalog_construction::VideoEncoderBackend::Software)
    );
    let text = serde_json::to_string(&prepared.to_json()).unwrap();
    assert!(Prepared::from_json(&serde_json::from_str(&text).unwrap()).is_ok());
    assert_eq!(prepared.resolved.configuration.start, RunStart::Rom);
    let sd = model(&prepared, "sd");
    assert_eq!(sd["kind"], "sd-host");
    assert_eq!(sd["options"]["protocol"], "sdhc");
    assert_eq!(sd["card"]["kind"], "formatted");
    assert_eq!(sd["card"]["options"]["sectors"], 16777216);
    assert_eq!(sd["card"]["options"]["label"], "CAMERA");
    assert_eq!(sd["card"]["options"]["partition_start"], 8192);
    assert_eq!(sd["card"]["options"]["sectors_per_cluster"], 64);
    assert_eq!(sd["card"]["options"]["max_dirty_sectors"], 1 << 20);
    assert_eq!(sd["card"]["files"], "virtual_card_files");
    assert_eq!(sd["pins"]["detect"], json!({"port": 12, "pin": 7}));
    assert_eq!(sd["pins"]["protect"], json!({"port": 12, "pin": 6}));
    assert_eq!(sd["present"], true);
    assert_eq!(sd["protected"], false);
    assert_eq!(sd["irq"]["Core"]["line"], 78);
    assert_eq!(prepared.scene["files"], json!({"virtual_card_files": {}}));
}

#[test]
fn browser_card_can_store_boot_files_and_a_raw_jpeg_pair() {
    use fujimod_emulator_devices::parts::sd_card::{Card, Options, SECTOR};

    let prepared = prepare("x70-110-rec", &["x70-110-browser-camera".into()]).unwrap();
    let options = &model(&prepared, "sd")["card"]["options"];
    let mut card = Card::new(Options {
        sectors: options["sectors"].as_u64().unwrap() as u32,
        max_dirty_sectors: options["max_dirty_sectors"].as_u64().unwrap() as usize,
    })
    .unwrap();
    // Boot media and captured photos occupy the same sparse sector budget.
    // Exercise real reservations and writes, including a full sensor RAW and
    // a 16 MiB JPEG, rather than only checking the configured numeric limit.
    let block = vec![0x46; 2048 * SECTOR];
    let mut sector = 8192;
    for bytes in [32 * 1024 * 1024usize, 33 * 1024 * 1024, 16 * 1024 * 1024] {
        let mut remaining = bytes.div_ceil(SECTOR);
        while remaining > 0 {
            let count = remaining.min(block.len() / SECTOR);
            card.reserve_write(sector, count as u32).unwrap();
            card.write_bytes(sector as u64 * SECTOR as u64, &block[..count * SECTOR])
                .unwrap();
            remaining -= count;
            sector += count as u32;
        }
    }
    assert_eq!(card.dirty_sectors(), sector as usize - 8192);
    assert_eq!(card.sector(sector - 1).unwrap(), &block[..SECTOR]);
}

#[test]
fn capture_chain_uses_catalog_windows_interrupts_and_sensor_phase() {
    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    let route = resolved
        .reference
        .soc
        .interrupts
        .iter_mut()
        .find(|route| route.source_block == "jpeg-encoder")
        .unwrap();
    route.line = 78;
    let prepared = prepared(resolved).unwrap();
    assert_eq!(model(&prepared, "jpeg_encoder")["irq"]["line"], 78);
    assert_eq!(
        model(&prepared, "still_processor")["options"]["cfa"],
        model(&prepared, "image_input")["options"]["cfa"]
    );
    assert_eq!(
        model(&prepared, "jpeg_encoder")["stream"],
        model(&prepared, "warp")["stream"]
    );
    for aperture in &prepared.execution.peripherals {
        assert_eq!(aperture.address % 4096, 0);
        assert_eq!(aperture.physical % 4096, 0);
        assert_eq!(aperture.size % 4096, 0);
    }
    let nodes = prepared.devices_value["nodes"].as_array().unwrap();
    let still = nodes
        .iter()
        .find(|node| node["name"] == "still_processor")
        .unwrap();
    let windows = still["windows"].as_array().unwrap();
    assert_eq!(windows.len(), 8);
    assert!(
        windows
            .iter()
            .any(|window| window["base"] == 0xfff44000u64 && window["size"] == 0x2080)
    );
    for node in nodes {
        for window in node["windows"].as_array().unwrap() {
            let base = window["base"].as_u64().unwrap();
            let end = base + window["size"].as_u64().unwrap();
            // The capture chain must not claim the unmapped gap after the prefilter.
            assert!(!(base <= 0xfff1a230 && end > 0xfff1a230));
        }
    }
}

#[test]
fn sensor_sync_acquisition_uses_the_declared_part_readouts() {
    let mut resolved = ResolvedConfiguration::with_overlays(
        "x70-110-rec", &["x70-110-browser-camera".into()],
    ).unwrap();
    let mut sensors = 0;
    for part in &mut resolved.reference.parts {
        if let fujimod_catalog::reference::PartModel::ImageSensor {
            radiometry: Some(radiometry),
            ..
        } = &mut part.model {
            radiometry.mode_change_frames = 4;
            sensors += 1;
        }
    }
    assert_eq!(sensors, 1);
    let prepared = prepared(resolved).unwrap();
    assert_eq!(
        model(&prepared, "image_input")["sensor"]["controls"]["options"]["mode_change_frames"],
        4
    );
}

#[test]
fn iris_motor_uses_the_body_mechanical_stops_beyond_the_optical_table() {
    let base = prepare("x70-110-rec", &[]).unwrap();
    let iris = &model(&base, "lens_serial")["routes"][0]["endpoint"]["options"]["motors"][1];
    assert_eq!(iris["travel_limits"], json!([0, 78]));
    assert_eq!(iris["boundary"], "end_stop");

    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    resolved.reference.body.lens.as_mut().unwrap().iris.as_mut().unwrap()
        .travel_limits = [0, 90];
    let changed = prepared(resolved).unwrap();
    assert_eq!(
        model(&changed, "lens_serial")["routes"][0]["endpoint"]["options"]["motors"][1]
            ["travel_limits"],
        json!([0, 90])
    );
}

#[test]
fn serial_spi1_refuses_a_shift_with_both_chip_selects_low() {
    use fujimod_emulator_devices::{Cx, Host, Record};
    struct Clock(u64);
    impl Host for Clock {
        fn now(&self) -> u64 {
            self.0
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {}
        fn set_irq(&mut self, _: u32, _: bool) {}
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            Err("no RAM".into())
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            Err("no RAM".into())
        }
        fn record(&mut self, _: Record) {}
    }
    const GPIO: u64 = 0xfff89000;
    const SPI1: u64 = 0xfff84040;
    let spec = prepare("x70-110-rec", &[]).unwrap();
    // Drive the panel select (GPIO11/3) and the shared register-endpoint
    // select (GPIO12/3), then shift a word in one endpoint's transport.
    let shift = |panel_high: bool, configuration: u64, word: u64| {
        let mut document = spec.devices_value.clone();
        document["nodes"].as_array_mut().unwrap().retain(|n| {
            ["irq0", "irq1", "serial_gpio", "serial_spi1"].contains(&n["name"].as_str().unwrap())
        });
        fujimod_emulator_exr2_board::register_board_builder();
        fujimod_emulator_models::register();
        let mut built = Plan::parse(&serde_json::to_vec(&document).unwrap())
            .unwrap()
            .build(fujimod_emulator_exr2_resources::Resources::default(), 0, &mut RefuseAdapters)
            .unwrap();
        let (gpio, spi) = (built.node_ids["serial_gpio"], built.node_ids["serial_spi1"]);
        let mut host = Clock(0);
        let mut access = |id: usize, address: u64, value: u64| {
            let mut cx = Cx::new(&mut host, 0, 0, id);
            built.graph.nodes[id]
                .device
                .write(&mut cx, address, 4, value);
            cx.take_refusal()
        };
        assert_eq!(
            access(gpio, GPIO + 11 * 0x40, 1 << 19 | u64::from(panel_high) << 3),
            None
        );
        assert_eq!(access(gpio, GPIO + 12 * 0x40, 1 << 19), None);
        assert_eq!(access(spi, SPI1, 0xe0), None);
        assert_eq!(access(spi, SPI1 + 4, configuration), None);
        access(spi, SPI1 + 0x1c, word)
    };
    // Calibrated-register write and MCU register read.
    for (configuration, word) in [(0x30004, 0x8000), (6, 0x4100)] {
        assert_eq!(shift(true, configuration, word), None);
        assert!(
            shift(false, configuration, word)
                .unwrap()
                .contains("no attached SPI route")
        );
    }
}

#[test]
fn video_encoder_backend_is_selected_in_the_prepared_graph() {
    use fujimod_catalog_construction::VideoEncoderBackend;
    let base = prepare("x70-110-rec", &[]).unwrap();
    assert_eq!(
        model(&base, "avc_codec")["options"]["backend"],
        "software"
    );
    for (backend, name) in [
        (VideoEncoderBackend::Software, "software"),
        (VideoEncoderBackend::Videotoolbox, "videotoolbox"),
    ] {
        let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
        resolved
            .configuration
            .preset
            .as_mut()
            .unwrap()
            .factory_options
            .video_encoder_backend = Some(backend);
        let run = prepared(resolved).unwrap();
        assert_eq!(model(&run, "avc_codec")["options"]["backend"], name);
        assert_eq!(
            model(&run, "avc_codec")["irq"],
            json!({"core":0,"line":75})
        );
        assert_eq!(
            run.plan
                .nodes
                .iter()
                .find(|n| n.name == "avc_codec")
                .unwrap()
                .windows
                .len(),
            2
        );
    }
}

#[test]
fn declared_pcm_input_selects_one_duplex_audio_channel_in_the_prepared_graph() {
    use fujimod_catalog_run_inputs::RunAudioInputProfile;
    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    resolved
        .configuration
        .preset
        .as_mut()
        .unwrap()
        .factory_options
        .audio_input_profile = Some(RunAudioInputProfile::DeclaredSilentStereoPcm48000);
    let run = prepared(resolved).unwrap();
    assert!(!run.plan.nodes.iter().any(|node| node.name == "audio_dma"));
    assert_eq!(
        model(&run, "audio_registers")["kind"],
        "serial-audio-duplex"
    );
    assert_eq!(
        model(&run, "audio_registers")["options"]["receive"]["sample_rate"],
        48000
    );
    assert_eq!(model(&run, "audio_registers")["options"]["receive"]["sample"], 0);
    assert_eq!(
        run.plan
            .nodes
            .iter()
            .find(|node| node.name == "audio_registers")
            .unwrap()
            .windows
            .len(),
        3
    );
}

#[test]
fn raw_readback_duration_is_an_explicit_prepared_input() {
    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    resolved
        .configuration
        .preset
        .as_mut()
        .unwrap()
        .factory_options
        .raw_readback_ticks = Some(100000);
    let run = prepared(resolved.clone()).unwrap();
    assert_eq!(
        model(&run, "image_input")["options"]["readback_ticks"],
        100000
    );
    resolved
        .configuration
        .preset
        .as_mut()
        .unwrap()
        .factory_options
        .raw_readback_ticks = Some(0);
    assert!(
        prepared(resolved)
            .err()
            .unwrap()
            .contains("positive virtual transfer ticks")
    );
}

#[test]
fn browser_graphs_select_portable_encoding_on_every_host() {
    use fujimod_catalog_construction::VideoEncoderBackend;
    for overlay in ["x70-110-browser-camera", "x70-110-browser-usb"] {
        let mut resolved =
            ResolvedConfiguration::with_overlays("x70-110-rec", &[overlay.into()]).unwrap();
        if resolved.configuration.browser_camera.is_some() {
            resolved
                .configuration
                .preset
                .as_mut()
                .unwrap()
                .factory_options
                .video_encoder_backend = Some(VideoEncoderBackend::Videotoolbox);
        }
        let run = prepared(resolved).unwrap();
        assert_eq!(
            model(&run, "avc_codec")["options"]["backend"],
            "software"
        );
    }
}

#[path = "tests/pwic.rs"]
mod pwic;

#[test]
fn statistics_dma_uses_its_selected_latency_instead_of_the_scene_frame_period() {
    // Owner report (2026-10-26): restarted X70 live view washes out before
    // AE recovers. A transfer longer than the sensor frame skips metering
    // samples and breaks the exposure command/readout pipeline's cadence.
    for (latency, frame) in [(1024, 400000), (321, 480000)] {
        let mut resolved =
            ResolvedConfiguration::with_overlays("x70-110-rec", &["x70-110-browser-camera".into()])
                .unwrap();
        resolved
            .configuration
            .preset
            .as_mut()
            .unwrap()
            .factory_options
            .producer_data_ticks = Some(latency);
        run_scene(&mut resolved).frame_period_ticks = frame;
        let run = prepared(resolved).unwrap();
        assert_eq!(
            model(&run, "statistics")["options"]["clock"]["ticks"],
            latency
        );
        assert_eq!(model(&run, "live_frames")["options"]["period_ticks"], frame);
    }
    let base = prepare("x70-110-rec", &[]).unwrap();
    assert_eq!(
        model(&base, "statistics")["options"]["clock"]["ticks"],
        400000
    );
}
