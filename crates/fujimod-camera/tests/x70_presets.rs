//! The FujiMod presets for the installed X70 firmware: the published release
//! card is downloaded once into the state directory, an SD card boot reaches
//! REC with the release, and autostart installs its flash once.
//!
//! Set \`FUJIMOD_FIRMWARE_LIBRARY\` to a library laid out by name. The state
//! directory is a scratch directory under \`/tmp\`.
use fujimod_camera::{BootCache, ExecutionPolicy, Library, Preset, Request, Stage, StateDir, open};
use std::time::{Duration, Instant};

fn library() -> Option<Library> {
    match Library::from_env() {
        Ok(library) => Some(library),
        Err(fujimod_camera::LibraryError::Environment) => None,
        Err(error) => panic!("firmware library: {error}"),
    }
}

fn state() -> StateDir {
    StateDir::at(
        std::env::temp_dir().join(format!("fujimod-camera-presets-{}", std::process::id())),
    )
}

fn request(preset: Preset) -> Request {
    Request {
        firmware: "x70-110".into(),
        files: fujimod_camera::FirmwareInput::Library,
        preset,
        sd_card: None,
        policy: ExecutionPolicy::Interactive,
        engine: None,
        cache: BootCache::Bypass,
        run_to_rec: true,
        card: Default::default(),
        nand_image: None,
        face_result_profile: None,
    }
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
