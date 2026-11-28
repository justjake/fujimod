//! Boot a body's catalog SD card combination to real stage 3.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a firmware library. Without it
//! each test returns without opening a camera.
//!
//! The card is the one `sd-boot-v2-sd-card` builds, plus a one-entry release
//! manifest and a stage 3 program that writes its marker. The boot stops at
//! the boot ROM, runs until stage 3 writes the marker, then reads the record
//! stage 2 wrote and the card's own release state. REC is not involved.
use crate::support;
use support::{MARKER_ADDRESS, STAGE3_MARKER, built_card, library, release_folder, state};

use fujimod_camera::{
    BootCache, CardSource, ExecutionPolicy, FirmwareInput, Opened, Preset, Request, Stage, open,
};
use fujimod_emulator_session::Session;

const RECORD_ADDRESS: u64 = 0x1d01_ff00;
const RECORD_BYTES: usize = 44;
/// One instruction window; the Python check uses the same 5M window.
const WINDOW: u64 = 5_000_000;
const WINDOWS: usize = 150;

fn request(firmware: &str, files: Vec<(String, Vec<u8>)>) -> Request {
    Request {
        firmware: firmware.into(),
        files: FirmwareInput::Library,
        overlays: Vec::new(),
        preset: Preset::SdCard,
        sd_card: Some(CardSource::Files(files)),
        policy: ExecutionPolicy::Interactive,
        engine: None,
        cache: BootCache::Bypass,
        run_to_rec: false,
        card: Default::default(),
        nand_image: None,
        face_result_profile: None,
        camera_settings: Default::default(),
    }
}

fn word(session: &Session, address: u64) -> u32 {
    let mut bytes = [0u8; 4];
    session
        .board
        .read_physical(address, &mut bytes)
        .expect("physical read");
    u32::from_le_bytes(bytes)
}

/// Run the held boot until a real stage 3 store writes the marker.
fn run_to_stage3(opened: &mut Opened) {
    let quantum = opened.camera.camera.quantum;
    opened
        .session_mut()
        .board
        .watch_writes(&[(MARKER_ADDRESS, 4)])
        .expect("arm the stage 3 marker watch");
    for window in 0..WINDOWS {
        if let Err(error) = opened
            .session_mut()
            .board
            .run_for_instructions(WINDOW, quantum)
        {
            panic!("board stopped in window {window}: {error}");
        }
        let (notes, _) = opened.session().board.take_write_notes();
        if notes
            .iter()
            .any(|note| note.address == MARKER_ADDRESS && note.value == STAGE3_MARKER.to_le_bytes())
        {
            return;
        }
    }
    panic!("stage 3 never wrote its marker");
}

fn decode_record(data: &[u8; RECORD_BYTES]) -> (u32, u32, u32, u32, u32, Vec<u8>, u32, u32) {
    let word = |offset: usize| {
        u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ])
    };
    let (magic, version, size, generation, slot, chained) =
        (word(0), word(4), word(8), word(12), word(16), word(20));
    let mut value = 2166136261u32;
    for word in [magic, version, size, generation, slot, chained] {
        value = (value ^ word).wrapping_mul(16777619);
    }
    for byte in &data[24..40] {
        value = (value ^ u32::from(*byte)).wrapping_mul(16777619);
    }
    let firmware = data[24..40]
        .split(|byte| *byte == 0)
        .next()
        .unwrap_or_default()
        .to_vec();
    (
        magic,
        version,
        size,
        generation,
        slot,
        firmware,
        word(40),
        value,
    )
}

fn check_boot(firmware: &str) {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY to a firmware library");
        return;
    };
    let mut opened = open(
        &library,
        &state(),
        request(firmware, built_card(&library, firmware)),
    )
    .unwrap_or_else(|error| panic!("{firmware}: open: {error}"));
    assert_eq!(
        opened.stage,
        Stage::BootRom,
        "{firmware}: stopped before REC"
    );
    assert!(
        opened.holds_applied(),
        "{firmware}: the SD combination is held"
    );
    let held: Vec<&str> = opened.holds.controls().collect();
    eprintln!("{firmware}: holds {held:?}");
    assert!(
        !held.is_empty(),
        "{firmware}: the catalog holds a boot combination"
    );
    run_to_stage3(&mut opened);
    let session = opened.session();
    let mut record = [0u8; RECORD_BYTES];
    session
        .board
        .read_physical(RECORD_ADDRESS, &mut record)
        .expect("boot record");
    let (magic, version, size, generation, slot, record_firmware, check, expected) =
        decode_record(&record);
    let identity = firmware.to_ascii_uppercase();
    assert_eq!(
        record_firmware,
        identity.as_bytes(),
        "{firmware}: record identity"
    );
    assert_eq!(slot, 0, "{firmware}: record slot");
    assert_eq!(generation, 1, "{firmware}: record generation");
    assert_eq!(check, expected, "{firmware}: record check");
    assert_eq!(magic, 0x5343_3242, "{firmware}: record magic");
    assert_eq!(
        (version, size),
        (1, RECORD_BYTES as u32),
        "{firmware}: record layout"
    );
    assert_eq!(
        word(session, MARKER_ADDRESS),
        STAGE3_MARKER,
        "{firmware}: marker"
    );
    let name = session
        .contacts
        .keys()
        .next()
        .expect("SD card slot")
        .clone();
    let path = format!("FUJIMOD/SYSTEM/{}/STATE.JSN", release_folder(firmware));
    let state: serde_json::Value = serde_json::from_slice(
        &session
            .card_read(&name, &path, None, 1 << 20)
            .unwrap_or_else(|error| panic!("{firmware}: {path}: {error}")),
    )
    .expect("STATE.JSN json");
    assert_eq!(
        state["a"],
        serde_json::json!({"gen": 1, "attempts": 1, "confirmed": false}),
        "{firmware}: release A attempt"
    );
    assert_eq!(
        state["last_boot"]["stage1"], "sd-card",
        "{firmware}: the boot started on the card"
    );
    eprintln!(
        "{firmware}: stage 3 marker {STAGE3_MARKER:#x}, record {record_firmware:?} gen {generation} slot {slot}"
    );
}

#[test]
fn xm1_sd_card_reaches_stage3() {
    check_boot("xm1-150");
}

#[test]
fn xe2_sd_card_reaches_stage3() {
    check_boot("xe2-411");
}

#[test]
fn xe2s_sd_card_reaches_stage3() {
    check_boot("xe2s-111");
}

#[test]
fn xt10_sd_card_reaches_stage3() {
    check_boot("xt10-131");
}
