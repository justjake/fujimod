//! The boot cache: an open saves its checkpoint and a later open with the
//! same inputs restores it instead of booting.
//!
//! Set \`FUJIMOD_FIRMWARE_LIBRARY\` to a directory holding the catalog's X70
//! update and boot ROM; the release card is fetched into that library.
use fujimod_emulator_camera::{
    Boot, BootCache, CardSource, ExecutionPolicy, Library, LibraryError, Request, Stage, open,
};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

fn library() -> Option<Library> {
    match Library::from_env() {
        Ok(library) => Some(library),
        Err(LibraryError::Environment) => None,
        Err(error) => panic!("firmware library: {error}"),
    }
}

fn request(boot: Boot, card_files: Vec<(String, PathBuf)>, cache: BootCache) -> Request {
    Request {
        firmware: "x70-110".into(),
        boot,
        policy: ExecutionPolicy::Interactive,
        engine: None,
        card_files,
        cache,
    }
}

/// The saved boots of one slot, sorted.
fn saved(directory: &Path, slot: &str) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(slot))
                })
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    entries
}

fn stamp(path: &Path) -> (u64, SystemTime) {
    let metadata = fs::metadata(path).unwrap();
    (metadata.len(), metadata.modified().unwrap())
}

#[test]
fn the_x70_sd_card_boot_is_saved_and_restored() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let directory = library.path().join("boot-cache/x70-110");
    let slot = "sd-card-interactive";
    for entry in saved(&directory, slot) {
        fs::remove_file(entry).unwrap();
    }
    let boot = || Boot::SdCard(CardSource::Release);

    let started = Instant::now();
    let first = open(&library, request(boot(), Vec::new(), BootCache::Reuse)).unwrap();
    let booted = started.elapsed();
    assert_eq!(first.stage, Stage::Rec);
    assert!(!first.cached, "the first open boots");
    let entries = saved(&directory, slot);
    assert_eq!(entries.len(), 1, "one saved boot: {entries:?}");
    let saved_boot = entries[0].clone();
    let before = stamp(&saved_boot);

    let started = Instant::now();
    let second = open(&library, request(boot(), Vec::new(), BootCache::Reuse)).unwrap();
    let restored = started.elapsed();
    assert_eq!(second.stage, Stage::Rec);
    assert!(second.cached, "the second open restores the saved boot");
    let report = fujimod_emulator_boot::status(second.session()).unwrap();
    assert_eq!(report.status, "rec");

    let started = Instant::now();
    let third = open(&library, request(boot(), Vec::new(), BootCache::Bypass)).unwrap();
    let bypassed = started.elapsed();
    assert_eq!(third.stage, Stage::Rec);
    assert!(!third.cached, "a bypass boots");
    assert_eq!(saved(&directory, slot), entries, "a bypass saves nothing");
    assert_eq!(
        stamp(&saved_boot),
        before,
        "a bypass leaves the saved boot alone"
    );

    eprintln!("X70 boot cache: boot {booted:?}, restore {restored:?}, bypass {bypassed:?}");
    assert!(
        restored < booted,
        "a restore is faster than a boot: {restored:?} vs {booted:?}"
    );
    assert!(booted < Duration::from_secs(60));
}

#[test]
fn a_changed_card_saves_its_own_x70_boot() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let directory = library.path().join("boot-cache/x70-110");
    let slot = "stock-interactive";
    for entry in saved(&directory, slot) {
        fs::remove_file(entry).unwrap();
    }
    let card = std::env::temp_dir().join(format!("fujimod-boot-cache-{}", std::process::id()));
    fs::create_dir_all(&card).unwrap();
    let file = card.join("CACHE.TXT");
    fs::write(&file, b"one").unwrap();
    let files = vec![("CACHE.TXT".to_owned(), file.clone())];

    let first = open(
        &library,
        request(Boot::Stock, files.clone(), BootCache::Reuse),
    )
    .unwrap();
    assert!(!first.cached);
    let entries = saved(&directory, slot);
    assert_eq!(entries.len(), 1);

    let reused = open(
        &library,
        request(Boot::Stock, files.clone(), BootCache::Reuse),
    )
    .unwrap();
    assert!(reused.cached, "the same card restores");

    fs::write(&file, b"two").unwrap();
    let changed = open(&library, request(Boot::Stock, files, BootCache::Reuse)).unwrap();
    assert!(!changed.cached, "a changed card boots");
    let entries = saved(&directory, slot);
    assert_eq!(entries.len(), 1, "the slot keeps one key: {entries:?}");

    let reused = open(
        &library,
        request(
            Boot::Stock,
            vec![("CACHE.TXT".to_owned(), file.clone())],
            BootCache::Reuse,
        ),
    )
    .unwrap();
    assert!(reused.cached, "the changed card's boot was saved");
    fs::remove_dir_all(&card).unwrap();
}
