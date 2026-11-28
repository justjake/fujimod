use fujimod_catalog::{Body, ReferenceCatalog, Soc};
use std::{fs, path::PathBuf};

#[test]
fn declared_hardware_profiles_decode_and_validate() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../catalog");
    let mut hardware_bodies = 0;
    let mut manual_only_bodies = 0;
    for entry in fs::read_dir(root.join("bodies")).unwrap() {
        let directory = entry.unwrap().path();
        let path = directory.join("body.json");
        if path.is_file() {
            let body: Body = serde_json::from_slice(&fs::read(&path).unwrap())
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            body.validate(&body.id)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            hardware_bodies += 1;
        } else {
            let path = directory.join("controls.json");
            let controls: fujimod_catalog_controls::PhysicalControls =
                serde_json::from_slice(&fs::read(&path).unwrap())
                    .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            controls.validate().unwrap();
            assert_eq!(
                directory.file_name().unwrap().to_str().unwrap(),
                controls.body_id
            );
            manual_only_bodies += 1;
        }
    }
    assert!(hardware_bodies > 0 && manual_only_bodies > 0);
    let catalog = ReferenceCatalog::load(&root).expect("catalog loads");
    for id in ["exr2", "xp4", "xp5", "xpp"] {
        assert!(catalog.soc(id).is_some(), "{id} loads with its blocks");
    }
}

#[test]
fn hardware_graph_requires_closed_nodes_and_complete_ordering() {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../../../../catalog/bodies/xm1/body.json")).unwrap();
    let graph = &mut value["hardware"]["board"]["graph"];
    graph["devices"][0]["kind"] = "invented-device".into();
    assert!(serde_json::from_value::<Body>(value.clone()).is_err());
    let graph = &mut value["hardware"]["board"]["graph"];
    graph["devices"][0]["kind"] = "BootControls".into();
    graph["attachment_order"].as_array_mut().unwrap().pop();
    let body: Body = serde_json::from_value(value).unwrap();
    assert!(body.validate(&body.id).unwrap_err().contains("order"));
}

#[test]
fn x70_board_connects_window_analysis_completion() {
    let value: serde_json::Value =
        serde_json::from_str(include_str!("../../../../catalog/bodies/x70/body.json")).unwrap();
    let board = &value["hardware"]["board"];
    let devices = board["graph"]["devices"].as_array().unwrap();
    assert_eq!(
        devices
            .iter()
            .filter(|d| d["kind"] == "WindowAnalysisControl")
            .count(),
        1,
        "the board must complete window-analysis requests"
    );
    assert!(
        !devices
            .iter()
            .any(|d| d["kind"] == "RawConversionRegisters")
    );
    assert_eq!(
        board["controllers"]["window_analysis"]["ticks_per_window"],
        1000
    );
}

#[test]
fn a_construction_node_input_is_checked_with_its_body() {
    let mut body = body("xe2");
    body.validate("xe2").unwrap();
    let board = body.hardware.as_mut().unwrap().board.as_mut().unwrap();
    let Some(fujimod_catalog_run_inputs::RunNativeDeviceInputs::Adc(adc)) =
        board.native_device_inputs.get_mut("adc")
    else {
        panic!("ADC input");
    };
    adc.samples[0] = 1024;
    assert!(body.validate("xe2").is_err());
}

#[test]
fn resting_inputs_declare_no_session_choices() {
    let mut body = body("x70");
    body.validate("x70").unwrap();
    let Some(fujimod_catalog_run_inputs::RunInputs::Exr2(inputs)) =
        body.hardware.as_mut().unwrap().inputs.as_mut()
    else {
        panic!("X70 resting inputs");
    };
    inputs.usb.connected = true;
    assert!(
        body.validate("x70")
            .unwrap_err()
            .contains("camera description")
    );
}

fn body(id: &str) -> Body {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../catalog/bodies")
        .join(id)
        .join("body.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn xp5_bodies_declare_their_own_bus_drain() {
    let drain = |id: &str| {
        let body = body(id);
        let drain = body.hardware.unwrap().bus_drain.into_iter().next().unwrap();
        (drain.address, drain.processor_ids)
    };
    let cores: Vec<String> = (0..4).map(|n| format!("core{n}")).collect();
    assert_eq!(drain("x100vi"), (0x6800_0000, cores.clone()));
    assert_eq!(drain("xt5"), (0x6000_0000, cores.clone()));
    assert_eq!(drain("xh2s"), (0x8000_0000, cores));
}

#[test]
fn bus_drain_must_be_aligned_and_name_processors() {
    let mut body = body("xt5");
    let drain = &mut body.hardware.as_mut().unwrap().bus_drain[0];
    drain.address += 2;
    assert!(body.validate("xt5").is_err());
    let mut body = self::body("xt5");
    let drain = &mut body.hardware.as_mut().unwrap().bus_drain[0];
    drain.processor_ids.clear();
    assert!(body.validate("xt5").is_err());
    let mut body = self::body("xt5");
    let drain = &mut body.hardware.as_mut().unwrap().bus_drain[0];
    drain.processor_ids = vec!["core0".into(), "core0".into()];
    assert!(body.validate("xt5").is_err());
}

#[test]
fn card_slots_and_programming_modes_name_declared_connections() {
    body("xe5").validate("xe5").unwrap();
    let mut body = self::body("xe5");
    let slots = &mut body.hardware.as_mut().unwrap().card_slots;
    slots[0].detect_connection_id = "missing".into();
    assert!(body.validate("xe5").unwrap_err().contains("missing"));
    let mut body = self::body("xe5");
    let slots = &mut body.hardware.as_mut().unwrap().card_slots;
    slots.push(slots[0].clone());
    assert!(body.validate("xe5").unwrap_err().contains("two card slots"));
    let mode = Some("power-controller-programming-mode".to_string());
    for (mode, bytes) in [
        (mode.clone(), None),
        (None, Some(0x1_0000)),
        (mode, Some(0)),
        (Some("missing".to_string()), Some(0x1_0000)),
    ] {
        let mut body = self::body("xe5");
        let attachment = body
            .hardware
            .as_mut()
            .unwrap()
            .serial_attachments
            .iter_mut()
            .find(|attachment| attachment.programming_mode_connection_id.is_some())
            .unwrap();
        attachment.programming_mode_connection_id = mode;
        attachment.programming_flash_bytes = bytes;
        assert!(body.validate("xe5").is_err());
    }
}

#[test]
fn xp4_display_channels_declare_the_same_plane_layouts() {
    let catalog = ReferenceCatalog::bundled().expect("bundled catalog loads");
    let soc: &Soc = catalog.soc("xp4").expect("xp4 record");
    soc.validate(&soc.id).unwrap();
    let block = |id: &str| match soc
        .blocks
        .iter()
        .find(|block| block.id == id)
        .and_then(|block| block.device.as_ref())
    {
        Some(fujimod_catalog::soc::BlockDevice::Display {
            picture_layout,
            video_layout,
            ..
        }) => (
            serde_json::to_value(picture_layout).unwrap(),
            serde_json::to_value(video_layout).unwrap(),
        ),
        other => panic!("{id} is not a declared display block: {other:?}"),
    };
    let lcdc0 = block("lcdc0");
    assert_eq!(
        lcdc0,
        block("lcdc1"),
        "XP4 retimes either display channel with the same plane packings"
    );
}
