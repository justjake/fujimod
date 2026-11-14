//! Declared XP4 movie audio (X-T3 5.11): the storage DMA's audio channels and
//! the audio format converter block are explicit, and a guest recording
//! through a linked ring from the serial audio port receives paced silence,
//! or the declared test microphone's ramp.
use fujimod_emulator_cpu::arm;
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

const CBAR: u32 = 0x2000_0000;
const CHANNEL: u32 = 0xfffe_4000;
const IRQ: u32 = 207 + 4;
const CODE: usize = 0x1000;
const DESCRIPTOR: usize = 0x2000;
const RING: usize = 0x3000;
const RESULT: usize = 0x4000;

fn storage() -> Value {
    json!({"ram_size": 65536, "cores": 1, "discovery": false, "cbar": CBAR,
        "storage_jedec": 32, "storage_geometry": [4096, 256, 8, 8], "storage_irq": 37,
        "storage_dma_routes": (207..223).collect::<Vec<_>>(),
        "initial_registers": [[["Pc", CODE]]]})
}

fn audio(channels: Value) -> Value {
    json!({"channels": channels, "bytes_per_second": 384000, "ticks_per_second": 54000000,
        "irq_at_end": true})
}

fn session(value: Value) -> Result<Session, String> {
    let definition: Definition = serde_json::from_value(value).map_err(|e| e.to_string())?;
    Session::new(definition, Some(vec![0xff; 4096]), vec![])
}

fn word(session: &Session, address: usize) -> u32 {
    let mut bytes = [0; 4];
    session.read(address, &mut bytes).unwrap();
    u32::from_le_bytes(bytes)
}

#[test]
fn the_audio_channels_are_explicit() {
    let absent = session(storage()).unwrap();
    assert!(!absent.device_state_json()["xp4-dma"].contains("\"audio\""));
    let mut selected = storage();
    selected["storage_dma_audio"] = audio(json!([4, 7, 8]));
    let board = session(selected.clone()).unwrap();
    assert!(board.device_state_json()["xp4-dma"].contains("\"audio\""));
    let mut extra = selected.clone();
    extra["storage_dma_audio"]["extra"] = json!(true);
    assert!(serde_json::from_value::<Definition>(extra).is_err());
    for channels in [json!([]), json!([16]), json!([4, 4])] {
        let mut invalid = storage();
        invalid["storage_dma_audio"] = audio(channels);
        let error = session(invalid).err().unwrap();
        assert!(error.contains("Invalid XP4 DMA audio channels"), "{error}");
    }
    let mut alone = json!({"ram_size": 65536, "cores": 1, "discovery": false, "cbar": CBAR});
    alone["storage_dma_audio"] = audio(json!([4]));
    let error = session(alone).err().unwrap();
    assert!(
        error.contains("require the XP4 storage controller"),
        "{error}"
    );
}

#[test]
fn the_audio_converter_is_an_explicit_block_job() {
    let block = |converter: bool, job: bool| {
        let mut b = json!({"base": 0xfffa_8000u32, "size": 4096, "idle": [[0, 1]],
            "audio_converter": converter});
        if job {
            b["job"] = json!({"start": [4, 1], "irq": 144, "ticks": 10});
        }
        let mut v = storage();
        v["xpp_register_blocks"] = json!([b]);
        session(v)
    };
    assert!(block(false, true).is_ok());
    assert!(block(true, true).is_ok());
    assert!(block(true, false).is_err());
}

/// Records two linked 16-byte blocks from 0xfff4f000 into RING with guest
/// stores and loads, and checks that only the end of the list raised the
/// channel interrupt.
fn record(audio: Value) -> Session {
    let mut selected = storage();
    selected["storage_dma_audio"] = audio;
    let board = session(selected).unwrap();
    // A terminal descriptor after the first block; the ring holds a marker.
    let next: [u32; 6] = [15, 0xfff4_f000, RING as u32 + 16, 0x20f04, 0x20f00, 0];
    let bytes: Vec<u8> = next.iter().flat_map(|w| w.to_le_bytes()).collect();
    board.write(DESCRIPTOR, &bytes).unwrap();
    board.write(RING, &[0xaa; 48]).unwrap();

    // The driver's order: size - 1, fixed source (serial audio port 0
    // receive), incremented destination, source and destination
    // configuration, the linked next descriptor, start; then poll +0x30
    // bit 0 and read where the destination has got to.
    let program: [u32; 13] = [
        0xe580_1010, // STR r1, [r0, #0x10]
        0xe580_2014, // STR r2, [r0, #0x14]
        0xe580_3018, // STR r3, [r0, #0x18]
        0xe580_401c, // STR r4, [r0, #0x1c]
        0xe580_5020, // STR r5, [r0, #0x20]
        0xe580_6024, // STR r6, [r0, #0x24]
        0xe580_7028, // STR r7, [r0, #0x28]
        0xe590_8030, // LDR r8, [r0, #0x30]
        0xe318_0001, // TST r8, #1
        0x0aff_fffc, // BEQ the load above
        0xe590_9018, // LDR r9, [r0, #0x18]
        0xe58a_9000, // STR r9, [r10]
        0xeaff_fffe, // B .
    ];
    let code: Vec<u8> = program.iter().flat_map(|w| w.to_le_bytes()).collect();
    board.write(CODE, &code).unwrap();
    for (register, value) in [
        (0, CHANNEL),
        (1, 15),
        (2, 0xfff4_f000),
        (3, RING as u32),
        (4, 0x20f04),
        (5, 0x20f00),
        (6, DESCRIPTOR as u32 | 1),
        (7, 0x1020_1011),
        (10, RESULT as u32),
    ] {
        board
            .write_register(0, arm::CORE[register], value.into())
            .unwrap();
    }
    // Two 16-byte blocks at 384 000 bytes/s take 4 500 ticks.
    board.run_board(20_000, 100).unwrap();
    assert!(board.state(false).failure.is_none());

    assert_eq!(
        word(&board, RESULT),
        RING as u32 + 32,
        "both blocks ended the list"
    );
    // Only the end of the list raised the channel interrupt.
    let pending = &board.interrupt_state().unwrap()[0].pending;
    assert_ne!(pending[(IRQ / 32) as usize] & 1 << (IRQ % 32), 0);
    let dma = &board.device_state_json()["xp4-dma"];
    assert!(dma.contains("\"completed\":2"), "{dma}");
    board
}

#[test]
fn a_guest_records_paced_silence_through_a_linked_ring() {
    let board = record(audio(json!([4])));
    let mut ring = [0; 48];
    board.read(RING, &mut ring).unwrap();
    assert_eq!(&ring[..32], &[0; 32]);
    assert_eq!(&ring[32..], &[0xaa; 16]);
    let dma = &board.device_state_json()["xp4-dma"];
    assert!(!dma.contains("microphone_frames"), "{dma}");
}

#[test]
fn a_guest_records_the_declared_test_microphone_ramp() {
    // Two 24-bit ramps, left-justified in 32-bit slots: the first slot
    // counts from 1, the second from 0xa00000.
    let mut declared = audio(json!([4]));
    declared["microphone"] =
        json!({"fifo": 0xfff4_f000u32, "slots": [[1, 1], [0xa0_0000, 1]], "shift": 8});
    let board = record(declared);
    for frame in 0..4 {
        let at = RING + frame * 8;
        let n = frame as u32;
        assert_eq!(
            (word(&board, at), word(&board, at + 4)),
            ((1 + n) << 8, (0xa0_0000 + n) << 8),
            "frame {frame}"
        );
    }
    assert_eq!(word(&board, RING + 32), 0xaaaa_aaaa);
    let dma = &board.device_state_json()["xp4-dma"];
    assert!(dma.contains("\"microphone_frames\":4"), "{dma}");

    // An invalid test microphone is refused when the board is built.
    let mut invalid = storage();
    invalid["storage_dma_audio"] = audio(json!([4]));
    invalid["storage_dma_audio"]["microphone"] =
        json!({"fifo": 0x1000, "slots": [[0, 1]], "shift": 8});
    let error = session(invalid).err().unwrap();
    assert!(error.contains("Invalid XP4 DMA test microphone"), "{error}");
}
