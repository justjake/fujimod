//! The boot cache: the compressed checkpoint a successful open leaves behind,
//! so a later open with the same inputs starts at REC instead of booting.
//!
//! The key is everything that decides what a boot reaches: the firmware
//! update, the resolved run configuration, the start, the SD card's files and
//! options, the execution policy, the engine and this builder's version. A
//! saved boot lives in `<state>/boot-cache/<firmware>/v<format>-<version>-<start>-<policy>-<engine>-<key>.snapshot.zst`;
//! saving one removes the other keys of the same slot.
//!
//! The cache holds at most `FUJIMOD_BOOT_CACHE_MAX` bytes (1 GiB by default).
//! Saving past the limit deletes the least recently used saves first. Opening
//! the cache deletes leftover temporary files and saves from other builder
//! versions.
//!
//! A saved boot is reused only when its checkpoint imports and restores into a
//! freshly built session and the session still reports REC. Anything else
//! boots the camera and replaces the entry.
use crate::{Flash, Library, OpenError, Opened, Request, Stage, Start, StateDir};
use fujimod_catalog_configuration::RunEngine;
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_boot as boot;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// Bumped when a builder change makes saved boots unusable.
const FORMAT: u32 = 2;

/// Compression level for saved boots.
const COMPRESSION: i32 = 3;

/// The environment variable bounding the whole cache.
pub const MAX_ENV: &str = "FUJIMOD_BOOT_CACHE_MAX";

/// The cache's size limit: `FUJIMOD_BOOT_CACHE_MAX`, or one GiB.
pub fn limit() -> u64 {
    std::env::var(MAX_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|bytes| *bytes > 0)
        .unwrap_or(1 << 30)
}

/// This builder's version, as it appears in saved file names.
fn version() -> String {
    env!("CARGO_PKG_VERSION")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect()
}

/// The file-name prefix every save of this builder version shares.
fn prefix() -> String {
    format!("v{FORMAT}-{}-", version())
}

/// One saved boot's place in the state directory.
pub(crate) struct Key {
    directory: PathBuf,
    /// Shared by every key of one firmware, start, policy and engine.
    slot: String,
    file: String,
}

impl Key {
    pub(crate) fn new(
        state: &StateDir,
        request: &Request,
        resolved: &ResolvedConfiguration,
    ) -> Result<Self, OpenError> {
        let start = match &request.start {
            Start::Rom => "stock".to_owned(),
            Start::Held { combination } => format!("held-{combination}"),
            Start::Loader(_) => "loader".to_owned(),
        };
        let policy = request.policy.name();
        let engine = engine_name(resolved);
        let slot = format!("{start}-{policy}-{engine}");
        let key = identity(request, resolved)?;
        let file = format!("{}{slot}-{key}.snapshot.zst", prefix());
        Ok(Self {
            directory: state.boot_cache().join(&request.firmware),
            slot,
            file,
        })
    }

    /// The saved boot this key names.
    pub(crate) fn path(&self) -> PathBuf {
        self.directory.join(&self.file)
    }

    /// Whether a directory entry belongs to another key of the same slot.
    fn supersedes(&self, name: &str) -> bool {
        name.starts_with(&format!("{}{}", prefix(), self.slot))
            && name != self.file
            && name.ends_with(".snapshot.zst")
    }
}

/// Restore the saved boot when it still restores into a session built from the
/// same inputs and reports REC. Also deletes leftover temporary files and
/// saves from other builder versions first.
pub(crate) fn restore(
    library: &Library,
    request: &Request,
    resolved: &ResolvedConfiguration,
    key: &Key,
) -> Result<Option<Opened>, OpenError> {
    clean(&key.directory);
    let path = key.path();
    let stored = match fs::read(&path) {
        Ok(stored) => stored,
        Err(_) => return Ok(None),
    };
    let Ok(bytes) = zstd::decode_all(stored.as_slice()) else {
        return Ok(None);
    };
    let mut opened = crate::open::construct(library, request.clone(), resolved.clone())?;
    let restored = opened
        .session()
        .import_snapshot(&bytes)
        .map_err(|error| format!("{}: {error}", path.display()))
        .and_then(|snapshot| opened.session_mut().restore(&snapshot));
    if restored.is_err() {
        return Ok(None);
    }
    let report = match boot::status(opened.session()) {
        Ok(report) => report,
        Err(_) => return Ok(None),
    };
    if report.status != "rec" || report.menu_active != 0 || report.pending {
        return Ok(None);
    }
    // The save is the most recently used one from now on.
    if let Ok(file) = fs::File::options().write(true).open(&path) {
        let _ = file.set_modified(SystemTime::now());
    }
    opened.stage = Stage::Rec;
    opened.cached = true;
    Ok(Some(opened))
}

/// Write the boot's checkpoint, replacing the slot's other keys.
///
/// The cache is a convenience: a directory that refuses the write leaves the
/// camera running and simply boots next time.
pub(crate) fn save(opened: &Opened, key: &Key) {
    let Ok(snapshot) = opened.session().checkpoint(None) else {
        return;
    };
    let Ok(bytes) = snapshot.export() else {
        return;
    };
    let Ok(compressed) = zstd::encode_all(bytes.as_slice(), COMPRESSION) else {
        return;
    };
    if fs::create_dir_all(&key.directory).is_err() {
        return;
    }
    let path = key.path();
    let temporary = path.with_extension("snapshot.zst.tmp");
    let written = fs::File::create(&temporary)
        .and_then(|mut file| file.write_all(&compressed))
        .and_then(|_| fs::rename(&temporary, &path));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
        return;
    }
    let Ok(entries) = fs::read_dir(&key.directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if key.supersedes(name) {
            let _ = fs::remove_file(entry.path());
        }
    }
    evict(&key.directory, limit());
}

/// Delete leftover temporary files and saves from other builder versions.
fn clean(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let keep = prefix();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(&keep) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Delete the least recently used saves until the cache is within its limit.
fn evict(directory: &Path, limit: u64) {
    let mut saves = saved(directory);
    let mut total: u64 = saves.iter().map(|save| save.size).sum();
    saves.sort_by_key(|save| save.modified);
    let mut at = 0;
    while total > limit && at < saves.len() {
        let save = &saves[at];
        if fs::remove_file(&save.path).is_ok() {
            total = total.saturating_sub(save.size);
        }
        at += 1;
    }
}

struct Save {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

/// Every saved boot in a cache directory, with its size and last use.
fn saved(directory: &Path) -> Vec<Save> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            if !name.ends_with(".snapshot.zst") {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            Some(Save {
                path: entry.path(),
                size: metadata.len(),
                modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            })
        })
        .collect()
}

/// The engine a boot runs, from the request's override or the configuration.
fn engine_name(resolved: &ResolvedConfiguration) -> String {
    match resolved
        .configuration
        .execution
        .map(|execution| execution.engine)
    {
        Some(engine) => name(engine),
        None => "none".into(),
    }
}

/// A run engine's catalog name.
fn name(engine: RunEngine) -> String {
    serde_json::to_value(engine)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{engine:?}"))
}

/// Hash the inputs that decide what the boot reaches.
fn identity(request: &Request, resolved: &ResolvedConfiguration) -> Result<String, OpenError> {
    let mut hash = Sha256::new();
    let mut part = |bytes: &[u8]| {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    };
    part(b"fujimod-emulator-camera boot cache");
    part(env!("CARGO_PKG_VERSION").as_bytes());
    part(&FORMAT.to_le_bytes());
    part(request.firmware.as_bytes());
    let firmware = &resolved.reference.firmware;
    part(
        firmware
            .update
            .as_ref()
            .map(|update| update.sha256.as_str())
            .unwrap_or_default()
            .as_bytes(),
    );
    part(
        firmware
            .boot_rom
            .as_ref()
            .map(|rom| rom.content.sha256.as_str())
            .unwrap_or_default()
            .as_bytes(),
    );
    part(resolved.configuration.id.as_bytes());
    part(request.policy.name().as_bytes());
    part(engine_name(resolved).as_bytes());
    match &request.start {
        Start::Rom => part(b"stock"),
        Start::Held { combination } => {
            part(b"held");
            part(combination.as_bytes());
        }
        Start::Loader(image) => {
            part(b"loader");
            part(image.sha256.to_ascii_lowercase().as_bytes());
        }
    }
    match &request.flash {
        Flash::Stock => part(b"stock-flash"),
        Flash::Image(bytes) => {
            part(b"flash-image");
            part(&Sha256::digest(bytes));
        }
    }
    part(&[request.card.present as u8, request.card.protected as u8]);
    match request.card.transfer_ticks {
        Some(ticks) => {
            part(b"transfer-ticks");
            part(&ticks.to_le_bytes());
        }
        None => part(b"catalog-transfer-ticks"),
    }
    if let Some(layout) = &request.card.layout {
        part(b"card-layout");
        let bytes = serde_json::to_vec(layout)
            .map_err(|failure| OpenError::at(Stage::BootRom, format!("card layout: {failure}")))?;
        part(&bytes);
    }
    let card: BTreeMap<&str, &[u8]> = request
        .card
        .files
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    for (name, bytes) in card {
        part(name.as_bytes());
        part(bytes);
    }
    Ok(format!("{:x}", hash.finalize())[..16].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("fujimod-boot-cache-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn save_file(directory: &Path, name: &str, bytes: usize, ago: u64) -> PathBuf {
        let path = directory.join(name);
        fs::write(&path, vec![0u8; bytes]).unwrap();
        let modified = SystemTime::now() - std::time::Duration::from_secs(ago);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        path
    }

    #[test]
    fn opening_the_cache_deletes_temporary_files_and_other_versions() {
        let directory = directory("clean");
        let current = save_file(&directory, &format!("{}x.snapshot.zst", prefix()), 8, 0);
        let temporary = save_file(&directory, "leftover.snapshot.zst.tmp", 8, 0);
        let other = save_file(&directory, "v0-0.0.1-old.snapshot.zst", 8, 0);
        clean(&directory);
        assert!(current.is_file(), "this version's save stays");
        assert!(!temporary.exists(), "a leftover temporary file goes");
        assert!(!other.exists(), "another builder version's save goes");
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn the_cache_deletes_the_least_recently_used_saves_past_its_limit() {
        let directory = directory("evict");
        let oldest = save_file(&directory, &format!("{}a.snapshot.zst", prefix()), 100, 30);
        let middle = save_file(&directory, &format!("{}b.snapshot.zst", prefix()), 100, 20);
        let newest = save_file(&directory, &format!("{}c.snapshot.zst", prefix()), 100, 10);
        evict(&directory, 250);
        assert!(!oldest.exists(), "the oldest save goes first");
        assert!(middle.is_file(), "the middle save fits");
        assert!(newest.is_file(), "the newest save stays");
        evict(&directory, 100);
        assert!(!middle.exists(), "the next oldest save goes");
        assert!(newest.is_file(), "the newest save is the last to go");
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn the_limit_defaults_to_one_gibibyte() {
        if std::env::var_os(MAX_ENV).is_none() {
            assert_eq!(limit(), 1 << 30);
        }
    }
}
