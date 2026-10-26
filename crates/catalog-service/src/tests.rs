use super::*;
fn identity(id: &str) -> FirmwareIdentity {
    FirmwareIdentity::new(id, &"ab".repeat(32)).unwrap()
}
fn document() -> serde_json::Value {
    serde_json::json!({"schema":1,"firmware":{"id":"body-a-v1","model":"Body A","version":"1","sha256":"ab".repeat(32)},
    "mailbox":{"page":2,"offset":384,"configuration_limit":256},"entries":[
      {"code":0,"label":"write pins","codec":"write_pins","slots":[0,1,2,3,4],"enabled_modes":1,"next_mode":4,"firmware_kind":1,"handler":4096,"parameter_table":8192},
      {"code":0,"label":"other mode","codec":"research","slots":[],"enabled_modes":4,"next_mode":0,"firmware_kind":9,"handler":4294967296u64,"parameter_table":0},
      {"code":2,"label":"disabled","codec":"research","slots":[],"enabled_modes":0,"next_mode":0,"firmware_kind":0,"handler":42,"parameter_table":0}
    ]})
}
fn parse(value: &serde_json::Value) -> Result<Definition, DefinitionError> {
    Definition::from_json(&value.to_string(), &identity("body-a-v1"))
}
#[test]
fn preserves_table_order_duplicate_codes_disabled_rows_and_unknown_flags() {
    let d = parse(&document()).unwrap();
    assert_eq!(d.entries().len(), 3);
    let first = d.select(0, ServiceModes::new(1).unwrap()).unwrap();
    assert_eq!(first.codec(), Codec::WritePins);
    assert_eq!(
        first.transition(),
        ModeTransition::Set(ServiceModes::new(4).unwrap())
    );
    let other = d.select(0, ServiceModes::new(4).unwrap()).unwrap();
    assert_eq!(other.firmware_kind(), 9);
    assert_eq!(other.handler(), 4294967296);
    assert!(d.select(2, ServiceModes::new(u32::MAX).unwrap()).is_none());
    assert_eq!(
        d.select(0, ServiceModes::new(5).unwrap()).unwrap().label(),
        "write pins"
    );
    assert_eq!(d.mailbox().page(), 2);
}
#[test]
fn identity_is_explicit_and_never_falls_back_to_another_body() {
    let raw = document().to_string();
    assert!(matches!(
        Definition::from_json(&raw, &identity("body-b-v1")),
        Err(DefinitionError::Identity(IdentityError::FirmwareMismatch))
    ));
    let wrong = FirmwareIdentity::new("body-a-v1", &"cd".repeat(32)).unwrap();
    assert!(matches!(
        Definition::from_json(&raw, &wrong),
        Err(DefinitionError::Identity(IdentityError::FirmwareMismatch))
    ));
    assert!(FirmwareIdentity::new("", &"ab".repeat(32)).is_err());
    for digest in ["a".repeat(63), "z".repeat(64), "é".repeat(32)] {
        assert!(Digest::parse(&digest).is_err());
    }
    assert_eq!(
        Digest::parse(&"AB".repeat(32)).unwrap().hex(),
        "ab".repeat(32)
    );
}
#[test]
fn invalid_layouts_and_schema_are_rejected_before_any_protocol_use() {
    for slots in [vec![0, 1], vec![0, 1, 2, 3, 32], vec![0, 1, 2, 3, 3]] {
        let mut raw = document();
        raw["entries"][0]["slots"] = serde_json::json!(slots);
        assert!(matches!(
            parse(&raw),
            Err(DefinitionError::InvalidLayout { .. })
        ));
    }
    for (path, value) in [
        ("schema", serde_json::json!(2)),
        ("unknown", serde_json::json!(1)),
    ] {
        let mut raw = document();
        raw[path] = value;
        assert!(parse(&raw).is_err());
    }
    let mut raw = document();
    raw["entries"][0]["codec"] = serde_json::json!("typo");
    assert!(parse(&raw).is_err());
    raw = document();
    raw["entries"][0]["code"] = serde_json::json!(256);
    assert!(parse(&raw).is_err());
    assert!(ServiceModes::new(0).is_err());
}
#[test]
fn invalid_mailbox_quantities_are_not_silently_truncated() {
    for (field, value) in [
        ("configuration_limit", 32u64),
        ("configuration_limit", 0),
        ("offset", u32::MAX as u64),
        ("offset", 1u64 << 32),
    ] {
        let mut raw = document();
        raw["mailbox"][field] = serde_json::json!(value);
        assert!(parse(&raw).is_err());
    }
}

#[test]
fn unreviewed_mode_rules_are_visible_but_cannot_enable_execution() {
    let mut raw = document();
    raw["entries"][0]["enabled_modes"] = serde_json::Value::Null;
    let d = parse(&raw).unwrap();
    assert_eq!(d.entries()[0].enabled_modes(), ModeAvailability::Unreviewed);
    assert!(d.select(0, ServiceModes::new(1).unwrap()).is_none());
}
#[test]
fn provenance_count_must_cover_the_loaded_records() {
    let mut raw = document();
    raw["source"] = serde_json::json!({"format":"dispatch24","records":2,"table":4096,"table_sha256":"ab".repeat(32),"dispatcher":8192});
    assert!(matches!(parse(&raw), Err(DefinitionError::SourceCount)));
}
