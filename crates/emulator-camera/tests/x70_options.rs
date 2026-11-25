//! X70 tests for opening before REC and for the SD card's model options.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a directory holding the catalog's X70
//! update and boot ROM, and `FUJIMOD_TEST_SD_CARD` to a directory holding the
//! release SD card. A test whose inputs are absent returns without opening.
use fujimod_emulator_camera::{
    Boot, BootCache, CardModel, CardSource, ExecutionPolicy, Library, LibraryError, Opened,
    Request, Stage, open, open_unbooted, remote_facts, remote_options,
};
use fujimod_remote::{BoardRemote, Control, operations};
use std::path::PathBuf;

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
        cache: BootCache::Bypass,
        run_to_rec: true,
        card: CardModel::default(),
        kept: Default::default(),
        card_source: None,
        image: None,
    }
}

fn release_card() -> Option<PathBuf> {
    std::env::var_os("FUJIMOD_TEST_SD_CARD").map(PathBuf::from)
}

fn sd_node(opened: &Opened) -> serde_json::Value {
    opened.session().definition["devices"]["nodes"]
        .as_array()
        .expect("device node list")
        .iter()
        .find(|node| node["name"] == "sd")
        .expect("SD card node")
        .clone()
}

fn positions(opened: &Opened) -> std::collections::BTreeMap<String, String> {
    opened
        .session()
        .control_state()
        .expect("control state")
        .positions
}

/// Press a control and wait for the firmware's own observation of it.
fn press(opened: &mut Opened, name: &str) {
    let facts = remote_facts(&opened.resolved).expect("remote facts");
    let session = &mut opened.camera.session;
    let options = remote_options(session, opened.camera.quantum).expect("remote options");
    let deadline = session.board.now() + 5 * opened.camera.ticks_per_second;
    let mut remote = BoardRemote::new(session, facts, options).expect("remote");
    futures_lite::future::block_on(operations::press(
        &mut remote,
        &Control::new(name),
        deadline,
    ))
    .unwrap_or_else(|error| panic!("press {name}: {error}"));
}

#[test]
fn a_held_sd_card_boot_stops_before_the_first_instruction() {
    let (Some(library), Some(card)) = (library(), release_card()) else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY and FUJIMOD_TEST_SD_CARD");
        return;
    };
    let mut request = request(Boot::SdCard(CardSource::Directory(card)));
    request.run_to_rec = false;
    let mut opened = open(&library, request).expect("open before the first instruction");
    assert_eq!(opened.stage, Stage::BootRom);
    assert!(opened.holds_applied());
    let held: Vec<&str> = opened.holds.controls().collect();
    assert_eq!(
        held,
        ["command_control.press", "movie_record_button"],
        "the catalog's boot.sd holds"
    );
    let report = fujimod_emulator_boot::status(opened.session()).expect("status");
    assert_eq!(report.instructions, 0, "the guest has not run");
    assert_eq!(report.virtual_ticks, 0, "guest time has not advanced");
    assert_ne!(report.status, "rec");
    let held_positions = positions(&opened);
    assert_eq!(
        held_positions
            .get("command_control.press")
            .map(String::as_str),
        Some("pressed")
    );
    assert_eq!(
        held_positions
            .get("movie_record_button")
            .map(String::as_str),
        Some("pressed")
    );

    let report = fujimod_emulator_boot::to_rec(&mut opened.camera.session, 2_000_000_000)
        .expect("the caller runs the boot");
    assert_eq!(report.status, "rec");
    let still_held = positions(&opened);
    assert_eq!(
        still_held.get("movie_record_button").map(String::as_str),
        Some("pressed"),
        "the caller releases the holds"
    );
    opened.release_holds().expect("release the boot holds");
    assert!(!opened.holds_applied());
    let released = positions(&opened);
    assert_eq!(
        released.get("command_control.press").map(String::as_str),
        Some("released")
    );
    assert_eq!(
        released.get("movie_record_button").map(String::as_str),
        Some("released")
    );
    press(&mut opened, BUTTON);
}

#[test]
fn an_absent_sd_card_boots_stock_and_is_reported_absent() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let mut request = request(Boot::Stock);
    request.card.present = false;
    let opened = open(&library, request).expect("stock boot without a card");
    assert_eq!(opened.stage, Stage::Rec);
    let state = opened.session().card_state("sd").expect("card state");
    assert!(!state.present);
    assert!(!state.protected);
}

#[test]
fn a_card_boot_refuses_an_absent_card() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let mut request = request(Boot::SdCard(CardSource::Files(Vec::new())));
    request.card.present = false;
    let error = match open(&library, request) {
        Err(error) => error,
        Ok(_) => panic!("a card boot opened without its card"),
    };
    assert_eq!(error.stage, Stage::BootRom);
    assert!(
        error.detail.contains("card present"),
        "unexpected refusal: {}",
        error.detail
    );
}

#[test]
fn a_write_protected_sd_card_boots_stock_and_is_reported_protected() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let mut request = request(Boot::Stock);
    request.card.protected = true;
    let opened = open(&library, request).expect("stock boot with a protected card");
    assert_eq!(opened.stage, Stage::Rec);
    let state = opened.session().card_state("sd").expect("card state");
    assert!(state.present);
    assert!(state.protected);
}

#[test]
fn sd_card_transfer_ticks_replace_the_catalog_value() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let catalog = open_unbooted(&library, request(Boot::Stock)).expect("unbooted stock camera");
    assert_eq!(sd_node(&catalog)["model"]["options"]["transfer_ticks"], 100);

    let mut request = request(Boot::Stock);
    request.card.transfer_ticks = Some(17);
    let opened = open(&library, request).expect("stock boot with 17 transfer ticks");
    assert_eq!(opened.stage, Stage::Rec);
    assert_eq!(sd_node(&opened)["model"]["options"]["transfer_ticks"], 17);
}

#[test]
fn transfer_ticks_must_be_positive() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let mut request = request(Boot::Stock);
    request.card.transfer_ticks = Some(0);
    let error = match open_unbooted(&library, request) {
        Err(error) => error,
        Ok(_) => panic!("zero transfer ticks opened"),
    };
    assert_eq!(error.stage, Stage::BootRom);
    assert!(
        error.detail.contains("transfer ticks"),
        "unexpected refusal: {}",
        error.detail
    );
}
