//! Board admission and wiring only; these checks execute no guest instructions.
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};
fn input() -> Value { json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":0x20000000u32}) }
fn dma() -> Value {
    json!({"irq_routes": (207..223).collect::<Vec<_>>(),
        "reset_profile":"experimental-zero-registers-v1", "timing_profile":"functional-16-setup-16-bytes-v1"})
}
#[test]
fn explicit_dma_has_no_flash_peer_and_keeps_absence_unchanged() {
    let absent = Session::new(serde_json::from_value(input()).unwrap(), None, vec![]).unwrap();
    assert!(!absent.device_state_json().contains_key("xp4-dma"));
    let mut v = input(); v["standalone_dma"] = dma();
    let board = Session::new(serde_json::from_value(v.clone()).unwrap(), None, vec![]).unwrap();
    let states = board.device_state_json(); assert_eq!(states.len(), 1);
    let state: Value = serde_json::from_str(&states["xp4-dma"]).unwrap();
    assert_eq!(state["serial_flash_peer"], false); assert_eq!(state["channels"].as_array().unwrap().len(), 16);
    for address in [0xfffd8000u64, 0xf0000000] { // Neither serial-flash controller nor aperture belongs to DMA-only wiring.
        let mut extra = v.clone(); extra["control_requests"] = json!([[address, 8]]);
        assert!(Session::new(serde_json::from_value(extra).unwrap(), None, vec![]).is_ok());
    }
    for address in [0xfffe0000u64, 0xfffef034, 0xfff4a500, 0xfff4a51c] {
        let mut extra = v.clone(); extra["control_requests"] = json!([[address, 8]]);
        let error = Session::new(serde_json::from_value(extra).unwrap(), None, vec![]).err().unwrap();
        assert!(error.contains("Overlapping"), "{error}");
    }
}
#[test]
fn profiles_routes_and_storage_controller_exclusion_are_explicit() {
    for field in ["reset_profile", "timing_profile", "irq_routes"] {
        let mut v = input(); v["standalone_dma"] = dma(); v["standalone_dma"].as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<Definition>(v).is_err());
    }
    for (field, value) in [("reset_profile", json!("default")), ("timing_profile", json!("measured")),
        ("irq_routes", json!(vec![207; 16])), ("irq_routes", json!(vec![207; 15])),
        ("irq_routes", json!((0..16).collect::<Vec<_>>()))] {
        let mut v = input(); v["standalone_dma"] = dma(); v["standalone_dma"][field] = value;
        assert!(Session::new(serde_json::from_value(v).unwrap(), None, vec![]).is_err());
    }
    let mut v = input(); v["standalone_dma"] = dma(); v["storage_jedec"] = json!(0x20);
    let error = Session::new(serde_json::from_value(v).unwrap(), None, vec![]).err().unwrap();
    assert!(error.contains("cannot be combined"), "{error}");
}
