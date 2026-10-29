use fujimod_catalog::{Body, Soc};
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
    for entry in fs::read_dir(root.join("soc")).unwrap() {
        let path = entry.unwrap().path().join("soc.json");
        let soc: Soc = serde_json::from_slice(&fs::read(&path).unwrap())
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        soc.validate(&soc.id)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    }
}

#[test]
fn hardware_graph_requires_closed_nodes_and_complete_ordering() {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../../../catalog/bodies/xm1/body.json")).unwrap();
    let graph = &mut value["hardware"]["profiles"]["xm1-rec"]["graph"];
    graph["devices"][0]["kind"] = "invented-device".into();
    assert!(serde_json::from_value::<Body>(value.clone()).is_err());
    let graph = &mut value["hardware"]["profiles"]["xm1-rec"]["graph"];
    graph["devices"][0]["kind"] = "BootControls".into();
    graph["attachment_order"].as_array_mut().unwrap().pop();
    let body: Body = serde_json::from_value(value).unwrap();
    assert!(body.validate(&body.id).unwrap_err().contains("order"));
}

#[test]
fn x70_loader_profiles_connect_window_analysis_completion() {
    let value: serde_json::Value =
        serde_json::from_str(include_str!("../../../catalog/bodies/x70/body.json")).unwrap();
    for name in ["x70", "x70-rec-usb", "x70-110-browser-usb"] {
        let profile = &value["hardware"]["profiles"][name];
        let devices = profile["graph"]["devices"].as_array().unwrap();
        assert_eq!(
            devices
                .iter()
                .filter(|d| d["kind"] == "WindowAnalysisControl")
                .count(),
            1,
            "{name} must complete window-analysis requests"
        );
        assert!(
            !devices
                .iter()
                .any(|d| d["kind"] == "RawConversionRegisters")
        );
        assert_eq!(
            profile["controllers"]["window_analysis"]["ticks_per_window"],
            1000
        );
    }
}
