//! Open the current release's automatic boot for the installed X70 firmware.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a directory holding the catalog's X70
//! update and boot ROM. The release card is fetched into that library; the
//! first open drives the card's installer to write the camera's internal
//! flash, and the second open starts from the kept flash without holding a
//! button.
use fujimod_emulator_camera::{
    Boot, BootCache, CardSource, ExecutionPolicy, Library, LibraryError, Request, Stage, open,
};
use std::{
    fs,
    time::{Duration, Instant},
};

fn library() -> Option<Library> {
    match Library::from_env() {
        Ok(library) => Some(library),
        Err(LibraryError::Environment) => None,
        Err(error) => panic!("firmware library: {error}"),
    }
}

fn request() -> Request {
    Request {
        firmware: "x70-110".into(),
        boot: Boot::AutomaticBoot(CardSource::Release),
        policy: ExecutionPolicy::Interactive,
        engine: None,
        card_files: Vec::new(),
        cache: BootCache::Bypass,
    }
}

#[test]
fn the_x70_automatic_boot_opens_to_rec_twice() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let card = library
        .release_card("x70-110")
        .unwrap_or_else(|error| panic!("release card: {error}"));
    let release = card
        .parent()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .map(str::to_owned);
    let directory = library.path().join("automatic-boot/x70-110");
    let _ = fs::remove_dir_all(&directory);
    let started = Instant::now();
    for attempt in 1..=2 {
        let opened =
            open(&library, request()).unwrap_or_else(|error| panic!("attempt {attempt}: {error}"));
        assert_eq!(opened.stage, Stage::Rec, "attempt {attempt}");
        assert_eq!(opened.release, release, "attempt {attempt}");
        let report = fujimod_emulator_boot::status(opened.session())
            .unwrap_or_else(|error| panic!("attempt {attempt}: {error}"));
        assert_eq!(report.status, "rec", "attempt {attempt}");
    }
    let flashed = fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("{}: {error}", directory.display()))
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "flash"))
        .count();
    assert_eq!(flashed, 1, "one kept internal-flash image");
    let elapsed = started.elapsed();
    eprintln!("X70 automatic boot: REC twice in {elapsed:?}");
    assert!(
        elapsed < Duration::from_secs(60),
        "two automatic boot opens took {elapsed:?}"
    );
}
