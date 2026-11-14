//! The XP4 RESZ engine's opt-in computed output (X-T3 5.11: page
//! 0xfff7e000, interrupt 132): wiring admission, and one halving job driven
//! by guest stores and loads the way the firmware's drivers drive it.
use fujimod_emulator_cpu::arm;
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

const CBAR: u32 = 0x2000_0000;
const RESZ: u32 = 0xfff7_e000;
const IRQ: u32 = 132;
const CODE: usize = 0x1000;
const RESULT: usize = 0x2000;
const TABLE: usize = 0x3000;
const SOURCE: usize = 0x4000;
const OUTPUT: usize = 0x8000;

fn input(engine: &str, compute: bool) -> Value {
    json!({"ram_size": 65536, "cores": 1, "discovery": false, "cbar": CBAR,
        "initial_registers": [[["Pc", CODE]]],
        "still_engines": [{"engine": engine, "base": RESZ, "irq": IRQ, "run_ticks": 8,
            "compute": compute}]})
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
fn only_the_resizer_admits_a_computed_output() {
    session(input("resz", true)).unwrap();
    session(input("resz", false)).unwrap();
    let mut absent = input("resz", false);
    absent["still_engines"][0]
        .as_object_mut()
        .unwrap()
        .remove("compute");
    session(absent).unwrap();
    let error = session(input("recc", true)).err().unwrap();
    assert!(error.contains("develops from a declared RAW"), "{error}");
    for engine in ["iifp", "reszl", "mix", "retch"] {
        let error = session(input(engine, true)).err().unwrap();
        assert!(error.contains("no computed output"), "{engine}: {error}");
    }
}

#[test]
fn a_guest_resize_job_halves_the_picture_and_holds_its_line_until_acknowledged() {
    let board = session(input("resz", true)).unwrap();

    // A 32x8 packed 4:2:2 picture (Y0 Y1 Cb Cr), pitch 64: luma x + 8y,
    // Cb the pixel pair, Cr 0x80 + y.
    let mut source = vec![0u8; 64 * 8];
    for y in 0..8 {
        for pair in 0..16 {
            let p = y * 64 + pair * 4;
            source[p..p + 4].copy_from_slice(&[
                (pair * 2 + 8 * y) as u8,
                (pair * 2 + 1 + 8 * y) as u8,
                pair as u8,
                (0x80 + y) as u8,
            ]);
        }
    }
    board.write(SOURCE, &source).unwrap();

    // The job words: format 0x100, sizes (lines - 1) << 16 | (pixels - 1),
    // the still capture's halving steps 0x11000, addresses and pitches, then
    // the start +0x04 = 1.
    let words: [(u32, u32); 10] = [
        (0x008, 0x100),
        (0x00c, 0x0007_001f),
        (0x018, 0x0003_000f),
        (0x01c, 0x0001_1000),
        (0x020, 0x0001_1000),
        (0x0c4, SOURCE as u32),
        (0x0d0, 64),
        (0x0d8, OUTPUT as u32),
        (0x0e4, 32),
        (0x004, 1),
    ];
    let table: Vec<u8> = words
        .iter()
        .flat_map(|&(o, v)| [o.to_le_bytes(), v.to_le_bytes()])
        .flatten()
        .collect();
    board.write(TABLE, &table).unwrap();

    // Store each word, poll the status +0x10c for bit 0 (done), keep it,
    // acknowledge with 1 and read the status again.
    let program: [u32; 14] = [
        0xe491_3004, // LDR r3, [r1], #4
        0xe491_4004, // LDR r4, [r1], #4
        0xe780_4003, // STR r4, [r0, r3]
        0xe252_2001, // SUBS r2, r2, #1
        0x1aff_fffa, // BNE the first load
        0xe590_510c, // LDR r5, [r0, #0x10c]
        0xe315_0001, // TST r5, #1
        0x0aff_fffc, // BEQ the load above
        0xe586_5000, // STR r5, [r6]
        0xe3a0_7001, // MOV r7, #1
        0xe580_710c, // STR r7, [r0, #0x10c]
        0xe590_510c, // LDR r5, [r0, #0x10c]
        0xe586_5004, // STR r5, [r6, #4]
        0xeaff_fffe, // B .
    ];
    let bytes: Vec<u8> = program.iter().flat_map(|w| w.to_le_bytes()).collect();
    board.write(CODE, &bytes).unwrap();
    for (register, value) in [
        (0, RESZ),
        (1, TABLE as u32),
        (2, words.len() as u32),
        (6, RESULT as u32),
    ] {
        board
            .write_register(0, arm::CORE[register], value.into())
            .unwrap();
    }
    board.run_board(4000, 100).unwrap();
    assert!(board.state(false).failure.is_none());

    assert_eq!(word(&board, RESULT), 0x0100_0101, "done");
    assert_eq!(word(&board, RESULT + 4), 0x0100_0100, "acknowledged");
    // 16x4 output: every second pixel of every second line.
    let mut out = vec![0; 32 * 4];
    board.read(OUTPUT, &mut out).unwrap();
    for (i, quad) in out.chunks(4).enumerate() {
        let (pair, y) = (i % 8, i / 8);
        let luma = |x: usize| (2 * x + 16 * y) as u8;
        assert_eq!(
            quad,
            [
                luma(pair * 2),
                luma(pair * 2 + 1),
                (pair * 2) as u8,
                (0x80 + 2 * y) as u8
            ],
            "pair {pair} line {y}"
        );
    }
    // The acknowledge dropped the level line.
    let pending = &board.interrupt_state().unwrap()[0].pending;
    assert_eq!(pending[(IRQ / 32) as usize] & 1 << (IRQ % 32), 0);
}
