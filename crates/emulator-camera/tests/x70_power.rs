//! X70 tests for powering a camera off and on again with its media kept.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a library laid out by name, and
//! `FUJIMOD_TEST_SD_CARD` to a directory holding the release SD card.
mod support;
use support::{Boot, CardSource, library, state, test_card};

use fujimod_emulator_camera::{
    BootCache, Library, OpenError, Opened, PowerOn, Request, Stage, open,
};

fn request(boot: Boot) -> Request {
    support::request(boot, None, BootCache::Bypass)
}

fn open_at(library: &Library, request: Request) -> Result<Opened, OpenError> {
    open(library, &state(), request)
}

fn assert_rec(opened: &Opened, name: &str) {
    assert_eq!(opened.stage, Stage::Rec, "{name} did not reach REC");
    let report = fujimod_emulator_boot::status(opened.session())
        .unwrap_or_else(|error| panic!("{name}: status: {error}"));
    assert_eq!(report.status, "rec", "{name} did not report REC");
}

#[test]
fn the_x70_keeps_its_flash_and_card_across_power_ons() {
    let (Some(library), Some(card)) = (library(), test_card()) else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY and FUJIMOD_TEST_SD_CARD");
        return;
    };
    let opened = open_at(&library, request(Boot::SdCard(CardSource::Directory(card))))
        .expect("SD card open");
    assert_rec(&opened, "SD card");
    let opened = opened
        .power_on(&library, &state(), PowerOn::default())
        .expect("power on from internal flash");
    assert_rec(&opened, "internal flash with the kept card");
    let held = PowerOn {
        hold: Some("sd".into()),
        ..PowerOn::default()
    };
    let opened = opened
        .power_on(&library, &state(), held.clone())
        .expect("power on from the kept card");
    assert_rec(&opened, "SD card with the kept card");
    let opened = opened
        .power_on(
            &library,
            &state(),
            PowerOn {
                fresh_card: true,
                ..held
            },
        )
        .expect("power on from a fresh card");
    assert_rec(&opened, "SD card with a fresh card");
}
