use fujimod_catalog_memory_reservations::FirmwareMemoryReservations;
use serde_json::json;

fn document(size: serde_json::Value, address: u64, length: u64) -> FirmwareMemoryReservations {
    serde_json::from_value(json!({
        "$schema": "https://fujimod.com/schemas/memory-reservations.generated.schema.json",
        "schema": 1,
        "id": "fixture-memory-reservations",
        "firmware_id": "fixture",
        "dram_size": size,
        "reservations": [{"name": "language_fonts", "address": address, "size": length}],
        "locator_failures": []
    }))
    .unwrap()
}

#[test]
fn unknown_size_accepts_only_valid_fixed_reservations() {
    let unknown = document(serde_json::Value::Null, 0x8e0000, 0x160000);
    assert_eq!(unknown.dram_size, None);
    assert!(unknown.validate().is_ok());
    let mut missing = serde_json::to_value(&unknown).unwrap();
    missing.as_object_mut().unwrap().remove("dram_size");
    assert!(serde_json::from_value::<FirmwareMemoryReservations>(missing).is_err());
    assert!(document(serde_json::Value::Null, u64::MAX, 1)
        .validate()
        .is_err());
    assert!(document(serde_json::Value::Null, 0x1000, 0)
        .validate()
        .is_err());
}

#[test]
fn known_size_still_bounds_reservations() {
    assert!(document(json!(0xb00000), 0x8e0000, 0x160000)
        .validate()
        .is_ok());
    assert!(document(json!(0x9fffff), 0x8e0000, 0x160000)
        .validate()
        .is_err());
    assert!(document(json!(0), 0, 1).validate().is_err());
}
