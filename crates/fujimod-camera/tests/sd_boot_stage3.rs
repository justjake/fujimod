//! Boot a body's catalog SD card combination to real stage 3.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a library laid out by name. Without it
//! each test returns without opening a camera.
//!
//! The card is the one `sd-boot-v2-sd-card` builds, plus a one-entry release
//! manifest and a stage 3 program that writes its marker. The boot stops at
//! the boot ROM, runs until stage 3 writes the marker, then reads the record
//! stage 2 wrote and the card's own release state. REC is not involved.
use fujimod_camera::{
    BootCache, CardSource, ExecutionPolicy, FirmwareInput, Library, LibraryError, Opened, Preset,
    Request, Stage, StateDir, open,
};
use fujimod_emulator_session::Session;
use fujimod_recovery::Recipe;

/// Stage 3 runs at `0x1d020000` and writes its marker here; stage 2 writes the
/// boot record below it.
const MARKER_ADDRESS: u64 = 0x1d03_ff00;
const RECORD_ADDRESS: u64 = 0x1d01_ff00;
const RECORD_BYTES: usize = 44;
const STAGE3_MARKER: u32 = 0x5a17_c0de;
/// The store hash and file name a one-entry release manifest names.
const STORE_HASH: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const STORE_PATH: &str = "FUJIMOD/SYSTEM/STORE/AAAAAAAA.AAA";
/// One instruction window; the Python check uses the same 5M window.
const WINDOW: u64 = 5_000_000;
const WINDOWS: usize = 150;

/// `ldr r0, =0x1d03ff00; ldr r1, =0x5a17c0de; str r1, [r0]; bx lr` plus its
/// literal pool, as the stage 3 marker test program assembles.
const STAGE3: [u32; 6] = [
    0xe59f_0008,
    0xe59f_1008,
    0xe580_1000,
    0xe12f_ff1e,
    0x1d03_ff00,
    0x5a17_c0de,
];

fn library() -> Option<Library> {
    match Library::from_env() {
        Ok(library) => Some(library),
        Err(LibraryError::Environment) => None,
        Err(error) => panic!("firmware library: {error}"),
    }
}

fn state() -> StateDir {
    StateDir::at(std::env::temp_dir().join(format!("fujimod-stage3-{}", std::process::id())))
}

/// The release identity and its 8.3 folder, as the release manifest and stage 2
/// derive them: the firmware id uppercased, hyphens removed past eight bytes.
fn release_folder(firmware: &str) -> String {
    let identity = firmware.to_ascii_uppercase();
    if identity.len() <= 8 {
        identity
    } else {
        identity.replace('-', "")
    }
}

fn stage3_program() -> Vec<u8> {
    STAGE3.iter().flat_map(|word| word.to_le_bytes()).collect()
}

/// The card's boot files, its release manifest and the stage 3 marker program.
fn card_files(library: &Library, firmware: &str) -> Vec<(String, Vec<u8>)> {
    let recipe = Recipe::bundled(firmware)
        .unwrap_or_else(|error| panic!("{firmware}: recipe: {error}"))
        .unwrap_or_else(|| panic!("{firmware}: no SD card recipe"));
    let update = std::fs::read(
        library
            .files(firmware)
            .unwrap_or_else(|error| panic!("{firmware}: library files: {error}"))
            .update,
    )
    .expect("read stock update");
    let mut files: Vec<(String, Vec<u8>)> = recipe
        .build_ota_startup(&update)
        .expect("build the SD card entry")
        .into_iter()
        .map(|file| (file.name, file.bytes))
        .collect();
    let folder = release_folder(firmware);
    let manifest = format!(
        "{{\"gen\":1,\"version\":\"0.1.0\",\"fs\":{{\"/boot/fujimod.bin\":\"{STORE_HASH}\"}}}}"
    );
    files.push((
        format!("FUJIMOD/SYSTEM/{folder}/A.JSN"),
        manifest.into_bytes(),
    ));
    files.push((STORE_PATH.into(), stage3_program()));
    files
}

fn request(firmware: &str, files: Vec<(String, Vec<u8>)>) -> Request {
    Request {
        firmware: firmware.into(),
        files: FirmwareInput::Library,
        preset: Preset::SdCard,
        sd_card: Some(CardSource::Files(files)),
        policy: ExecutionPolicy::Interactive,
        engine: None,
        cache: BootCache::Bypass,
        run_to_rec: false,
        card: Default::default(),
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
        request(firmware, card_files(&library, firmware)),
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
