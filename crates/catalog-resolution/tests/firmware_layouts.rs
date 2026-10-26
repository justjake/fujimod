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
