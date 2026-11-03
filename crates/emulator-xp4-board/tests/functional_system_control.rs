//! Definition admission only: these tests execute no guest instructions.
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};
fn input() -> Value { json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":0x20000000u32}) }
fn config() -> Value { json!({"profile":"experimental-source-functional-v1","control_f03c_reset":0}) }
#[test]
fn absent_input_keeps_device_disconnected_and_explicit_input_connects() {
    let strict: Definition=serde_json::from_value(input()).unwrap();
    let strict=Session::new(strict,None,vec![]).unwrap();
    assert!(!strict.device_state_json().contains_key("xp4-functional-system-control"));
    let mut v=input(); v["xp4_functional_system_control"]=config();
    let functional=Session::new(serde_json::from_value(v).unwrap(),None,vec![]).unwrap();
    assert!(functional.device_state_json().contains_key("xp4-functional-system-control"));
}
#[test]
fn typed_inputs_reject_missing_reset_and_overlapping_request_owner() {
    let mut v=input(); v["xp4_functional_system_control"]=json!({"profile":"experimental-source-functional-v1"});
    assert!(serde_json::from_value::<Definition>(v).is_err());
    let mut v=input(); v["xp4_functional_system_control"]=config();
    v["control_requests"]=json!([[0xff70f03cu64,2]]);
    let error=Session::new(serde_json::from_value(v).unwrap(),None,vec![]).err().unwrap();
    assert!(error.contains("Overlapping"),"{error}");
}

fn banks() -> Value {
    let a_reset = [0; 16];
    let b_reset = [0; 15];
    json!({"a_reset": a_reset, "b_reset": b_reset})
}
#[test]
fn exact_bank_definition_connects_and_rejects_each_duplicate_owner() {
    let mut cfg = config(); cfg["retained_request_banks"] = banks();
    let mut v = input(); v["xp4_functional_system_control"] = cfg.clone();
    let session = Session::new(serde_json::from_value(v.clone()).unwrap(), None, vec![]).unwrap();
    let state: Value = serde_json::from_str(&session.device_state_json()["xp4-functional-system-control"]).unwrap();
    assert_eq!(state["retained_banks"]["a"].as_array().unwrap().len(), 15);
    for (address, _) in fujimod_emulator_xp4_io::system_control::A_WORDS.into_iter()
        .chain(fujimod_emulator_xp4_io::system_control::B_WORDS)
    {
        let mut conflict = v.clone(); conflict["control_requests"] = json!([[address, 2], [address, 2]]);
        let error = Session::new(serde_json::from_value(conflict).unwrap(), None, vec![]).err().unwrap();
        assert!(error.contains("Overlapping"), "{error}");
    }
    let mut partial = v.clone(); partial["control_requests"] = json!([[0xfff48609u64, 2]]);
    assert!(Session::new(serde_json::from_value(partial).unwrap(), None, vec![]).is_err());
    cfg["retained_request_banks"]["a_reset"][15] = json!(1);
    v["xp4_functional_system_control"] = cfg;
    assert!(Session::new(serde_json::from_value(v).unwrap(), None, vec![]).is_err());
}
