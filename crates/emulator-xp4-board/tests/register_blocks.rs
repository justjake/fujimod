//! X-Processor Pro register blocks on the XP4 board: a block may start on a
//! word boundary after another window of the same page (X-T3 5.11: the gate
//! request word 0xff702080 that `catalog/soc/xp4/soc.json` declares owns its
//! block 0xff702000..0xff702100), and a command may clear its own busy bit
//! (0xfff7801c, which the firmware writes to 1 and reads back until bit 0 is
//! clear).
use fujimod_emulator_cpu::arm;
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

const CBAR: u32 = 0x2000_0000;
const CODE: usize = 0x1000;
const RESULT: usize = 0x2000;
const GATES: u32 = 0xff70_2000;
const ENGINE: u32 = 0xfff7_8000;

fn input() -> Value {
    json!({"ram_size": 65536, "cores": 1, "discovery": false, "cbar": CBAR,
    "initial_registers": [[["Pc", CODE]]],
    "control_requests": [[GATES + 0x80, 4]],
    "xpp_register_blocks": [
        {"base": GATES + 0x100, "size": 0xf00, "idle": []},
        {"base": ENGINE, "size": 0x1000, "idle": [],
         "commands": [{"word": 0x1c, "value": 1, "status": 0x1c, "mask": 1, "state": 0}]},
    ]})
}

fn session(value: Value) -> Result<Session, String> {
    let definition: Definition = serde_json::from_value(value).map_err(|e| e.to_string())?;
    Session::new(definition, None, vec![])
}

fn word(session: &Session, address: usize) -> u32 {
    let mut bytes = [0; 4];
    session.read(address, &mut bytes).unwrap();
    u32::from_le_bytes(bytes)
}

#[test]
fn a_block_may_share_its_page_with_an_earlier_window() {
    session(input()).unwrap();
    // The whole page would overlap the gate request block.
    let mut whole = input();
    whole["xpp_register_blocks"][0]["base"] = json!(GATES);
    whole["xpp_register_blocks"][0]["size"] = json!(0x1000);
    let error = session(whole).err().unwrap();
    assert!(error.contains("Overlapping"), "{error}");
    // Not word-aligned, or past the end of its page.
    for (base, size) in [(GATES + 0x102, 0xefc), (GATES + 0x100, 0xf04)] {
        let mut invalid = input();
        invalid["xpp_register_blocks"][0]["base"] = json!(base);
        invalid["xpp_register_blocks"][0]["size"] = json!(size);
        let error = session(invalid).err().unwrap();
        assert!(
            error.contains("Invalid X-Processor Pro register block"),
            "{error}"
        );
    }
}

#[test]
fn a_guest_busy_wait_on_a_self_clearing_command_completes() {
    let board = session(input()).unwrap();
    board.write(RESULT, &[0xff; 8]).unwrap();
    // The firmware's enable: write 1 to +0x1c, read it back until bit 0 is
    // clear. Then a word of the shared page reads back its write.
    let program: [u32; 9] = [
        0xe580_101c, // STR r1, [r0, #0x1c]
        0xe590_201c, // LDR r2, [r0, #0x1c]
        0xe312_0001, // TST r2, #1
        0x1aff_fffc, // BNE the load above
        0xe583_2000, // STR r2, [r3]
        0xe584_5000, // STR r5, [r4]
        0xe594_2000, // LDR r2, [r4]
        0xe583_2004, // STR r2, [r3, #4]
        0xeaff_fffe, // B .
    ];
    let bytes: Vec<u8> = program.iter().flat_map(|w| w.to_le_bytes()).collect();
    board.write(CODE, &bytes).unwrap();
    for (register, value) in [
        (0, ENGINE),
        (1, 1),
        (3, RESULT as u32),
        (4, GATES + 0x200),
        (5, 0x5a),
    ] {
        board
            .write_register(0, arm::CORE[register], value.into())
            .unwrap();
    }
    board.run_board(4000, 100).unwrap();
    assert!(board.state(false).failure.is_none());
    assert_eq!(word(&board, RESULT), 0, "busy bit read clear");
    assert_eq!(word(&board, RESULT + 4), 0x5a, "shared page word");
}
