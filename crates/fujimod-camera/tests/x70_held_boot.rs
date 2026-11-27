//! Hold the X70 release SD card's catalog power-on combination, run the boot
//! from the caller, then release the holds and press a control.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a firmware library. Without it the
//! test returns without opening a camera. The SD card is the published release
//! card, downloaded once into the state directory.
mod support;
use support::{FIRMWARE, library, request, state};

use fujimod_camera::{BootCache, Opened, Preset, Stage, open};
use std::collections::BTreeMap;

const BUTTON: &str = "menu_ok_button";

fn positions(opened: &Opened) -> BTreeMap<String, String> {
    opened
        .session()
        .control_state()
        .expect("control state")
        .positions
}

#[test]
fn a_held_sd_card_boot_stops_before_the_first_instruction() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let mut opened = open(
        &library,
        &state(),
        request(FIRMWARE, Preset::SdCard, None, BootCache::Bypass, false),
    )
    .expect("open before the first instruction");
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

    let report = fujimod_emulator_boot::to_rec(opened.session_mut(), 2_000_000_000)
        .expect("the caller runs the boot");
    assert_eq!(report.status, "rec");
    let still_held = positions(&opened);
    assert_eq!(
        still_held.get("movie_record_button").map(String::as_str),
        Some("pressed"),
        "the caller releases the holds"
    );
    opened
        .camera
        .release_holds()
        .expect("release the boot holds");
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
    support::press(&mut opened, BUTTON);
}
