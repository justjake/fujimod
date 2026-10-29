use fujimod_catalog::Firmware;
use fujimod_catalog_run_inputs::HardwareInputs;
use std::{fs, path::PathBuf};

#[test]
fn firmware_layouts_and_body_inputs_decode_as_closed_records() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for body in fs::read_dir(root.join("catalog/firmware")).unwrap() {
        for version in fs::read_dir(body.unwrap().path()).unwrap() {
            let path = version.unwrap().path().join("firmware.json");
            let firmware: Firmware = serde_json::from_slice(&fs::read(&path).unwrap())
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            firmware
                .validate(&firmware.id)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        }
    }
    for entry in fs::read_dir(root.join("configurations/emulator")).unwrap() {
        let path = entry.unwrap().path().join("hardware-inputs.json");
        if path.exists() {
            let inputs: HardwareInputs = serde_json::from_slice(&fs::read(&path).unwrap())
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            assert_eq!(inputs.schema, 1);
            assert_eq!(
                inputs.schema_uri,
                fujimod_catalog_run_inputs::HARDWARE_INPUTS_SCHEMA_URI
            );
        }
    }
}

#[test]
fn xt1_rec_inputs_release_contacts_and_select_idle_positions() {
    let inputs = HardwareInputs::bundled("xt1-rec").unwrap();
    let reset = inputs.reset.inputs.unwrap();
    let gpio = reset.gpio.gpio_inputs;
    assert_eq!(gpio.get("0"), Some(&3630)); // GPIO0.1 grip absent; GPIO0.9 idle.
    assert_eq!(gpio.get("2"), Some(&4608)); // GPIO2.8 remains low.
    assert_eq!(gpio.get("3"), Some(&845)); // PLAY and DELETE released.
    assert_eq!(gpio.get("8"), Some(&24584)); // AF-LOCK released; strap retained.
    assert_eq!(gpio.get("12"), Some(&160)); // Idle input GPIO12.5 high.
    assert_eq!(reset.mcu_packet_hex, "00000000d00700ff008000040000aafc");
}
