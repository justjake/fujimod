#![cfg(target_arch = "wasm32")]
//! App80 executed as Wasm in a JS runtime, with no USB API.
use wasm_bindgen_test::wasm_bindgen_test;

#[wasm_bindgen_test]
fn selected_catalog_and_typed_replies_execute_in_wasm() {
    use fujimod_catalog_firmware_identity::FirmwareIdentity;
    use fujimod_catalog_service::{Definition, ServiceModes};
    use fujimod_usb_app80::operations::Service;
    use fujimod_usb_app80_types::{
        Reply,
        catalog::{Command, ReplyValue, SaveStatus},
    };
    let json = serde_json::json!({
        "schema": 1,
        "firmware": {"id": "synthetic-b-v2", "model": "Body B", "version": "2", "sha256": "ab".repeat(32)},
        "mailbox": {"page": 3, "offset": 576, "configuration_limit": 512},
        "entries": [{"code": 153, "label": "save", "codec": "flush_configuration", "slots": [7],
            "enabled_modes": 4, "next_mode": 0, "firmware_kind": 19, "handler": 4096, "parameter_table": 8192}]
    });
    let identity = FirmwareIdentity::new("synthetic-b-v2", &"ab".repeat(32)).unwrap();
    let definition = Definition::from_json(&json.to_string(), &identity).unwrap();
    let service = Service::new(definition).unwrap();
    let invocation = service
        .prepare(ServiceModes::new(4).unwrap(), Command::FlushConfiguration)
        .unwrap();
    assert_eq!(invocation.command().request().code().0, 153);
    let mut bytes = invocation.command().request().encode();
    assert_eq!(bytes[8], 255);
    bytes[8] = 1;
    let reply = invocation
        .command()
        .decode(Reply::decode(&bytes).unwrap())
        .unwrap();
    assert_eq!(
        reply.value(),
        &ReplyValue::ConfigurationSaved {
            status: SaveStatus::Failed
        }
    );
    assert!(
        service
            .prepare(ServiceModes::new(1).unwrap(), Command::FlushConfiguration)
            .is_err()
    );
}
