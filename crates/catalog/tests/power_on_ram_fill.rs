use fujimod_catalog::provenance::EvidenceKind;
use fujimod_catalog::reference::MemoryKind;
use fujimod_catalog::{Body, ReferenceCatalog, Soc};
use std::{fs, path::PathBuf};

fn catalog_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../catalog")
}

#[test]
fn exr2_records_the_measured_x70_fill_on_its_dram_region() {
    let catalog = ReferenceCatalog::bundled().expect("bundled catalog loads");
    let soc = catalog.soc("exr2").expect("exr2 record");
    let region = soc
        .memory_regions
        .iter()
        .find(|region| region.kind == MemoryKind::Dram)
        .expect("exr2 declares a DRAM region");
    let fill = region
        .power_on_ram_fill
        .as_ref()
        .expect("exr2 DRAM records a power-on fill");
    assert_eq!(fill.byte, 0xff);
    assert_eq!(fill.evidence.kind, EvidenceKind::Measured);
    assert_eq!(fill.evidence.firmware_ids, ["x70-110"]);
    assert_eq!(soc.power_on_ram_fill(), Some(0xff));
}

#[test]
fn a_body_records_only_its_own_different_measurement() {
    let path = catalog_root().join("bodies/x70/body.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("x70 body")).expect("x70 body JSON");
    assert!(value.get("power_on_ram_fill").is_none());
    value["power_on_ram_fill"] = serde_json::json!({
        "byte": 0,
        "evidence": {"kind": "measured", "firmware_ids": ["x70-110"]},
    });
    let body: Body = serde_json::from_value(value).expect("body with a measured fill");
    body.validate(&body.id).expect("measured body fill validates");
    assert_eq!(body.power_on_ram_fill.as_ref().unwrap().byte, 0);
}

#[test]
fn a_fill_outside_dram_is_refused() {
    let catalog = ReferenceCatalog::bundled().expect("bundled catalog loads");
    let mut value = serde_json::to_value(catalog.soc("exr2").expect("exr2 record"))
        .expect("serialize exr2");
    let fill = value["memory_regions"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|region| region["kind"] == "dram")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("power_on_ram_fill")
        .expect("the DRAM fill");
    let rom = value["memory_regions"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|region| region["kind"] == "rom")
        .unwrap();
    rom["power_on_ram_fill"] = fill;
    let soc: Soc = serde_json::from_value(value).expect("soc with a ROM fill");
    let error = soc.validate(&soc.id).expect_err("a ROM fill is refused");
    assert!(error.contains("outside DRAM"), "{error}");
}
