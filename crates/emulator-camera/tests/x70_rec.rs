//! Open the installed X70 firmware and the release SD card at REC, then press a
//! control through `fujimod-remote`.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a directory holding the catalog's X70
//! update and boot ROM, and `FUJIMOD_TEST_SD_CARD` to a directory holding the
//! release SD card. Without both, the test returns without opening a camera.
use fujimod_catalog_remote::{Facts, Selection};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_boot as boot;
use fujimod_emulator_camera::{
    Boot, BootCache, CardSource, ExecutionPolicy, Library, LibraryError, Opened, Request, Stage,
    open,
};
use fujimod_remote::{BoardRemote, Control, board::Options as RemoteOptions, operations};
use std::path::PathBuf;

const FIRMWARE: &str = "x70-110";
const BUTTON: &str = "menu_ok_button";
/// The SD card loader's stage 3 is linked at this guest address.
const STAGE3: u64 = 0x1d02_0000;

fn library() -> Option<Library> {
    match Library::from_env() {
        Ok(library) => Some(library),
        Err(LibraryError::Environment) => None,
        Err(error) => panic!("firmware library: {error}"),
    }
}

fn request(boot: Boot) -> Request {
    Request {
        firmware: FIRMWARE.into(),
        boot,
        policy: ExecutionPolicy::Interactive,
        engine: None,
        card_files: Vec::new(),
        cache: BootCache::Bypass,
    }
}

/// The catalog facts `fujimod-remote` needs for a resolved selection.
fn remote_facts(resolved: &ResolvedConfiguration) -> Result<Facts, String> {
    let selection = &resolved.reference;
    let input = Selection {
        firmware_id: &selection.firmware.id,
        body_id: &selection.body.id,
        firmware_sha256: selection
            .firmware
            .update
            .as_ref()
            .map(|content| content.sha256.as_str()),
        remote_facts: selection
            .firmware
            .generated_facts
            .as_ref()
            .and_then(|facts| facts.remote.as_ref()),
        physical_controls: selection.physical_controls.as_ref(),
        control_ids: selection.control_ids.as_ref(),
        connections: selection
            .body
            .connections
            .iter()
            .map(|connection| connection.id.as_str())
            .collect(),
    };
    Facts::from_selection(&input, None)
}

/// Press a control and wait for the firmware to confirm it.
fn press(opened: &mut Opened, name: &str) -> Result<(), String> {
    let facts = remote_facts(&opened.resolved)?;
    let quantum = opened.camera.quantum;
    let ticks_per_second = opened.camera.ticks_per_second;
    let session = &mut opened.camera.session;
    let options = RemoteOptions {
        quantum,
        window_instructions: 100_000,
        scanout_node: boot::scanout_node(session)?,
        output: "lcd".into(),
        // The EXR II display-timing model counts completed LCD frames here.
        frame_counter_kind: "display-timing".into(),
        frame_counter_pointer: "/0/banks/1/frames".into(),
    };
    let deadline = session.board.now() + 5 * ticks_per_second;
    let mut remote = BoardRemote::new(session, facts, options)?;
    futures_lite::future::block_on(operations::press(
        &mut remote,
        &Control::new(name),
        deadline,
    ))
}

fn open_and_press(library: &Library, name: &str, boot: Boot) -> Opened {
    let started = std::time::Instant::now();
    let mut opened = open(library, request(boot)).unwrap_or_else(|error| panic!("{name}: {error}"));
    assert_eq!(opened.stage, Stage::Rec, "{name} did not report REC");
    let report =
        boot::status(opened.session()).unwrap_or_else(|error| panic!("{name}: status: {error}"));
    assert_eq!(report.status, "rec", "{name} did not reach REC");
    press(&mut opened, BUTTON).unwrap_or_else(|error| panic!("{name}: press {BUTTON}: {error}"));
    let elapsed = started.elapsed();
    eprintln!("{name}: REC and {BUTTON} in {elapsed:?}");
    assert!(
        elapsed < std::time::Duration::from_secs(60),
        "{name} open and press took {elapsed:?}"
    );
    opened
}

#[test]
fn stock_and_sd_card_open_to_rec_and_press_menu_ok() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY and FUJIMOD_TEST_SD_CARD");
        return;
    };
    let Some(card) = std::env::var_os("FUJIMOD_TEST_SD_CARD") else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY and FUJIMOD_TEST_SD_CARD");
        return;
    };
    open_and_press(&library, "stock", Boot::Stock);
    let opened = open_and_press(
        &library,
        "SD card",
        Boot::SdCard(CardSource::Directory(PathBuf::from(card))),
    );
    let mut stage3 = [0u8; 4];
    opened
        .session()
        .board
        .read_physical(STAGE3, &mut stage3)
        .unwrap();
    assert_ne!(
        stage3, [0xff; 4],
        "the SD card's stage 3 loader is not resident at {STAGE3:#x}"
    );
    eprintln!("SD card: stage 3 at {STAGE3:#x} is {stage3:02x?}");
}
