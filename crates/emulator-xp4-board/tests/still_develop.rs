//! The XP4 RECC engine's opt-in development (X-T3 5.11: page 0xfff82000,
//! interrupt 129): the `raw` wiring is admitted on RECC alone and checked.
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

const RECC: u32 = 0xfff8_2000;

fn input(engine: &str, extra: Value) -> Value {
    let mut wiring = json!({"engine": engine, "base": RECC, "irq": 129, "run_ticks": 8});
    for (k, v) in extra.as_object().unwrap() {
        wiring[k] = v.clone();
    }
    json!({"ram_size": 65536, "cores": 1, "discovery": false, "cbar": 0x2000_0000_u32,
        "initial_registers": [[["Pc", 0x1000]]], "still_engines": [wiring]})
}

fn session(value: Value) -> Result<Session, String> {
    let definition: Definition = serde_json::from_value(value).map_err(|e| e.to_string())?;
    Session::new(definition, None, vec![])
}

#[test]
fn only_recc_develops_and_only_from_a_valid_raw() {
    let raw = json!({"address": 0x473d_8fd0_u32, "pitch": 0x31e0, "site": [5, 1]});
    session(input("recc", json!({}))).unwrap();
    session(input("recc", json!({ "raw": raw }))).unwrap();
    let error = session(input("recc", json!({"compute": true})))
        .err()
        .unwrap();
    assert!(error.contains("develops from a declared RAW"), "{error}");
    for engine in ["iifp", "resz", "reszl", "mix", "retch"] {
        let error = session(input(engine, json!({ "raw": raw }))).err().unwrap();
        assert!(error.contains("does not develop"), "{engine}: {error}");
    }
    for invalid in [
        json!({"address": 0, "pitch": 0x31e0, "site": [6, 1]}),
        json!({"address": 0, "pitch": 7, "site": [5, 1]}),
    ] {
        let error = session(input("recc", json!({ "raw": invalid })))
            .err()
            .unwrap();
        assert!(error.contains("Invalid xp4-still-recc RAW"), "{error}");
    }
    let mut extra = raw.clone();
    extra["phase"] = json!([0, 0]);
    assert!(session(input("recc", json!({ "raw": extra }))).is_err());
}
