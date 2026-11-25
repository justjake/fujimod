//! X-T30 2.01 reads battery-monitor selector 27 once per five-phase poll and
//! keeps the word at `0x0150b692`, which nothing reads back. The declared word
//! is strict hardware; the register stays read-only.
mod support;
use fujimod_emulator_xp4_board::Definition;
use serde_json::{Value, json};

fn definition(sample: Option<u16>) -> Value {
    let mut inputs = json!({"0":0,"1":0,"2":0,"3":512,"24":165,"32":0,"34":0,"35":0});
    if let Some(sample) = sample {
        inputs["27"] = json!(sample);
    }
    json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":0xffff0000u32,
        "gpio":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
        "power_spi":[0xfff33000u32,false,12,5,true,inputs,54000000]})
}

fn session(value: Value) -> Result<support::Board, String> {
    support::open(serde_json::from_value::<Definition>(value).map_err(|e| e.to_string())?)
}

#[test]
fn declared_battery_sample_is_strict_hardware() {
    for sample in [0u16, 0x1234, 0xffff] {
        session(definition(Some(sample))).expect("a declared battery sample is strict hardware");
    }
    session(definition(None)).expect("the sample stays optional");
}

#[test]
fn battery_sample_is_a_full_sixteen_bit_word() {
    let mut too_wide = definition(Some(0));
    too_wide["power_spi"][5]["27"] = json!(0x10000);
    assert!(session(too_wide).is_err());
}
