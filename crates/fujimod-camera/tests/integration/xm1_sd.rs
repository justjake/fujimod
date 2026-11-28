//! Boot the installed X-M1 firmware from its catalog SD card combination.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a firmware library. Without it the
//! test returns without opening a camera.
use crate::support;
use support::{built_card, library, state};

use fujimod_camera::{BootCache, Preset, Stage, open};
use fujimod_emulator_boot as boot;
use support::CardSource;

const FIRMWARE: &str = "xm1-150";
/// The catalog declares REC as this UI mode for X-M1.
const REC_MODE: u32 = 3;

#[test]
fn the_card_boot_holds_the_catalog_sd_controls() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let card = CardSource::Files(built_card(&library, FIRMWARE));
    let opened = open(
        &library,
        &state(),
        support::request(
            FIRMWARE,
            Preset::SdCard,
            Some(card),
            BootCache::Bypass,
            false,
        ),
    )
    .expect("open before the first instruction");
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
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let card = CardSource::Files(built_card(&library, FIRMWARE));
    let started = std::time::Instant::now();
    let mut opened = open(
        &library,
        &state(),
        support::request(
            FIRMWARE,
            Preset::SdCard,
            Some(card),
            BootCache::Bypass,
            false,
        ),
    )
    .expect("open before the first instruction");
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
