use fujimod_catalog::Part;
use std::{fs, path::PathBuf};

#[test]
fn reusable_part_assemblies_are_closed_and_validated() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../catalog/parts");
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path().join("part.json");
        let value = fs::read(&path).unwrap();
        let part: Part = serde_json::from_slice(&value)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        part.validate(&part.id)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    }
}

#[test]
fn lens_counter_and_pad_errors_are_rejected() {
    let mut value: serde_json::Value = serde_json::from_str(include_str!(
        "../../../catalog/parts/banked-programmed-lens/part.json"
    ))
    .unwrap();
    value["model"]["motor_counters"][0]["motor"] = 2.into();
    let part: Part = serde_json::from_value(value.clone()).unwrap();
    assert!(part.validate(&part.id).unwrap_err().contains("counter"));
    value["model"]["motor_counters"][0]["motor"] = 0.into();
    value["attachment"]["motor_home_pins"][0]["pin"] = 16.into();
    let part: Part = serde_json::from_value(value.clone()).unwrap();
    assert!(part.validate(&part.id).unwrap_err().contains("GPIO"));
    value["attachment"]["invented"] = true.into();
    assert!(serde_json::from_value::<Part>(value).is_err());
}
