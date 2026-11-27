//! Open the installed X70 firmware and the release SD card at REC, then press a
//! control through `fujimod-remote`.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a firmware library. Without it the
//! test returns without opening a camera. The SD card is the published release
//! card, downloaded once into the state directory.
use crate::support;
use support::{FIRMWARE, library, request, state};

use fujimod_camera::{BootCache, Library, Opened, Preset, Stage, StateDir, open};
use fujimod_emulator_boot as boot;

const BUTTON: &str = "menu_ok_button";
/// The SD card loader's stage 3 is linked at this guest address.
const STAGE3: u64 = 0x1d02_0000;

fn open_and_press(library: &Library, state: &StateDir, name: &str, preset: Preset) -> Opened {
    let started = std::time::Instant::now();
    let mut opened = open(
        library,
        state,
        request(FIRMWARE, preset, None, BootCache::Bypass, true),
    )
    .unwrap_or_else(|error| panic!("{name}: {error}"));
    assert_eq!(opened.stage, Stage::Rec, "{name} did not report REC");
    let report =
        boot::status(opened.session()).unwrap_or_else(|error| panic!("{name}: status: {error}"));
    assert_eq!(report.status, "rec", "{name} did not reach REC");
    support::press(&mut opened, BUTTON);
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
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let state = state();
    open_and_press(&library, &state, "stock", Preset::Stock);
    let opened = open_and_press(&library, &state, "SD card", Preset::SdCard);
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
