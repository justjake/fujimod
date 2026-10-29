use super::*;
use serde_json::{Value, json};

fn identity() -> FirmwareIdentity {
    FirmwareIdentity::new("body-100", &"a".repeat(64)).unwrap()
}
fn document() -> Value {
    json!({"$schema":SCHEMA_URI,"schema":1,"firmware_id":"body-100","firmware_sha256":"a".repeat(64),
        "identity":{"products":["BODY"],"internal_versions":["1.0"],"code_checks":[]},
        "setup":{"ram_alias":0,"configuration":{"address":4096,"cached_address":4096,"size":256,
            "page_bytes":64,"memory_read_bytes":248},"debug_offset":247,
            "guard":{"offset":163,"value":1,"aliases":[4259],"code_checks":[
                {"name":"guard","address":8192,"size":4,"sha256":"b".repeat(64)}],
                "state_words":[{"name":"system_mode","address":8200,"value":7}],
                "callback_table":{"address":8216,"words":[123,456]},
                "output_state":{"address":8224,"size":8},
                "payloads":[{"name":"main","address":8300,"size":100}]}},
        "persistence":null})
}
fn decode(value: Value) -> Result<WorkflowDefinition, WorkflowError> {
    WorkflowDefinition::from_json(&value.to_string(), &identity())
}
#[test]
fn setup_does_not_require_debug_persistence() {
    let definition = decode(document()).unwrap();
    let setup = definition.setup().unwrap();
    assert_eq!(setup.guard.callback_table.1, [123, 0, 0, 0, 200, 1, 0, 0]);
    assert_eq!(setup.configuration.memory_read_bytes, 248);
    assert_eq!(setup.identity.products, ["BODY"]);
    assert!(matches!(
        definition.persistence(),
        Err(WorkflowError::Missing("debug persistence"))
    ));
}
#[test]
fn located_persistence_decodes_separately() {
    let mut value = document();
    value["persistence"] = json!({"callback":{"address":8228,"stock":123},"output_fields":8224,
        "adapter":{"address":4224,"bytes":[1,2,3,4]},
        "cache":{"clean":1,"instruction_invalidate":2,"barrier":3,"instruction_barrier":4},
        "code_checks":[],"storage":{"geometry":{"address":9000,"words":[40,931,null,1,971]},
            "banks":[{"address":10000,"size":100}]},"preferences":{"address":8300,"size":100}});
    let definition = decode(value.clone()).unwrap();
    assert_eq!(
        definition.persistence().unwrap().storage_geometry.1[2],
        None
    );
    value["setup"] = Value::Null;
    assert!(decode(value).is_err());
}
#[test]
fn document_is_closed_and_firmware_bound() {
    let mut value = document();
    value["firmware_id"] = json!("other-100");
    assert!(decode(value).is_err());
    let mut value = document();
    value["review"] = json!({"status":"reviewed"});
    assert!(decode(value).is_err());
    let mut value = document();
    value["setup"]["configuration"]["address"] = json!("0x1000");
    assert!(decode(value).is_err());
}
#[test]
fn invalid_extents_are_refused() {
    for (field, value) in [
        ("offset", json!(256)),
        ("offset", json!(128)),
        ("offset", json!(247)),
        ("value", json!(0)),
    ] {
        let mut contract = document();
        contract["setup"]["guard"][field] = value;
        assert!(decode(contract).is_err());
    }
    let mut value = document();
    value["setup"]["guard"]["output_state"]["address"] = json!(u32::MAX);
    assert!(decode(value).is_err());
}
