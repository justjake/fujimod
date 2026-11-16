//! FF61/FF51 controller boundary at the current model limit.
//!
//! The catalog's X-T3 5.11 basis describes channel-1 request/lock/apply words,
//! but only the explicit `commands` entries are modeled. These tests separate
//! plain retention of configuration words from readiness/status transitions and
//! keep every readiness result labelled synthetic. No GFX100S firmware runs
//! here, and nothing here claims training, timing or a physical result.
use fujimod_emulator_cpu::arm;
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::Value;
use serde_json::json;

const CBAR: u32 = 0x2000_0000;
const CODE: usize = 0x1000;
const RESULT: usize = 0x2000;
const REQUEST: u32 = 0xFF61_2000;
const LOCK_BLOCK: u32 = 0xFF61_4000;
const LOCK: u32 = 0xFF61_4670;
const APPLY_BLOCK: u32 = 0xFF51_7000;
const APPLY: u32 = 0xFF51_7C18;

fn input(idle: Value) -> Value {
    json!({"ram_size": 65536, "cores": 1, "discovery": false, "cbar": CBAR,
    "initial_registers": [[["Pc", CODE]]],
    "xpp_register_blocks": [
        {"base": REQUEST, "size": 0x1000, "idle": [],
         "commands": [{"word": 0, "value": 0xB01, "status": 0x110, "mask": 1, "state": 1}]},
        {"base": LOCK_BLOCK, "size": 0x1000, "idle": idle},
        {"base": APPLY_BLOCK, "size": 0x1000, "idle": []}
    ]})
}

fn session(value: Value) -> Result<Session, String> {
    let definition: Definition = serde_json::from_value(value).map_err(|e| e.to_string())?;
    Session::new(definition, None, vec![])
}

fn run(board: &Session, program: &[u32], registers: &[(usize, u32)]) -> Result<(), String> {
    let bytes: Vec<u8> = program.iter().flat_map(|w| w.to_le_bytes()).collect();
    board.write(CODE, &bytes).unwrap();
    for (index, value) in registers {
        board
            .write_register(0, arm::CORE[*index], (*value).into())
            .unwrap();
    }
    board.run_board(4000, 100)
}

fn word(board: &Session, address: usize) -> u32 {
    let mut bytes = [0; 4];
    board.read(address, &mut bytes).unwrap();
    u32::from_le_bytes(bytes)
}

// str r1,[r0]; ldr r2,[r0]; str r2,[r3]; mov r0,#0; b .
const COPY: [u32; 5] = [0xe580_1000, 0xe590_2000, 0xe583_2000, 0xe3a0_0000, 0xeaff_fffe];
// str r1,[r0]; ldr r2,[r0,#0x110]; str r2,[r3]; mov r0,#0; b .
const STATUS: [u32; 5] = [0xe580_1000, 0xe590_2110, 0xe583_2000, 0xe3a0_0000, 0xeaff_fffe];
// ldr r2,[r0]; str r2,[r3]; mov r0,#0; b .
const LOAD: [u32; 4] = [0xe590_2000, 0xe583_2000, 0xe3a0_0000, 0xeaff_fffe];

#[test]
fn retained_configuration_does_not_satisfy_status_polls() {
    let board = session(input(json!([]))).unwrap();
    run(&board, &COPY, &[(0, LOCK), (1, 0x924), (3, RESULT as u32)]).unwrap();
    assert_eq!(word(&board, RESULT), 0x924, "configuration must be retained");
    assert_eq!(
        word(&board, RESULT) & 0x30000,
        0,
        "retention must not satisfy the 0x30000 lock poll"
    );
    run(&board, &STATUS, &[(0, REQUEST), (1, 0xB00), (3, RESULT as u32)]).unwrap();
    assert_eq!(
        word(&board, RESULT) & 1,
        0,
        "0xB00 is a request word, not the 0xB01 command"
    );
}

#[test]
fn catalog_command_sets_the_status_bit_but_is_not_training() {
    let board = session(input(json!([]))).unwrap();
    run(&board, &STATUS, &[(0, REQUEST), (1, 0xB01), (3, RESULT as u32)]).unwrap();
    assert_eq!(word(&board, RESULT) & 1, 1, "the declared command law sets bit 0");
    let other = session(input(json!([]))).unwrap();
    run(&other, &STATUS, &[(0, REQUEST), (1, 0xB02), (3, RESULT as u32)]).unwrap();
    assert_eq!(word(&other, RESULT) & 1, 0);
}

#[test]
fn config_readback_is_distinguished_from_a_status_transition() {
    let board = session(input(json!([]))).unwrap();
    run(&board, &COPY, &[(0, APPLY), (1, 0x4000_0000), (3, RESULT as u32)]).unwrap();
    assert_eq!(word(&board, RESULT), 0x4000_0000);
    assert_ne!(
        word(&board, RESULT) & 0x4000_0001,
        1,
        "retaining 0x40000000 does not satisfy the 0x40000001 poll; bit 0 is separate"
    );
    // A synthetic fixed word is the only way this model satisfies the poll.
    let mut value = input(json!([]));
    value["xpp_register_blocks"][2]["fixed"] = json!([[0xC18, 1]]);
    let fixed = session(value).unwrap();
    run(&fixed, &LOAD, &[(0, APPLY), (3, RESULT as u32)]).unwrap();
    assert_eq!(word(&fixed, RESULT) & 0x4000_0001, 1);
}

#[test]
fn synthetic_readiness_control_is_hypothetical_only() {
    // A declared `idle` entry is the only way this model can satisfy the lock
    // poll. It is a synthetic control, not a GFX100S measurement, and it must
    // not be used to justify an accepted board model.
    let board = session(input(json!([[0x670, 0x30000]]))).unwrap();
    run(&board, &LOAD, &[(0, LOCK), (3, RESULT as u32)]).unwrap();
    assert_eq!(word(&board, RESULT) & 0x30000, 0x30000);
    let plain = session(input(json!([]))).unwrap();
    run(&plain, &LOAD, &[(0, LOCK), (3, RESULT as u32)]).unwrap();
    assert_eq!(word(&plain, RESULT) & 0x30000, 0);
}

#[test]
fn unsupported_controller_access_is_refused() {
    let board = session(input(json!([]))).unwrap();
    let result = run(&board, &LOAD, &[(0, 0xFF61_F000), (3, RESULT as u32)]);
    assert!(result.is_err() || board.state(false).failure.is_some());
}

#[test]
fn an_unmodeled_block_is_refused_rather_than_ready() {
    let mut value = input(json!([]));
    value["xpp_register_blocks"] = json!([]);
    let board = session(value).unwrap();
    let result = run(&board, &COPY, &[(0, LOCK), (1, 0x30000), (3, RESULT as u32)]);
    assert!(result.is_err() || board.state(false).failure.is_some());
}
