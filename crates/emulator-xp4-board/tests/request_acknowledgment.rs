use fujimod_emulator_cpu::arm;
mod support;
use fujimod_emulator_xp4_board::Definition;
use fujimod_emulator_xp4_io::request_acknowledgment::ADDRESS;
use serde_json::{Value, json};

const ENTRY: u64 = 0x1000;
const NAME: &str = "xp4-request-acknowledgment";

fn input() -> Value {
    json!({
        "ram_size": 65536,
        "cores": 1,
        "discovery": false,
        "cbar": 0x2000_0000u32,
        "instructions_per_tick": [1, 1],
        "initial_registers": [[["Pc", ENTRY]]]
    })
}

fn board(input: Value) -> Result<support::Board, String> {
    let definition: Definition = serde_json::from_value(input).unwrap();
    support::open(definition)
}

fn program(session: &support::Board, address: u64, words: &[u32]) {
    let bytes: Vec<_> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    session.write(ENTRY as usize, &bytes).unwrap();
    session.write_register(0, arm::CORE[0], address).unwrap();
    session.write_register(0, arm::CORE[1], 1).unwrap();
}

#[test]
fn strict_defaults_and_adjacent_words_remain_unmapped() {
    for (ticks, address) in [
        (None, ADDRESS),
        (Some(8), ADDRESS - 4),
        (Some(8), ADDRESS + 4),
    ] {
        let mut definition = input();
        if let Some(ticks) = ticks {
            definition["request_acknowledgment_ticks"] = json!(ticks);
        }
        let session = board(definition).unwrap();
        assert_eq!(
            session.device_state_json().contains_key(NAME),
            ticks.is_some()
        );
        program(&session, address, &[0xe590_2000]); // LDR r2,[r0]
        assert!(session.run(0, 1).is_err(), "unexpected access at {address:#x}");
    }
}

#[test]
fn exact_word_registration_refuses_overlapping_owners() {
    let mut definition = input();
    definition["request_acknowledgment_ticks"] = json!(8);
    for (key, wiring) in [
        ("register_banks", json!([[ADDRESS, ADDRESS + 4]])),
        ("control_requests", json!([[ADDRESS, 8]])),
        ("control_requests", json!([[ADDRESS + 4, 8]])),
    ] {
        let mut overlapping = definition.clone();
        overlapping[key] = wiring;
        let error = board(overlapping).err().unwrap();
        assert!(error.contains("Overlapping"), "{error}");
    }
    definition["register_banks"] = json!([[ADDRESS - 4, ADDRESS], [ADDRESS + 4, ADDRESS + 8]]);
    assert!(board(definition.clone()).is_ok());
    for invalid in [0u64, 1_000_001] {
        definition["request_acknowledgment_ticks"] = json!(invalid);
        assert!(board(definition.clone()).is_err());
    }
}

#[test]
fn guest_writes_one_and_polls_until_the_declared_acknowledgment() {
    for ticks in [8u64, 17] {
        let mut definition = input();
        definition["request_acknowledgment_ticks"] = json!(ticks);
        let session = board(definition).unwrap();
        program(
            &session,
            ADDRESS,
            &[
                0xe580_1000, // STR r1,[r0]
                0xe590_2000, // LDR r2,[r0] (first observation)
                0xe590_3000, // LDR r3,[r0] (poll)
                0xe353_0000, // CMP r3,#0
                0x1aff_fffc, // BNE poll
                0xe1a0_0000, // MOV r0,r0 (stop before this instruction)
            ],
        );
        let stop = ENTRY + 5 * 4;
        for _ in 0..128 {
            if session.read_register(0, arm::PC).unwrap() == stop {
                break;
            }
            session.run_board(1, 1).unwrap();
        }
        assert_eq!(session.read_register(0, arm::PC).unwrap(), stop);
        assert_eq!(session.read_register(0, arm::CORE[2]).unwrap(), 1);
        assert_eq!(session.read_register(0, arm::CORE[3]).unwrap(), 0);
        let states = session.device_state_json();
        let snapshot: Value = states[NAME].clone();
        let state = &snapshot["state"];
        let start = state["request"]["started_at"].as_u64().unwrap();
        let deadline = start + ticks;
        assert_eq!(state["phase"], json!("completed"));
        assert_eq!(state["request"]["deadline"], json!(deadline));
        assert_eq!(state["completed_at"], json!(deadline));
    }
}
