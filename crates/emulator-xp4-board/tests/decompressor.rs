//! Decompressor wiring admission only; no guest instructions execute.
use fujimod_emulator_xp4_board::Session;
use serde_json::{Value,json};
fn input() -> Value {json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":0x20000000u32})}
#[test]
fn decompressor_is_explicit_and_owns_exact_window() {
    let absent=Session::new(serde_json::from_value(input()).unwrap(),None,vec![]).unwrap();
    assert!(!absent.device_state_json().contains_key("lz-osd-engine"));
    let mut selected=input(); selected["decompressor"]=json!([0xfff79000u64,128]);
    let board=Session::new(serde_json::from_value(selected.clone()).unwrap(),None,vec![]).unwrap();
    assert!(board.device_state_json().contains_key("lz-osd-engine"));
    for address in [0xfff79000u64,0xfff79028,0xfff79068] {
        let mut conflict=selected.clone(); conflict["control_requests"]=json!([[address,4]]);
        let error=Session::new(serde_json::from_value(conflict).unwrap(),None,vec![]).err().unwrap();
        assert!(error.contains("Overlapping decompressor"),"{error}");
    }
    selected["control_requests"]=json!([[0xfff7906cu64,4],[0xfff78ffcu64,4]]);
    assert!(Session::new(serde_json::from_value(selected).unwrap(),None,vec![]).is_ok());
}
#[test]
fn invalid_base_and_nonphysical_interrupt_routes_refuse() {
    for (base,irq) in [(0xfff80000u64,128),(0xfff79004,128),(0xfff79000,31),(0xfff79000,512)] {
        let mut selected=input();selected["decompressor"]=json!([base,irq]);
        let error=Session::new(serde_json::from_value(selected).unwrap(),None,vec![]).err().unwrap();
        assert!(error.contains("Invalid XP4 decompressor wiring"),"{error}");
    }
}
