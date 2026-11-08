use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};
fn definition() -> Value {
    json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":0xffff0000u32,
        "gpio":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
        "power_spi":[0xfff33000u32,false,12,5,true,
            {"0":0,"1":0,"2":0,"3":0,"32":0,"34":0,"35":0},54000000]})
}
fn session(value: Value) -> Result<Session,String> {
    Session::new(serde_json::from_value::<Definition>(value).map_err(|e|e.to_string())?,None,vec![])
}
#[test]
fn command_output_is_optional_typed_and_requires_its_peer() {
    assert!(session(definition()).is_ok());
    let mut selected=definition();
    selected["power_spi_command_output"]=json!({"bank":false,"register":17,"value":4096});
    assert!(session(selected.clone()).is_ok());
    let mut orphan=selected.clone();orphan.as_object_mut().unwrap().remove("power_spi");
    assert!(session(orphan).err().unwrap().contains("requires power SPI"));
    for register in [0,2,18,23,24,32,38,64] {
        let mut value=selected.clone();value["power_spi_command_output"]["register"]=json!(register);
        assert!(session(value).is_err());
    }
    for field in ["bank","register","value"] {
        let mut value=selected.clone();value["power_spi_command_output"].as_object_mut().unwrap().remove(field);
        assert!(session(value).is_err());
    }
    let mut value=selected;value["power_spi_command_output"]["extra"]=json!(true);
    assert!(session(value).is_err());
}
