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

#[test]
fn configuration_family_has_one_owner_and_preserves_absent_graphics_semantics() {
    let mut cfg = config();
    cfg["configuration_requests"] = json!({"initial_values": vec![0; 52], "completion_ticks": vec![8; 20]});
    let mut v = input(); v["xp4_functional_system_control"] = cfg.clone();
    let session = Session::new(serde_json::from_value(v.clone()).unwrap(), None, vec![]).unwrap();
    let state: Value = serde_json::from_str(&session.device_state_json()["xp4-functional-system-control"]).unwrap();
    assert!(state["configuration_state"].is_object());
    for address in fujimod_emulator_xp4_io::configuration_request::DESCRIPTORS.iter().map(|&(_, word)| word)
        .chain(fujimod_emulator_xp4_io::configuration_request::GROUP_BASES.iter().map(|base| base + 0x80))
    {
        let mut conflict = v.clone(); conflict["control_requests"] = json!([[address, 8]]);
        let error = Session::new(serde_json::from_value(conflict).unwrap(), None, vec![]).err().unwrap();
        assert!(error.contains("Overlapping"), "{error}");
    }
    cfg["graphics"] = json!({"initial_value": 0, "completion_ticks": 8});
    v["xp4_functional_system_control"] = cfg;
    let error = Session::new(serde_json::from_value(v).unwrap(), None, vec![]).err().unwrap();
    assert!(error.contains("overlap"), "{error}");
}

#[test]
fn selection_special_owns_exactly_three_words_and_leaves_b14_to_its_existing_owner() {
    let mut cfg = config();
    cfg["selection_special"] = json!({"selection_reset": 0, "special52_reset": 2, "special53_reset": 0});
    let mut v = input(); v["xp4_functional_system_control"] = cfg.clone();
    let session = Session::new(serde_json::from_value(v.clone()).unwrap(), None, vec![]).unwrap();
    let state: Value = serde_json::from_str(&session.device_state_json()["xp4-functional-system-control"]).unwrap();
    assert!(state["selection_state"].is_object());
    for address in [0xfff48600u64, 0xfff48604, 0xfff4860c] {
        let mut bad = v.clone(); bad["control_requests"] = json!([[address, 8]]);
        let error = Session::new(serde_json::from_value(bad).unwrap(), None, vec![]).err().unwrap();
        assert!(error.contains("Overlapping"), "{error}");
    }
    v["control_requests"] = json!([[0xfff48608u64, 8]]);
    assert!(Session::new(serde_json::from_value(v.clone()).unwrap(), None, vec![]).is_ok());
    cfg["selection_special"]["selection_reset"] = json!(0xc00);
    v["xp4_functional_system_control"] = cfg;
    assert!(Session::new(serde_json::from_value(v).unwrap(), None, vec![]).is_err());
}
