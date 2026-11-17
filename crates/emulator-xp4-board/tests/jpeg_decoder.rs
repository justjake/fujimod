//! The XP4 JPEG decoder (X-T3 5.11: page 0xfffa1000, interrupt 136): wiring
//! admission, and one decode driven by guest stores and loads the way the
//! firmware's playback driver drives it.
use fujimod_emulator_cpu::arm;
use fujimod_emulator_image_codecs::jpeg::Codec;
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

const CBAR: u32 = 0x2000_0000;
const DECODER: u32 = 0xfffa_1000;
const IRQ: u32 = 136;
const CODE: usize = 0x1000;
const RESULT: usize = 0x2000;
const STREAM: usize = 0x4000;
const OUTPUT: usize = 0x8000;

fn input() -> Value {
    json!({"ram_size": 65536, "cores": 1, "discovery": false, "cbar": CBAR,
        "initial_registers": [[["Pc", CODE]]]})
}

fn decoder(base: u32, irq: u32, run_ticks: u64) -> Value {
    json!({"base": base, "irq": irq, "run_ticks": run_ticks})
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
fn the_decoder_is_explicit_and_owns_its_page() {
    let absent = session(input()).unwrap();
    assert!(!absent.device_state_json().contains_key("xp4-jpeg-decoder"));
    let mut selected = input();
    selected["jpeg_decoder"] = decoder(DECODER, IRQ, 8);
    let board = session(selected.clone()).unwrap();
    assert!(board.device_state_json().contains_key("xp4-jpeg-decoder"));
    for address in [DECODER, DECODER + 0x118, DECODER + 0xffc] {
        let mut conflict = selected.clone();
        conflict["control_requests"] = json!([[address, 4]]);
        let error = session(conflict).err().unwrap();
        assert!(error.contains("Overlapping"), "{error}");
    }
    let mut extra = selected.clone();
    extra["jpeg_decoder"]["extra"] = json!(true);
    assert!(serde_json::from_value::<Definition>(extra).is_err());
    for (base, irq, run_ticks) in [
        (DECODER + 4, IRQ, 8),
        (DECODER, 31, 8),
        (DECODER, 512, 8),
        (DECODER, IRQ, 0),
    ] {
        let mut invalid = input();
        invalid["jpeg_decoder"] = decoder(base, irq, run_ticks);
        let error = session(invalid).err().unwrap();
        assert!(error.contains("Invalid JPEG decoder"), "{error}");
    }
}

#[test]
fn a_guest_decode_completes_on_the_declared_interrupt_line() {
    let mut selected = input();
    selected["jpeg_decoder"] = decoder(DECODER, IRQ, 8);
    let board = session(selected).unwrap();

    // A 32x16 packed 4:2:2 picture whose luma rises by 8 per pixel.
    let mut codec = Codec::default();
    codec.tables(&[1; 128]).unwrap();
    let source: Vec<u8> = (0..32 * 16 / 2)
        .flat_map(|i| {
            let x = (i % 16 * 2) as u8;
            [x * 8, (x + 1) * 8, 100, 160]
        })
        .collect();
    let mut body = Vec::new();
    codec.encode(&source, 32, 16, 0, &mut body, 0x2000).unwrap();
    let mut stream = vec![0xff, 0xd8];
    stream.extend(body);
    board.write(STREAM, &stream).unwrap();

    // The driver's sequence: initialise through +0x118 and read the status
    // (+0x04, bit 16 = initialised), program the stream (+0x10c, +0x110),
    // the output and its limit (+0x100, +0x104), start with +0x00 bit 0, poll
    // the status for bit 4 (done) and read the output end at +0x108.
    let program: [u32; 15] = [
        0xe580_1118, // STR r1, [r0, #0x118]
        0xe590_2004, // LDR r2, [r0, #4]
        0xe583_2000, // STR r2, [r3]
        0xe580_410c, // STR r4, [r0, #0x10c]
        0xe580_5110, // STR r5, [r0, #0x110]
        0xe580_6100, // STR r6, [r0, #0x100]
        0xe580_7104, // STR r7, [r0, #0x104]
        0xe580_1000, // STR r1, [r0]
        0xe590_2004, // LDR r2, [r0, #4]
        0xe312_0010, // TST r2, #0x10
        0x0aff_fffc, // BEQ the load above
        0xe583_2004, // STR r2, [r3, #4]
        0xe590_2108, // LDR r2, [r0, #0x108]
        0xe583_2008, // STR r2, [r3, #8]
        0xeaff_fffe, // B .
    ];
    let bytes: Vec<u8> = program.iter().flat_map(|w| w.to_le_bytes()).collect();
    board.write(CODE, &bytes).unwrap();
    for (register, value) in [
        (0, DECODER),
        (1, 1),
        (3, RESULT as u32),
        (4, STREAM as u32),
        (5, stream.len() as u32),
        (6, OUTPUT as u32),
        (7, 0x1000),
    ] {
        board
            .write_register(0, arm::CORE[register], value.into())
            .unwrap();
    }
    // The board scheduler advances time and the decoder's deadline.
    board.run_board(4000, 100).unwrap();
    assert!(board.state(false).failure.is_none());

    assert_eq!(word(&board, RESULT), 1 << 16, "initialised");
    assert_eq!(word(&board, RESULT + 4) & 0x10, 0x10, "done");
    // 32x16 pixels of two bytes, lines padded to 16 pixels.
    assert_eq!(word(&board, RESULT + 8), OUTPUT as u32 + 32 * 2 * 16);
    let mut out = vec![0; 32 * 2 * 16];
    board.read(OUTPUT, &mut out).unwrap();
    for (i, quad) in out.chunks(4).enumerate() {
        let x = (i % 16 * 2) as i32;
        assert!((i32::from(quad[0]) - x * 8).abs() <= 3, "{i} {quad:?}");
        assert!((i32::from(quad[2]) - 100).abs() <= 3 && (i32::from(quad[3]) - 160).abs() <= 3);
    }
    // The level interrupt is pending on the declared line until the status
    // is cleared.
    let pending = &board.interrupt_state().unwrap()[0].pending;
    assert_ne!(pending[(IRQ / 32) as usize] & 1 << (IRQ % 32), 0);
}
