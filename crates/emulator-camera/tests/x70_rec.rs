//! Open the installed X70 firmware at REC, then press a control through
//! `fujimod-remote`.
//!
//! The test needs a firmware library. Set `FUJIMOD_FIRMWARE_LIBRARY` to a
//! directory holding the catalog's X70 update and boot ROM. Without it, the
//! test returns without opening a camera.
use fujimod_catalog_remote::{Facts, Selection};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_boot as boot;
use fujimod_emulator_camera::{
    Boot, ExecutionPolicy, Library, LibraryError, Opened, Request, open,
};
use fujimod_remote::{BoardRemote, Control, board::Options as RemoteOptions, operations};

const FIRMWARE: &str = "x70-110";
const BUTTON: &str = "menu_ok_button";

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

#[test]
fn stock_opens_to_rec_and_presses_menu_ok() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let started = std::time::Instant::now();
    let mut opened =
        open(&library, request(Boot::Stock)).unwrap_or_else(|error| panic!("stock: {error}"));
    let report =
        boot::status(opened.session()).unwrap_or_else(|error| panic!("stock: status: {error}"));
    assert_eq!(report.status, "rec", "stock did not reach REC");
    press(&mut opened, BUTTON).unwrap_or_else(|error| panic!("stock: press {BUTTON}: {error}"));
    let elapsed = started.elapsed();
    eprintln!("stock: REC and {BUTTON} in {elapsed:?}");
    assert!(
        elapsed < std::time::Duration::from_secs(60),
        "stock open and press took {elapsed:?}"
    );
}
