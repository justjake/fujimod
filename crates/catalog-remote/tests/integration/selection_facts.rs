use fujimod_catalog::ReferenceCatalog;
use fujimod_catalog_remote::{
    BuildMenuManifest, Facts, MENU_MANIFEST_SCHEMA_URI, Selection, WalkthroughMetadata,
};

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
fn action_choice_owners_are_required_and_roundtrip_by_action_id() {
    let mut metadata = serde_json::json!({
        "menu": null,
        "qfn": null,
        "binding": null,
        "canvas": null,
        "choice_sets": {},
        "binding_choices": {},
        "qfn_action_bindings": [],
        "canvas_hosts": {},
        "requirements": [],
        "option_requirements": [],
        "screen_requirements": []
    });
    assert!(serde_json::from_value::<WalkthroughMetadata>(metadata.clone()).is_err());
    metadata["action_choice_owners"] = serde_json::json!({});
    let parsed: WalkthroughMetadata = serde_json::from_value(metadata.clone()).unwrap();
    assert!(parsed.action_choice_owners.is_empty());

    metadata["action_choice_owners"] = serde_json::json!({"42": 0x1000_0001u64});
    let parsed: WalkthroughMetadata = serde_json::from_value(metadata).unwrap();
    assert_eq!(parsed.action_choice_owners.get(&42), Some(&0x1000_0001u64));
    let roundtrip: WalkthroughMetadata =
        serde_json::from_value(serde_json::to_value(&parsed).unwrap()).unwrap();
    assert_eq!(roundtrip, parsed);
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
        "menu_tree": {
            "schema": 1,
            "nodes": [
                {"name": "setup", "identity": "lib-setup/setup", "owner": "lib-setup",
                 "id": 12288, "kind": "PAGE", "label_text": "SET-UP", "parent": null,
                 "order": 0, "action": "", "target": ""},
                {"name": "setup.bug_report_shortcut",
                 "identity": "bug-report/setup.bug_report_shortcut", "owner": "bug-report",
                 "id": 12289, "kind": "VALUE", "label_text": "BUG REPORT SHORTCUT",
                 "parent": "setup", "order": 0, "action": "bug_report_shortcut", "target": ""}
            ],
            "attachments": [{"root": "setup", "anchor": "setup", "place": "in",
                             "order": 0, "record": ""}]
        },
        "symbols": {}, "memory": [], "owner_constants": {},
        "walkthrough": { "menu": null, "qfn": null, "binding": null, "canvas": null,
            "choice_sets": {}, "action_choice_owners": {}, "binding_choices": {},
            "qfn_action_bindings": [], "canvas_hosts": {},
            "requirements": [], "option_requirements": [], "screen_requirements": [] }
    }))
    .unwrap();
    facts.menu_manifest = Some(manifest);
    assert!(facts.validate().unwrap_err().contains("different firmware"));
}
