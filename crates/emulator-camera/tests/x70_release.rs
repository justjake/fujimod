//! Open the current release's SD card for the installed X70 firmware.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a directory holding the catalog's X70
//! update and boot ROM. The release card is fetched into that library and
//! then booted; the second open reuses the verified card.
use fujimod_emulator_camera::{
    Boot, BootCache, CardSource, ExecutionPolicy, Library, LibraryError, Request, Stage, open,
};
use std::time::{Duration, Instant};

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
        boot: Boot::SdCard(CardSource::Release),
        policy: ExecutionPolicy::Interactive,
        engine: None,
        card_files: Vec::new(),
        cache: BootCache::Bypass,
        run_to_rec: true,
        card: Default::default(),
    }
}

#[test]
fn the_x70_release_card_opens_to_rec_twice() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let card = library
        .release_card("x70-110")
        .unwrap_or_else(|error| panic!("release card: {error}"));
    assert!(card.join("FUJIMOD/SYSTEM/X70-110/A.JSN").is_file());
    let started = Instant::now();
    for attempt in 1..=2 {
        let opened =
            open(&library, request()).unwrap_or_else(|error| panic!("attempt {attempt}: {error}"));
        assert_eq!(opened.stage, Stage::Rec, "attempt {attempt}");
        let report = fujimod_emulator_boot::status(opened.session())
            .unwrap_or_else(|error| panic!("attempt {attempt}: {error}"));
        assert_eq!(report.status, "rec", "attempt {attempt}");
    }
    let elapsed = started.elapsed();
    eprintln!("X70 release card: REC twice in {elapsed:?}");
    assert!(
        elapsed < Duration::from_secs(60),
        "two release card opens took {elapsed:?}"
    );
}
