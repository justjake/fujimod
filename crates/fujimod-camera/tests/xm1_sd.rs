//! Boot the installed X-M1 firmware from its catalog SD card combination.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a library laid out by name, and
//! `FUJIMOD_TEST_SD_CARD` to a directory holding an X-M1 SD-boot card.
//! Without them the test returns without opening a camera.
use fujimod_camera::{
    BootCache, CardSource, ExecutionPolicy, FirmwareInput, Library, LibraryError, Preset, Request,
    Stage, StateDir, open,
};
use fujimod_emulator_boot as boot;
use std::path::PathBuf;

const FIRMWARE: &str = "xm1-150";
/// The catalog declares REC as this UI mode for X-M1.
const REC_MODE: u32 = 3;

fn library() -> Option<Library> {
    match Library::from_env() {
        Ok(library) => Some(library),
        Err(LibraryError::Environment) => None,
        Err(error) => panic!("firmware library: {error}"),
    }
}

fn card() -> Option<PathBuf> {
    std::env::var_os("FUJIMOD_TEST_SD_CARD").map(PathBuf::from)
}

fn state() -> StateDir {
    StateDir::at(std::env::temp_dir().join(format!("fujimod-xm1-sd-{}", std::process::id())))
}

fn request(card: PathBuf, run_to_rec: bool) -> Request {
    Request {
        firmware: FIRMWARE.into(),
        files: FirmwareInput::Library,
        preset: Preset::SdCard,
        sd_card: Some(CardSource::Directory(card)),
        policy: ExecutionPolicy::Interactive,
        engine: None,
        cache: BootCache::Bypass,
        run_to_rec,
        card: Default::default(),
    }
}

#[test]
fn the_card_boot_holds_the_catalog_sd_controls() {
    let (Some(library), Some(card)) = (library(), card()) else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY and FUJIMOD_TEST_SD_CARD");
        return;
    };
    let opened =
        open(&library, &state(), request(card, false)).expect("open before the first instruction");
    assert_eq!(opened.stage, Stage::BootRom);
    assert!(opened.holds_applied());
    let held: Vec<&str> = opened.holds.controls().collect();
    assert_eq!(
        held,
        ["sub_command_dial.press", "fn_button"],
        "the catalog's boot.sd holds"
    );
    let name = opened
        .session()
        .contacts
        .keys()
        .next()
        .expect("the construction declares the SD card slot");
    let state = opened.session().card_state(name).expect("card state");
    assert!(state.present, "the boot card is inserted");
    assert!(!state.protected, "the boot card is writable");
}

#[test]
fn the_card_boots_the_installed_firmware() {
    let (Some(library), Some(card)) = (library(), card()) else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY and FUJIMOD_TEST_SD_CARD");
        return;
    };
    let started = std::time::Instant::now();
    let mut opened =
        open(&library, &state(), request(card, false)).expect("open before the first instruction");
    assert_eq!(opened.stage, Stage::BootRom);
    let report = boot::to_rec(opened.session_mut(), 2_000_000_000).expect("the card boot to REC");
    // The card's FujiMod boot loads the internal firmware, which then reports
    // its own REC UI mode. Whether that screen is presented is the strict
    // gate's decision, so a `rec` status always carries a presented frame.
    assert_eq!(
        report.ui_mode, REC_MODE,
        "the card boot reported another UI mode"
    );
    if report.status == "rec" {
        assert!(
            report.scanout_instance.is_some(),
            "REC status without a presented scanout: {report:?}"
        );
    }
    eprintln!(
        "card: {} ui mode {} at tick {} after {} instructions in {:?}",
        report.status,
        report.ui_mode,
        report.virtual_ticks,
        report.instructions,
        started.elapsed(),
    );
}
