//! A declared write range is an explicit construction input, not a fallback
//! from whole-array write protection. The fixture asserts the exact policy it
//! selected so an omitted option cannot silently leave the array unrestricted.
mod support;
use fujimod_emulator_xp4_board::Definition;
use serde_json::{json, Value};

const FLASH_SIZE_RANGE: (u64, u64) = (0x8000, 0x2000);

fn definition(ranges: Value) -> Value {
    json!({
        "ram_size": 65536,
        "cores": 1,
        "discovery": false,
        "cbar": 0xffff0000u32,
        "storage_jedec": 0x20,
        "storage_dma_routes": (32..48).collect::<Vec<_>>(),
        "storage_geometry": [4096, 256, 10, 2],
        "storage_irq": 48,
        "storage_strict_backing": true,
        "storage_write_protect": false,
        "storage_erased_ranges": [{"offset": FLASH_SIZE_RANGE.0, "length": FLASH_SIZE_RANGE.1}],
        "storage_write_ranges": ranges,
        "gpio": vec![0u16; 16],
    })
}

fn session(ranges: Value) -> Result<support::Board, String> {
    support::open_with(
        serde_json::from_value::<Definition>(definition(ranges)).unwrap(),
        Some(vec![0xff; 0x1000]),
        Vec::new(),
        Vec::new(),
    )
}

#[test]
fn declared_write_range_is_exact_and_nonempty_at_construction() {
    let s = session(json!([{"offset": FLASH_SIZE_RANGE.0, "length": FLASH_SIZE_RANGE.1}]))
        .unwrap();
    let state: Value =
        s.device_state_json()["xp4-serial-flash"].clone();
    assert_eq!(state["nor"]["write_protect"], json!(false));
    assert_eq!(
        state["nor"]["write_ranges"],
        json!([{"offset": FLASH_SIZE_RANGE.0, "length": FLASH_SIZE_RANGE.1}])
    );
    assert_eq!(state["nor"]["write_ranges"].as_array().unwrap().len(), 1);
}

#[test]
fn write_ranges_and_whole_array_protection_are_contradictory() {
    let mut d = definition(json!([{"offset": FLASH_SIZE_RANGE.0, "length": FLASH_SIZE_RANGE.1}]));
    d["storage_write_protect"] = json!(true);
    assert!(support::open_with(serde_json::from_value::<Definition>(d).unwrap(), Some(vec![0xff; 0x1000]), Vec::new(), Vec::new())
    .is_err());
}

#[test]
fn write_ranges_require_strict_backing_and_an_explicit_image() {
    let mut d = definition(json!([{"offset": FLASH_SIZE_RANGE.0, "length": FLASH_SIZE_RANGE.1}]));
    d["storage_strict_backing"] = json!(false);
    assert!(support::open_with(serde_json::from_value::<Definition>(d).unwrap(), Some(vec![0xff; 0x1000]), Vec::new(), Vec::new())
    .is_err());
    let d = definition(json!([{"offset": FLASH_SIZE_RANGE.0, "length": FLASH_SIZE_RANGE.1}]));
    assert!(support::open(serde_json::from_value::<Definition>(d).unwrap()).is_err());
}
