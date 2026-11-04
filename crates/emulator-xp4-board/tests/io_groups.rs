//! IO-group board composition; no guest instructions are executed.
use fujimod_emulator_xp4_board::Session;
use serde_json::{Value,json};
fn input() -> Value { json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":0x20000000u32}) }
#[test]
fn io_group_is_explicit_and_owns_its_exact_window() {
    let absent=Session::new(serde_json::from_value(input()).unwrap(),None,vec![]).unwrap();
    assert!(!absent.device_state_json().contains_key("keyed-io-group-control"));
    let mut selected=input(); selected["io_group_base"]=json!(0xfff42000u64);
    let board=Session::new(serde_json::from_value(selected.clone()).unwrap(),None,vec![]).unwrap();
    let state: Value=serde_json::from_str(&board.device_state_json()["keyed-io-group-control"]).unwrap();
    assert_eq!(state["active"],0); assert_eq!(state["status"],0);
    for address in [0xfff42000u64,0xfff42018,0xfff4209c] {
        let mut conflict=selected.clone(); conflict["control_requests"]=json!([[address,4]]);
        let error=Session::new(serde_json::from_value(conflict).unwrap(),None,vec![]).err().unwrap();
        assert!(error.contains("Overlapping XP4 IO-group"),"{error}");
    }
    selected["control_requests"]=json!([[0xfff420a0u64,4]]);
    assert!(Session::new(serde_json::from_value(selected).unwrap(),None,vec![]).is_ok());
}
#[test]
fn gpio_owner_overlap_is_rejected_but_adjacent_normal_gpio_is_valid() {
    for (base,valid) in [(0xfff42000u64,false),(0xfff41000u64,true)] {
        let mut selected=input(); selected["io_group_base"]=json!(0xfff42000u64);
        selected["gpio"]=json!(vec![0;16]); selected["gpio_base"]=json!(base);
        let result=Session::new(serde_json::from_value(selected).unwrap(),None,vec![]);
        if valid { let states=result.unwrap().device_state_json(); assert!(states.contains_key("keyed-io-group-control")); }
        else { let error=result.err().unwrap(); assert!(error.contains("Overlapping XP4 IO-group"),"{error}"); }
    }
}
