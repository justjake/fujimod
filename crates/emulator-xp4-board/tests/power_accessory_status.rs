mod support;
use fujimod_emulator_xp4_board::Definition;
use serde_json::{Value, json};

fn definition(status: Option<u16>) -> Value {
    let mut inputs = json!({"0":0,"1":0,"2":0,"3":512,"32":0,"34":0,"35":0});
    if let Some(status) = status {
        inputs["24"] = json!(status);
    }
    json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":0xffff0000u32,
        "gpio":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
        "power_spi":[0xfff33000u32,false,12,5,true,inputs,54000000]})
}

#[test]
fn declared_accessory_status_word_is_strict_hardware() {
    for status in [0x00a5, 0xff0c] {
        let strict: Definition = serde_json::from_value(definition(Some(status))).unwrap();
        support::open(strict).expect("a declared status word is strict hardware");
    }
}

#[test]
fn ordinary_power_inputs_remain_available_in_strict_mode() {
    let strict: Definition = serde_json::from_value(definition(None)).unwrap();
    assert!(support::open(strict).is_ok());
}
