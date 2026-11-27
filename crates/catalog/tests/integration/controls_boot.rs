use fujimod_catalog::ReferenceCatalog;
use fujimod_catalog_controls::BootMode;
use std::path::PathBuf;

#[test]
fn confirmed_combinations_load_from_controls() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../catalog");
    for catalog in [
        ReferenceCatalog::load(root).unwrap(),
        ReferenceCatalog::bundled().unwrap(),
    ] {
        let x70 = catalog
            .resolve_firmware("x70-110")
            .unwrap()
            .physical_controls
            .unwrap();
        let x70 = fujimod_catalog_controls::PhysicalControls::decode_document(&x70, "x70").unwrap();
        assert_eq!(x70.boot[&BootMode::Service].usb, Some(true));
        assert_eq!(
            x70.boot[&BootMode::Service].hold[1].stage.as_deref(),
            Some("full")
        );
        let xt1 = catalog.resolve_firmware("xt1-552").unwrap();
        let xt1_controls = xt1.physical_controls.as_ref().unwrap();
        let xt1_boot =
            fujimod_catalog_controls::PhysicalControls::decode_document(xt1_controls, "xt1")
                .unwrap();
        assert_eq!(
            xt1_boot.boot[&BootMode::Sd]
                .hold
                .iter()
                .map(|input| input.id.as_str())
                .collect::<Vec<_>>(),
            ["ae_lock_button", "fn1_button"]
        );
        // The physical front Fn1 input has the internal event name FUNCTION2.
        assert_eq!(
            xt1_controls
                .pointer("/controls/fn1_button/location")
                .and_then(|v| v.as_str()),
            Some("front")
        );
        assert_eq!(
            xt1_controls
                .pointer("/controls/fn1_button/connection_ids/0")
                .and_then(|v| v.as_str()),
            Some("controls-gpio-0-11")
        );
        let key = xt1.control_ids.as_ref().unwrap()["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == "row-19")
            .unwrap();
        assert_eq!(key["event"], 0x224);
        assert_eq!(key["target"]["control_id"], "fn1_button");
        let xm5 = catalog
            .resolve_firmware("xm5-130")
            .unwrap()
            .physical_controls
            .unwrap();
        let xm5 = fujimod_catalog_controls::PhysicalControls::decode_document(&xm5, "xm5").unwrap();
        assert_eq!(
            xm5.boot[&BootMode::Sd]
                .hold
                .iter()
                .map(|input| input.id.as_str())
                .collect::<Vec<_>>(),
            ["quick_menu_button", "front_command_dial.press"]
        );
    }
}

#[test]
fn startup_controls_and_shutter_stages_are_checked() {
    let catalog = ReferenceCatalog::bundled().unwrap();
    let original = catalog
        .resolve_firmware("x70-110")
        .unwrap()
        .physical_controls
        .unwrap();
    let original =
        fujimod_catalog_controls::PhysicalControls::decode_document(&original, "x70").unwrap();
    let mut controls = original.clone();
    controls.boot.get_mut(&BootMode::Sd).unwrap().hold[0].id = "missing".into();
    assert!(controls.validate().is_err());
    let mut controls = original.clone();
    controls.boot.get_mut(&BootMode::Service).unwrap().hold[1].stage = Some("missing".into());
    assert!(controls.validate().is_err());
}

#[test]
fn every_body_has_valid_markings_and_sd_update_entry() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../catalog/bodies");
    let mut count = 0;
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path().join("controls.json");
        if !path.is_file() {
            continue;
        }
        let controls: fujimod_catalog_controls::PhysicalControls =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        controls
            .validate()
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        if controls.boot.contains_key(&BootMode::Sd) {
            assert!(controls.boot.contains_key(&BootMode::FirmwareUpdate));
        }
        count += 1;
    }
    assert!(count >= 49);
}

#[test]
fn markings_and_sd_update_requirements_refuse_invalid_facts() {
    let original: fujimod_catalog_controls::PhysicalControls =
        serde_json::from_str(include_str!("../../../../catalog/bodies/x70/controls.json")).unwrap();
    for invalid in [
        "",
        "elevenchars",
        "ABCDEFGHIJK",
        "lower",
        " CMD",
        "CMD/REAR",
    ] {
        let mut controls = original.clone();
        controls
            .controls
            .get_mut("command_control")
            .unwrap()
            .short_name = Some(invalid.into());
        assert!(controls.validate().is_err(), "{invalid:?}");
    }
    let mut controls = original.clone();
    controls
        .controls
        .get_mut("command_control")
        .unwrap()
        .short_name = None;
    assert!(controls.validate().is_err());
    let mut controls = original.clone();
    controls.controls.get_mut("disp_back_button").unwrap().face = Some("DISP//BACK".into());
    assert!(controls.validate().is_err());
    let mut controls = original.clone();
    controls.boot.remove(&BootMode::FirmwareUpdate);
    assert!(controls.validate().is_err());
    let mut value = serde_json::to_value(original).unwrap();
    value["controls"]["movie_record_button"]["icon"] = serde_json::json!("movie recording");
    assert!(serde_json::from_value::<fujimod_catalog_controls::PhysicalControls>(value).is_err());
}
