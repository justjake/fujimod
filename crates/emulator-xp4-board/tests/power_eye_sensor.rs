use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

fn definition() -> Value {
    json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":0xffff0000u32,
        "gpio":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
        "power_spi":[0xfff33000u32,false,12,5,true,
            {"0":0,"1":0,"2":0,"3":0,"16":4,"32":0,"34":0,"35":0},54000000],
        "power_spi_bank_inputs":[[true,16,2]],
        "power_spi_eye_commit":true})
}

fn session(value: Value) -> Result<Session, String> {
    Session::new(
        serde_json::from_value::<Definition>(value).map_err(|e| e.to_string())?,
        None,
        vec![],
    )
}

#[test]
fn eye_bank_inputs_and_commit_reach_the_power_peer() {
    assert!(session(definition()).is_ok());
    for bank_inputs in [
        json!([[true, 32, 0]]),
        json!([[true, 3, 1024]]),
        json!([[true, 16, 1], [true, 16, 2]]),
    ] {
        let mut value = definition();
        value["power_spi_bank_inputs"] = bank_inputs;
        assert!(session(value).is_err());
    }
}

#[test]
fn eye_options_require_the_power_spi_peer() {
    for field in ["power_spi_bank_inputs", "power_spi_eye_commit"] {
        let mut value = definition();
        value.as_object_mut().unwrap().remove("power_spi");
        value.as_object_mut().unwrap().remove("power_spi_bank_inputs");
        value["power_spi_bank_inputs"] = if field == "power_spi_bank_inputs" {
            json!([[true, 16, 2]])
        } else {
            json!([])
        };
        value["power_spi_eye_commit"] = json!(field == "power_spi_eye_commit");
        let error = session(value).err().unwrap();
        assert!(error.contains("require a power SPI peer"), "{error}");
    }
}
