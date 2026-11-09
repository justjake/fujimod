use fujimod_catalog::ReferenceCatalog;
use fujimod_catalog_remote::{BuildMenuManifest, Facts, MENU_MANIFEST_SCHEMA_URI, Selection};

#[cfg(feature = "schema")]
#[test]
fn nullable_type_code_is_required_and_accepts_integer_or_null_in_schema() {
    let schema = serde_json::to_value(schemars::schema_for!(
        fujimod_catalog_remote::MenuFields
    ))
    .unwrap();
    let required = schema["required"].as_array().unwrap();
    assert!(required.iter().any(|name| name == "type_code"));

    let alternatives = schema["properties"]["type_code"]["anyOf"]
        .as_array()
        .expect("type_code schema should have integer and null alternatives");
    assert!(alternatives.iter().any(|item| item["type"] == "null"));
    assert!(alternatives.iter().any(|item| {
        item["type"] == "integer" || item.get("$ref").is_some()
    }));
}

#[cfg(feature = "schema")]
#[test]
fn observation_metadata_schemas_require_each_nullable_key() {
    fn required_nullable(schema: &serde_json::Value, name: &str) {
        assert!(schema["required"].as_array().unwrap().iter().any(|item| item == name));
        let alternatives = schema["properties"][name]["anyOf"]
            .as_array()
            .unwrap_or_else(|| panic!("{name} schema should admit explicit null"));
        assert!(alternatives.iter().any(|item| item["type"] == "null"));
        assert!(alternatives.iter().any(|item| {
            item["type"] == "integer" || item.get("$ref").is_some()
        }));
    }

    let cursor = serde_json::to_value(schemars::schema_for!(
        fujimod_catalog_remote::CursorFields
    ))
    .unwrap();
    for name in [
        "pointer",
        "subcursor",
        "visible_count",
        "value_cursor",
        "raw_count",
        "filtered_count",
    ] {
        required_nullable(&cursor, name);
    }
    assert!(cursor["required"].as_array().unwrap().iter().any(|item| item == "cursor"));
    assert_eq!(cursor["properties"]["cursor"]["type"], "integer");

    let scanout = serde_json::to_value(schemars::schema_for!(
        fujimod_catalog_remote::Scanout
    ))
    .unwrap();
    for name in ["address", "instance", "layer", "channel"] {
        required_nullable(&scanout, name);
    }

    let menus = serde_json::to_value(schemars::schema_for!(
        fujimod_catalog_remote::MenuTables
    ))
    .unwrap();
    assert!(menus["required"].as_array().unwrap().iter().any(|item| item == "q_grid"));
    let alternatives = menus["properties"]["q_grid"]["anyOf"]
        .as_array()
        .expect("q_grid schema should admit explicit null");
    assert!(alternatives.iter().any(|item| item["type"] == "null"));
    assert!(alternatives
        .iter()
        .any(|item| item["type"] == "object" || item.get("$ref").is_some()));
}

fn remote_selection<'a>(selection: &'a fujimod_catalog::ResolvedSelection) -> Selection<'a> {
    let generated = selection.firmware.generated_facts.as_ref();
    Selection {
        firmware_id: &selection.firmware.id,
        body_id: &selection.body.id,
        firmware_sha256: selection
            .firmware
            .update
            .as_ref()
            .map(|content| content.sha256.as_str()),
        remote_facts: generated.and_then(|facts| facts.remote.as_ref()),
        physical_controls: selection.physical_controls.as_ref(),
        control_ids: selection.control_ids.as_ref(),
        connections: selection
            .body
            .connections
            .iter()
            .map(|connection| connection.id.as_str())
            .collect(),
    }
}

fn selected_remote_facts() -> Facts {
    let selected = ReferenceCatalog::bundled()
        .unwrap()
        .resolve_firmware("x70-110")
        .unwrap();
    Facts::from_selection(&remote_selection(&selected), None).unwrap()
}

#[test]
fn observation_metadata_requires_explicit_null_and_preserves_numeric_values() {
    let original = selected_remote_facts();
    let base = serde_json::to_value(&original).unwrap();
    let cursor_names = [
        "pointer",
        "subcursor",
        "visible_count",
        "value_cursor",
        "raw_count",
        "filtered_count",
    ];
    for name in cursor_names {
        let mut explicit_null = base.clone();
        explicit_null["firmware"]["menus"]["cursor"]["fields"][name] =
            serde_json::Value::Null;
        let parsed: Facts = serde_json::from_value(explicit_null).unwrap();
        parsed.validate().unwrap();

        let mut missing = base.clone();
        missing["firmware"]["menus"]["cursor"]["fields"]
            .as_object_mut()
            .unwrap()
            .remove(name);
        let error = serde_json::from_value::<Facts>(missing).unwrap_err();
        assert!(error.to_string().contains(name), "{name}: {error}");
    }
    for name in ["address", "instance", "layer", "channel"] {
        let mut explicit_null = base.clone();
        explicit_null["firmware"]["scanout"][name] = serde_json::Value::Null;
        let parsed: Facts = serde_json::from_value(explicit_null).unwrap();
        parsed.validate().unwrap();

        let mut missing = base.clone();
        missing["firmware"]["scanout"]
            .as_object_mut()
            .unwrap()
            .remove(name);
        let error = serde_json::from_value::<Facts>(missing).unwrap_err();
        assert!(error.to_string().contains(name), "{name}: {error}");
    }

    let mut explicit_null = base.clone();
    explicit_null["firmware"]["menus"]["q_grid"] = serde_json::Value::Null;
    let parsed: Facts = serde_json::from_value(explicit_null).unwrap();
    parsed.validate().unwrap();

    let mut missing = base.clone();
    missing["firmware"]["menus"]
        .as_object_mut()
        .unwrap()
        .remove("q_grid");
    let error = serde_json::from_value::<Facts>(missing).unwrap_err();
    assert!(error.to_string().contains("q_grid"), "{error}");

    let parsed: Facts = serde_json::from_value(base).unwrap();
    assert_eq!(
        parsed.firmware.menus.cursor.fields.pointer,
        original.firmware.menus.cursor.fields.pointer
    );
    assert_eq!(
        parsed.firmware.scanout.address,
        original.firmware.scanout.address
    );
    assert_eq!(
        parsed.firmware.scanout.instance,
        original.firmware.scanout.instance
    );
    assert_eq!(parsed.firmware.menus.q_grid, original.firmware.menus.q_grid);
}

#[test]
fn cursor_offsets_still_validate_when_declared() {
    let facts = selected_remote_facts();
    let mut overlap = facts.clone();
    overlap.firmware.menus.cursor.fields.pointer =
        Some(overlap.firmware.menus.cursor.fields.cursor);
    assert!(
        overlap
            .validate()
            .unwrap_err()
            .contains("field offset exceeds")
    );

    let mut out_of_bounds = facts;
    out_of_bounds.firmware.menus.cursor.fields.pointer =
        Some(out_of_bounds.firmware.menus.cursor.size);
    assert!(
        out_of_bounds
            .validate()
            .unwrap_err()
            .contains("field offset exceeds")
    );
}

#[test]
fn quick_grid_dimensions_are_validated_when_present() {
    let facts = selected_remote_facts();
    for (rows, columns) in [(0, 2), (2, 0)] {
        let mut invalid = facts.clone();
        let grid = invalid.firmware.menus.q_grid.as_mut().unwrap();
        grid.rows = rows;
        grid.columns = columns;
        assert!(invalid.validate().is_err());
    }
    let mut unknown = facts;
    unknown.firmware.menus.q_grid = None;
    unknown.validate().unwrap();
}

#[test]
fn selected_firmware_supplies_complete_stock_remote_inputs() {
    let selected = ReferenceCatalog::bundled()
        .unwrap()
        .resolve_firmware("x70-110")
        .unwrap();
    let input = remote_selection(&selected);
    let facts = Facts::from_selection(&input, None).unwrap();
    assert_eq!(facts.firmware.menus.root_count, 188);
    assert_eq!(facts.firmware.menus.cursor.fields.cursor, 8);
    assert!(
        facts
            .control_ids
            .debug_keys
            .iter()
            .any(|key| key.name == "EXE_M")
    );
    assert!(facts.physical_controls.control("menu_ok_button").is_ok());
    assert!(facts.menu_manifest.is_none());
    let roundtrip: Facts = serde_json::from_value(serde_json::to_value(&facts).unwrap()).unwrap();
    assert_eq!(roundtrip, facts);
}

#[test]
fn mismatched_build_and_invalid_scanout_fail_before_use() {
    let selected = ReferenceCatalog::bundled()
        .unwrap()
        .resolve_firmware("x70-110")
        .unwrap();
    let input = remote_selection(&selected);
    let mut facts = Facts::from_selection(&input, None).unwrap();
    let mut value = serde_json::to_value(&facts).unwrap();
    value["firmware"]["scanout"]["stride_bytes"] = 1.into();
    assert!(
        serde_json::from_value::<Facts>(value)
            .unwrap()
            .validate()
            .is_err()
    );
    let manifest: BuildMenuManifest = serde_json::from_value(serde_json::json!({
        "$schema": MENU_MANIFEST_SCHEMA_URI, "schema": 1,
        "firmware_id": "different-100", "firmware_sha256": "a".repeat(64),
        "elf_sha256": "b".repeat(64), "artifacts": [], "items": [],
        "symbols": {}, "memory": [], "owner_constants": {},
        "walkthrough": { "menu": null, "qfn": null, "binding": null, "canvas": null,
            "choice_sets": {}, "binding_choices": {}, "canvas_hosts": {},
            "requirements": [], "option_requirements": [], "screen_requirements": [] }
    }))
    .unwrap();
    facts.menu_manifest = Some(manifest);
    assert!(facts.validate().unwrap_err().contains("different firmware"));
}

#[test]
fn xt20_profile_resolves_packed_observations_and_explicit_unknown_metadata() {
    use fujimod_catalog_remote::{PixelFormat, RootLayout};
    let selected = ReferenceCatalog::bundled().unwrap().resolve_firmware("xt20-211").unwrap();
    let facts = Facts::from_selection(&remote_selection(&selected), None).unwrap();
    let firmware = facts.firmware;
    assert_eq!(firmware.body_id, "xt20");
    assert_eq!(firmware.state.widths.ui_mode, 1);
    assert_eq!(firmware.state.widths.menu_active, 1);
    assert_eq!(firmware.state.readiness.input_pending_width, 1);
    assert_eq!(firmware.state.readiness.override_width, 1);
    assert!(matches!(firmware.menus.root_layout, RootLayout::KeyedEntries {
        stride: 8, key_offset: 0, key_width: 1, pointer_offset: 4
    }));
    assert_eq!(firmware.menus.layout.field_widths.menu_id, 1);
    assert_eq!(firmware.menus.layout.fields.type_code, None);
    assert_eq!(firmware.menus.cursor.status_width, 1);
    assert_eq!(firmware.menus.cursor.status_stride, 1);
    assert!(firmware.menus.q_grid.is_none());
    assert!(firmware.scanout.instance.is_none());
    assert_eq!(firmware.scanout.pixel_format, PixelFormat::Rgba8888Premultiplied);
}
