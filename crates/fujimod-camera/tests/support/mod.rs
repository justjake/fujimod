//! Inputs the FujiMod camera tests share.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a firmware library. A test
//! returns without opening a camera when it is absent. Every test uses a
//! scratch state directory under `/tmp`.
#![allow(dead_code)]

use fujimod_camera::{
    BootCache, CardSource, ExecutionPolicy, FirmwareInput, Library, Preset, Request, StateDir,
};
use fujimod_recovery::Recipe;

pub const FIRMWARE: &str = "x70-110";

pub fn library() -> Option<Library> {
    let library = Library::from_env().unwrap_or_else(|error| panic!("firmware library: {error}"));
    library
        .firmwares()
        .iter()
        .any(|firmware| firmware.id == FIRMWARE)
        .then_some(library)
}

/// A scratch state directory for this test process.
pub fn state() -> StateDir {
    StateDir::at(std::env::temp_dir().join(format!("fujimod-camera-test-{}", std::process::id())))
}

/// One FujiMod open with a caller-chosen preset and card.
pub fn request(
    firmware: &str,
    preset: Preset,
    sd_card: Option<CardSource>,
    cache: BootCache,
    run_to_rec: bool,
) -> Request {
    Request {
        firmware: firmware.into(),
        files: FirmwareInput::Library,
        overlays: Vec::new(),
        preset,
        sd_card,
        policy: ExecutionPolicy::Interactive,
        engine: None,
        cache,
        run_to_rec,
        card: Default::default(),
        nand_image: None,
        face_result_profile: None,
    }
}

/// The published release SD card, downloaded once into the state directory.
pub fn release(state: &StateDir, library: &Library, firmware: &str) -> CardSource {
    let directory = fujimod_camera::release_card(state, library, firmware)
        .unwrap_or_else(|error| panic!("{firmware}: release card: {error}"));
    CardSource::Directory(directory)
}

/// Stage 3 runs at `0x1d020000` and writes its marker here; stage 2 writes the
/// boot record below it.
pub const MARKER_ADDRESS: u64 = 0x1d03_ff00;
pub const STAGE3_MARKER: u32 = 0x5a17_c0de;
/// The store hash and file name a one-entry release manifest names.
const STORE_HASH: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const STORE_PATH: &str = "FUJIMOD/SYSTEM/STORE/AAAAAAAA.AAA";

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

fn stage3_program() -> Vec<u8> {
    STAGE3.iter().flat_map(|word| word.to_le_bytes()).collect()
}

/// The release identity and its 8.3 folder, as the release manifest and stage 2
/// derive them: the firmware id uppercased, hyphens removed past eight bytes.
pub fn release_folder(firmware: &str) -> String {
    let identity = firmware.to_ascii_uppercase();
    if identity.len() <= 8 {
        identity
    } else {
        identity.replace('-', "")
    }
}

/// The card `sd-boot-v2-sd-card` builds, plus a one-entry release manifest and
/// the stage 3 marker program.
pub fn built_card(library: &Library, firmware: &str) -> Vec<(String, Vec<u8>)> {
    let recipe = Recipe::bundled(firmware)
        .unwrap_or_else(|error| panic!("{firmware}: recipe: {error}"))
        .unwrap_or_else(|| panic!("{firmware}: no SD card recipe"));
    let update = library
        .files(firmware)
        .unwrap_or_else(|error| panic!("{firmware}: library files: {error}"))
        .update;
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

/// Press a control and wait for the firmware to confirm it.
pub fn press(opened: &mut fujimod_camera::Opened, name: &str) {
    let facts = fujimod_emulator_camera::remote_facts(&opened.resolved).expect("remote facts");
    let quantum = opened.camera.camera.quantum;
    let ticks_per_second = opened.camera.camera.ticks_per_second;
    let session = opened.session_mut();
    let options =
        fujimod_emulator_camera::remote_options(session, quantum).expect("remote options");
    let deadline = session.board.now() + 5 * ticks_per_second;
    let mut remote = fujimod_remote::BoardRemote::new(session, facts, options).expect("remote");
    futures_lite::future::block_on(fujimod_remote::operations::press(
        &mut remote,
        &fujimod_remote::Control::new(name),
        deadline,
    ))
    .unwrap_or_else(|error| panic!("press {name}: {error}"));
}
