//! FF61/FF51 controller boundary at the current model limit.
//!
//! The catalog's X-T3 5.11 basis describes channel-1 request/lock/apply words
//! and declares `fixed_words` plus `idle_bits` for them; the shared model
//! applies fixed first and ORs idle over the stored word. These tests separate
//! plain retention of configuration words from that declared readiness, and
//! keep every readiness result labelled synthetic. No GFX100S firmware runs
//! here, and nothing here claims training, timing or a physical result.
use fujimod_emulator_cpu::arm;
mod support;
use fujimod_emulator_xp4_board::Definition;
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
const CLEAR_BLOCK: u32 = 0xFF51_5000;
const CLEAR: u32 = 0xFF51_5384;

fn input(idle: Value) -> Value {
    json!({"ram_size": 65536, "cores": 1, "discovery": false, "cbar": CBAR,
    "initial_registers": [[["Pc", CODE]]],
    "xpp_register_blocks": [
        {"base": REQUEST, "size": 0x1000, "idle": [],
         "commands": [{"word": 0, "value": 0xB01, "status": 0x110, "mask": 1, "state": 1}]},
        {"base": LOCK_BLOCK, "size": 0x1000, "idle": idle},
        {"base": APPLY_BLOCK, "size": 0x1000, "idle": []},
        {"base": CLEAR_BLOCK, "size": 0x1000, "idle": [], "fixed": [[0x384, 0]]}
    ]})
}

fn session(value: Value) -> Result<support::Board, String> {
    let definition: Definition = serde_json::from_value(value).map_err(|e| e.to_string())?;
    support::open(definition)
}

fn run(
    board: &support::Board,
    code: usize,
    program: &[u32],
    registers: &[(usize, u32)],
) -> Result<(), String> {
    // Each program gets its own address: the board caches translations, so
    // overwriting one entry point would re-run the previous body.
    let bytes: Vec<u8> = program.iter().flat_map(|w| w.to_le_bytes()).collect();
    board.write(code, &bytes).unwrap();
    for (index, value) in registers {
        board
            .write_register(0, arm::CORE[*index], (*value).into())
            .unwrap();
    }
    board
        .write_register(0, arm::CORE[15], (code as u32).into())
        .unwrap();
    board.run_board(4000, 100)
}

fn word(board: &support::Board, address: usize) -> u32 {
    let mut bytes = [0; 4];
    board.read(address, &mut bytes).unwrap();
    u32::from_le_bytes(bytes)
}

// str r1,[r0]; ldr r2,[r0]; str r2,[r3]; mov r0,#0; b .
const COPY: [u32; 5] = [
    0xe580_1000,
    0xe590_2000,
    0xe583_2000,
    0xe3a0_0000,
    0xeaff_fffe,
];
// str r1,[r0]; ldr r2,[r0,#0x110]; str r2,[r3]; mov r0,#0; b .
const STATUS: [u32; 7] = [
    0xe580_1000, // str r1,[r0]
    0xe1a0_4000, // mov r4,r0
    0xe590_2110, // ldr r2,[r0,#0x110]
    0xe583_2000, // str r2,[r3]
    0xe583_4004, // str r4,[r3,#4]
    0xe3a0_0000, // mov r0,#0
    0xeaff_fffe, // b .
];
// ldr r2,[r0]; str r2,[r3]; mov r0,#0; b .
const LOAD: [u32; 4] = [0xe590_2000, 0xe583_2000, 0xe3a0_0000, 0xeaff_fffe];

#[test]
fn retained_configuration_does_not_satisfy_status_polls() {
    let board = session(input(json!([]))).unwrap();
    run(
        &board,
        CODE,
        &COPY,
        &[(0, LOCK), (1, 0x924), (3, RESULT as u32)],
    )
    .unwrap();
    assert_eq!(
        word(&board, RESULT),
        0x924,
        "configuration must be retained"
    );
    assert_eq!(
        word(&board, RESULT) & 0x30000,
        0,
        "retention must not satisfy the 0x30000 lock poll"
    );
    run(
        &board,
        CODE + 0x40,
        &STATUS,
        &[(0, REQUEST), (1, 0xB00), (3, RESULT as u32)],
    )
    .unwrap();
    assert_eq!(word(&board, RESULT), 0, "the status program actually ran");
    assert_eq!(
        word(&board, RESULT) & 1,
        0,
        "0xB00 is a request word, not the 0xB01 command"
    );
}

#[test]
fn catalog_command_sets_the_status_bit_but_is_not_training() {
    let board = session(input(json!([]))).unwrap();
    run(
        &board,
        CODE,
        &STATUS,
        &[(0, REQUEST), (1, 0xB01), (3, RESULT as u32)],
    )
    .unwrap();
    assert_eq!(
        word(&board, RESULT) & 1,
        1,
        "the declared command law sets bit 0"
    );
    let other = session(input(json!([]))).unwrap();
    run(
        &other,
        CODE,
        &STATUS,
        &[(0, REQUEST), (1, 0xB02), (3, RESULT as u32)],
    )
    .unwrap();
    assert_eq!(word(&other, RESULT) & 1, 0);
}

#[test]
fn config_readback_is_distinguished_from_a_status_transition() {
    let board = session(input(json!([]))).unwrap();
    run(
        &board,
        CODE,
        &COPY,
        &[(0, APPLY), (1, 0x4000_0000), (3, RESULT as u32)],
    )
    .unwrap();
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
    run(&fixed, CODE, &LOAD, &[(0, APPLY), (3, RESULT as u32)]).unwrap();
    assert_eq!(word(&fixed, RESULT) & 0x4000_0001, 1);
}

#[test]
fn synthetic_readiness_control_is_hypothetical_only() {
    // The catalog's own `idle_bits` are the declared mechanism; a single idle
    // entry here is a synthetic control, not a GFX100S measurement, and it must
    // not be used to justify an accepted board model.
    let board = session(input(json!([[0x670, 0x30000]]))).unwrap();
    run(&board, CODE, &LOAD, &[(0, LOCK), (3, RESULT as u32)]).unwrap();
    assert_eq!(word(&board, RESULT) & 0x30000, 0x30000);
    let plain = session(input(json!([]))).unwrap();
    run(&plain, CODE, &LOAD, &[(0, LOCK), (3, RESULT as u32)]).unwrap();
    assert_eq!(word(&plain, RESULT) & 0x30000, 0);
}

#[test]
fn catalog_declared_idle_and_fixed_words_satisfy_the_polls_from_reset() {
    // The X-T3 catalog declares idle bits on the lock words and fixed words on
    // FF517C18/FF515384. A catalog-shaped block therefore reads ready with no
    // command at all: FF614670 returns 0x30003 from reset and 0x924 | 0x30003
    // after the table write. This is the declared synthetic readiness and a
    // false-success hazard; it is not a completion law.
    let mut value = input(json!([
        [0x670, 0x30003],
        [0x674, 0x30003],
        [0x678, 0x30003],
        [0xDC, 0x300],
        [0x2DC, 0x300]
    ]));
    value["xpp_register_blocks"][2]["fixed"] = json!([[0xC18, 1]]);
    let board = session(value).unwrap();
    run(&board, CODE, &LOAD, &[(0, LOCK), (3, RESULT as u32)]).unwrap();
    assert_eq!(
        word(&board, RESULT) & 0x30000,
        0x30000,
        "catalog idle bits satisfy the lock poll from reset"
    );
    run(
        &board,
        CODE + 0x40,
        &COPY,
        &[(0, LOCK), (1, 0x924), (3, RESULT as u32)],
    )
    .unwrap();
    assert_eq!(word(&board, RESULT), 0x924 | 0x30003);
    run(
        &board,
        CODE + 0x80,
        &LOAD,
        &[(0, APPLY), (3, RESULT as u32)],
    )
    .unwrap();
    assert_eq!(
        word(&board, RESULT),
        1,
        "catalog fixed word is ready from reset"
    );
    run(
        &board,
        CODE + 0xC0,
        &LOAD,
        &[(0, CLEAR), (3, RESULT as u32)],
    )
    .unwrap();
    assert_eq!(
        word(&board, RESULT),
        0,
        "catalog apply-clear fixed word reads zero"
    );
    // The stock firmware writes 0x40000000 to FF517C18; a fixed word must win.
    run(
        &board,
        CODE + 0x100,
        &COPY,
        &[(0, APPLY), (1, 0x4000_0000), (3, RESULT as u32)],
    )
    .unwrap();
    assert_eq!(word(&board, RESULT), 1, "fixed wins over the stock write");
}

#[test]
fn unsupported_controller_access_is_refused() {
    let board = session(input(json!([]))).unwrap();
    let result = run(&board, CODE, &LOAD, &[(0, 0xFF61_F000), (3, RESULT as u32)]);
    assert!(result.is_err() || board.failure().is_some());
}

#[test]
fn an_unmodeled_block_is_refused_rather_than_ready() {
    let mut value = input(json!([]));
    value["xpp_register_blocks"] = json!([]);
    let board = session(value).unwrap();
    let result = run(
        &board,
        CODE,
        &COPY,
        &[(0, LOCK), (1, 0x30000), (3, RESULT as u32)],
    );
    assert!(result.is_err() || board.failure().is_some());
}
