//! The FujiMod presets for the installed X70 firmware: the published release
//! card is downloaded once into the state directory, an SD card boot reaches
//! REC with the release, and autostart installs its flash once.
//!
//! Set \`FUJIMOD_FIRMWARE_LIBRARY\` to a firmware library. The state
//! directory is a scratch directory under \`/tmp\`.
use fujimod_camera::{BootCache, ExecutionPolicy, Library, Preset, Request, Stage, StateDir, open};
use std::time::{Duration, Instant};

fn library() -> Option<Library> {
    let library = Library::from_env().unwrap_or_else(|error| panic!("firmware library: {error}"));
    library
        .firmwares()
        .iter()
        .any(|firmware| firmware.id == "x70-110")
        .then_some(library)
}

fn state() -> StateDir {
    StateDir::at(
        std::env::temp_dir().join(format!("fujimod-camera-presets-{}", std::process::id())),
    )
}

fn request(preset: Preset) -> Request {
    Request {
        camera: fujimod_camera::EmulatorRequestBase {
            policy: ExecutionPolicy::Interactive,
            cache: BootCache::Bypass,
            ..fujimod_camera::EmulatorRequestBase::new(
                "x70",
                fujimod_camera::FirmwareChoice::Named("x70-110".into()),
            )
        },
        preset,
    }
}

/// The same request, stopped before its first instruction.
fn unbooted(preset: Preset, files: Option<Vec<(String, Vec<u8>)>>) -> Request {
    let mut request = request(preset);
    request.camera.cards.sd_card = files.map(fujimod_emulator_camera::Card::with_files);
    request.camera.run_to_rec = false;
    request
}

#[test]
fn the_x70_sd_card_boot_starts_from_the_stock_internal_flash() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let state = state();
    let stock = open(&library, &state, unbooted(Preset::Stock, None))
        .expect("stock open before the first instruction");
    let card = open(&library, &state, unbooted(Preset::SdCard, Some(Vec::new())))
        .expect("SD card open before the first instruction");
    let facts = library.nand_boot("x70-110").expect("X70 NAND boot facts");
    let stock_flash = fujimod_camera::read_flash(stock.session(), facts).expect("stock flash");
    let card_flash = fujimod_camera::read_flash(card.session(), facts).expect("card flash");
    assert_eq!(
        card_flash, stock_flash,
        "the SD card boot did not start from the stock internal flash"
    );
}

#[test]
fn the_x70_release_card_opens_to_rec_twice() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let state = state();
    let started = Instant::now();
    let mut release = None;
    for attempt in 1..=2 {
        let opened = open(&library, &state, request(Preset::SdCard))
            .unwrap_or_else(|error| panic!("attempt {attempt}: {error}"));
        assert_eq!(opened.stage, Stage::Rec, "attempt {attempt}");
        let report = fujimod_emulator_boot::status(opened.session())
            .unwrap_or_else(|error| panic!("attempt {attempt}: {error}"));
        assert_eq!(report.status, "rec", "attempt {attempt}");
        // REC is ui mode 3 in system mode 1; first-start setup is another
        // mode, so the pair distinguishes the two.
        assert_eq!(
            (report.ui_mode, report.system_mode),
            (3, 1),
            "attempt {attempt}: not the REC screen"
        );
        release = opened.release.clone();
    }
    let elapsed = started.elapsed();
    let tag = release.unwrap_or_else(|| panic!("the release card names no tag"));
    let card = state.sd_cards().join(&tag).join("x70-110");
    assert!(
        card.join("FUJIMOD/SYSTEM/X70-110/A.JSN").is_file(),
        "{}",
        card.display()
    );
    eprintln!("X70 release card: REC twice in {elapsed:?} from {tag}");
    assert!(
        elapsed < Duration::from_secs(900),
        "two release card opens took {elapsed:?}"
    );
}

#[test]
fn the_x70_autostart_opens_to_rec_twice_from_one_keep_flash() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let state = state();
    let directory = state.autostart().join("x70-110");
    let _ = std::fs::remove_dir_all(&directory);
    let started = Instant::now();
    for attempt in 1..=2 {
        let opened = open(&library, &state, request(Preset::Autostart))
            .unwrap_or_else(|error| panic!("attempt {attempt}: {error}"));
        assert_eq!(opened.stage, Stage::Rec, "attempt {attempt}");
        let report = fujimod_emulator_boot::status(opened.session())
            .unwrap_or_else(|error| panic!("attempt {attempt}: {error}"));
        assert_eq!(report.status, "rec", "attempt {attempt}");
    }
    let flashed = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("{}: {error}", directory.display()))
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "flash"))
        .count();
    assert_eq!(flashed, 1, "one kept internal-flash image");
    let elapsed = started.elapsed();
    eprintln!("X70 autostart: REC twice in {elapsed:?}");
    assert!(
        elapsed < Duration::from_secs(900),
        "two autostart opens took {elapsed:?}"
    );
}
