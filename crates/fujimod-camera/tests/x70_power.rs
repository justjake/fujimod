//! X70 tests for powering a camera off and on again with its media kept.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a library laid out by name. Without it the
//! test returns without opening a camera. The SD card is the published release
//! card, downloaded once into the state directory.
mod support;
use support::{FIRMWARE, library, request, state};

use fujimod_camera::{BootCache, Error, Library, Opened, PowerOn, Preset, Stage, StateDir, open};

fn assert_rec(opened: &Opened, name: &str) {
    assert_eq!(opened.stage, Stage::Rec, "{name} did not reach REC");
    let report = fujimod_emulator_boot::status(opened.session())
        .unwrap_or_else(|error| panic!("{name}: status: {error}"));
    assert_eq!(report.status, "rec", "{name} did not report REC");
}

fn open_at(library: &Library, state: &StateDir) -> Result<Opened, Error> {
    open(
        library,
        state,
        request(FIRMWARE, Preset::SdCard, None, BootCache::Bypass, true),
    )
}

#[test]
fn the_x70_keeps_its_flash_and_card_across_power_ons() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let state = state();
    let opened = open_at(&library, &state).expect("SD card open");
    assert_rec(&opened, "SD card");
    let opened = opened
        .power_on(
            &library,
            &state,
            PowerOn {
                run_to_rec: true,
                ..PowerOn::default()
            },
        )
        .expect("power on from internal flash");
    assert_rec(&opened, "internal flash with the kept card");
    let held = PowerOn {
        hold_controls: true,
        run_to_rec: true,
        ..PowerOn::default()
    };
    let opened = opened
        .power_on(&library, &state, held)
        .expect("power on from the kept card");
    assert_rec(&opened, "SD card with the kept card");
    let opened = opened
        .power_on(
            &library,
            &state,
            PowerOn {
                fresh_card: true,
                ..held
            },
        )
        .expect("power on from a fresh card");
    assert_rec(&opened, "SD card with a fresh card");
}
