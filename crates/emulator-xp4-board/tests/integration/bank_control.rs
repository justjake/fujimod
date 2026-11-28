use crate::support;
use fujimod_emulator_cpu::arm;
use serde_json::{Value, json};

const ENTRY: u64 = 0x1000;
const BANK: u64 = 0x1000_1000;

fn definition() -> Value {
    json!({"ram_size": 65536, "cores": 1, "discovery": false,
        "cbar": 0x2000_0000u32, "instructions_per_tick": [1, 1],
        "initial_registers": [[["Pc", ENTRY]]]})
}
fn configured() -> Value {
    let mut value = definition();
    value["bank_command_registers"] = json!([BANK, BANK + 16]);
    value
}
fn program(board: &support::Board, instruction: u32, address: u64, value: u64) {
    board
        .write(ENTRY as usize, &instruction.to_le_bytes())
        .unwrap();
    board.write_register(0, arm::PC, ENTRY).unwrap();
    board.write_register(0, arm::CORE[0], address).unwrap();
    board.write_register(0, arm::CORE[1], value).unwrap();
}

#[test]
fn wiring_is_optional_and_rejects_invalid_or_overlapping_words() {
    let board = support::board(definition()).unwrap();
    program(&board, 0xe580_1000, BANK, 1); // STR r1,[r0]
    assert!(board.run(0, 1).is_err());
    for words in [
        json!([BANK, BANK]),
        json!([BANK + 1]),
        json!([0x1_0000_0000u64]),
    ] {
        let mut input = definition();
        input["bank_command_registers"] = words;
        assert!(support::board(input).is_err());
    }
    let mut input = configured();
    input["register_banks"] = json!([[BANK, BANK + 4]]);
    assert!(support::board(input).is_err());
    let mut input = configured();
    input["memory_regions"] = json!([{ "base": BANK, "size": 4096, "permissions": 3 }]);
    assert!(support::board(input).is_err());
}

#[test]
fn guest_commands_and_checkpoint_preserve_independent_history() {
    let board = support::board(configured()).unwrap();
    program(&board, 0xe580_1000, BANK, 1);
    board.run(0, 1).unwrap();
    let saved = board.checkpoint().unwrap();
    let restored = support::board(configured()).unwrap();
    restored.restore(&saved).unwrap();
    program(&restored, 0xe580_1000, BANK, 0);
    restored.run(0, 1).unwrap();
    assert_eq!(
        restored
            .device_records()
            .iter()
            .filter(|v| v["name"] == "XP4 bank control write")
            .count(),
        2
    );
    // A fresh board has no corresponding command history.
    let fresh = support::board(configured()).unwrap();
    program(&fresh, 0xe580_1000, BANK, 0);
    assert!(fresh.run(0, 1).is_err());
    let other = support::board(configured()).unwrap();
    other.restore(&saved).unwrap();
    program(&other, 0xe580_1000, BANK + 16, 0);
    assert!(other.run(0, 1).is_err());
    let read = support::board(configured()).unwrap();
    program(&read, 0xe590_2000, BANK, 0); // LDR r2,[r0]
    assert!(read.run(0, 1).is_err());
}
