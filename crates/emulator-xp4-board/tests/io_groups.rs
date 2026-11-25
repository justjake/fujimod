//! IO-group board composition; no guest instructions are executed.
mod support;
use serde_json::{Value,json};
fn input() -> Value { json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":0x20000000u32}) }
#[test]
fn io_group_is_explicit_and_owns_its_exact_window() {
    let absent=support::open(serde_json::from_value(input()).unwrap()).unwrap();
    assert!(!absent.device_state_json().contains_key("keyed-io-group-control"));
    let mut selected=input(); selected["io_group_base"]=json!(0xfff42000u64);
    let board=support::open(serde_json::from_value(selected.clone()).unwrap()).unwrap();
    let state: Value=board.device_state_json()["keyed-io-group-control"].clone();
    assert_eq!(state["active"],0); assert_eq!(state["status"],0);
    for address in [0xfff42000u64,0xfff42018,0xfff4209c] {
        let mut conflict=selected.clone(); conflict["control_requests"]=json!([[address,4]]);
        let error=support::open(serde_json::from_value(conflict).unwrap()).err().unwrap();
        assert!(error.contains("Overlapping XP4 IO-group"),"{error}");
    }
    // A request word at +0x80 owns its whole 0x100-byte block.
    selected["control_requests"]=json!([[0xfff42120u64,4]]);
    assert!(support::open(serde_json::from_value(selected).unwrap()).is_ok());
}
#[test]
fn gpio_owner_overlap_is_rejected_but_adjacent_normal_gpio_is_valid() {
    for (base,valid) in [(0xfff42000u64,false),(0xfff41000u64,true)] {
        let mut selected=input(); selected["io_group_base"]=json!(0xfff42000u64);
        selected["gpio"]=json!(vec![0;16]); selected["gpio_base"]=json!(base);
        let result=support::open(serde_json::from_value(selected).unwrap());
        if valid { let states=result.unwrap().device_state_json(); assert!(states.contains_key("keyed-io-group-control")); }
        else { let error=result.err().unwrap(); assert!(error.contains("Overlapping XP4 IO-group"),"{error}"); }
    }
}
