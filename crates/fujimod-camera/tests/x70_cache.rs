//! The boot cache: an open saves its compressed checkpoint and a later open
//! with the same inputs restores it instead of booting.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a library laid out by name. Without it a
//! test returns without opening a camera. The SD card is the published release
//! card, downloaded once into the state directory.
mod support;
use support::{FIRMWARE, library, request, state};

use fujimod_camera::{
    BootCache, CardSource, Error, Library, Opened, Preset, Request, Stage, StateDir, open,
};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

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
                        .is_some_and(|name| name.contains(&format!("-{slot}-")))
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

fn open_at(library: &Library, state: &StateDir, request: Request) -> Result<Opened, Error> {
    open(library, state, request)
}

#[test]
fn the_x70_sd_card_boot_is_saved_and_restored() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let state = state();
    let directory = state.boot_cache().join(FIRMWARE);
    let _ = fs::remove_dir_all(&directory);
    let boot = || request(FIRMWARE, Preset::SdCard, None, BootCache::Reuse, true);

    let started = Instant::now();
    let first = open_at(&library, &state, boot()).unwrap();
    let booted = started.elapsed();
    assert_eq!(first.stage, Stage::Rec);
    assert!(!first.cached, "the first open boots");
    let entries = saved(&directory, "held-sd-interactive");
    assert_eq!(entries.len(), 1, "one saved boot: {entries:?}");
    let saved_boot = entries[0].clone();

    let started = Instant::now();
    let second = open_at(&library, &state, boot()).unwrap();
    let restored = started.elapsed();
    assert_eq!(second.stage, Stage::Rec);
    assert!(second.cached, "the second open restores the saved boot");
    let report = fujimod_emulator_boot::status(second.session()).unwrap();
    assert_eq!(report.status, "rec");
    // A restore marks the save as the most recently used one.
    let before = stamp(&saved_boot);

    let started = Instant::now();
    let third = open_at(
        &library,
        &state,
        request(FIRMWARE, Preset::SdCard, None, BootCache::Bypass, true),
    )
    .unwrap();
    let bypassed = started.elapsed();
    assert_eq!(third.stage, Stage::Rec);
    assert!(!third.cached, "a bypass boots");
    assert_eq!(
        saved(&directory, "held-sd-interactive"),
        entries,
        "a bypass saves nothing"
    );
    assert_eq!(stamp(&saved_boot), before, "a bypass leaves the save alone");

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
    let state = state();
    let directory = state.boot_cache().join(FIRMWARE);
    let slot = "stock-interactive";
    for entry in saved(&directory, slot) {
        fs::remove_file(entry).unwrap();
    }
    let one = vec![("CACHE.TXT".to_owned(), b"one".to_vec())];
    let two = vec![("CACHE.TXT".to_owned(), b"two".to_vec())];

    let first = open_at(
        &library,
        &state,
        request(
            FIRMWARE,
            Preset::Stock,
            Some(CardSource::Files(one.clone())),
            BootCache::Reuse,
            true,
        ),
    )
    .unwrap();
    assert!(!first.cached);
    assert_eq!(saved(&directory, slot).len(), 1);

    let reused = open_at(
        &library,
        &state,
        request(
            FIRMWARE,
            Preset::Stock,
            Some(CardSource::Files(one)),
            BootCache::Reuse,
            true,
        ),
    )
    .unwrap();
    assert!(reused.cached, "the same card restores");

    let changed = open_at(
        &library,
        &state,
        request(
            FIRMWARE,
            Preset::Stock,
            Some(CardSource::Files(two.clone())),
            BootCache::Reuse,
            true,
        ),
    )
    .unwrap();
    assert!(!changed.cached, "a changed card boots");
    assert_eq!(
        saved(&directory, slot).len(),
        1,
        "the slot keeps one key: {:?}",
        saved(&directory, slot)
    );

    let reused = open_at(
        &library,
        &state,
        request(
            FIRMWARE,
            Preset::Stock,
            Some(CardSource::Files(two)),
            BootCache::Reuse,
            true,
        ),
    )
    .unwrap();
    assert!(reused.cached, "the changed card's boot was saved");
}
