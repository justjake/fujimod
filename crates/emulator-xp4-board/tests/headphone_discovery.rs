use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

fn definition(discovery: bool) -> Value {
    json!({"ram_size":65536,"cores":1,"discovery":discovery,"cbar":0xffff0000u32,
        "gpio":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
        "power_spi":[0xfff33000u32,false,12,5,true,
            {"0":0,"1":0,"2":0,"3":512,"24":165,"32":0,"34":0,"35":0},54000000]})
}

#[test]
fn observed_status_fixture_requires_discovery() {
    let strict: Definition = serde_json::from_value(definition(false)).unwrap();
    let error = Session::new(strict, None, vec![])
        .err()
        .expect("strict mode must refuse");
    assert!(error.contains("requires discovery mode"), "{error}");
    let discovery: Definition = serde_json::from_value(definition(true)).unwrap();
    assert!(Session::new(discovery, None, vec![]).is_ok());
}

#[test]
fn ordinary_power_inputs_remain_available_in_strict_mode() {
    let mut value = definition(false);
    value["power_spi"][5].as_object_mut().unwrap().remove("24");
    let strict: Definition = serde_json::from_value(value).unwrap();
    assert!(Session::new(strict, None, vec![]).is_ok());
}
