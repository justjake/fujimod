//! The boot cache: the portable checkpoint a successful open leaves behind,
//! so a later open with the same inputs starts at REC instead of booting.
//!
//! The key is everything that decides what a boot reaches: the firmware
//! update, the resolved run configuration, the boot, the SD card's files, the
//! execution policy, the engine and this builder's version. A saved boot lives
//! in `boot-cache/<firmware>/<boot>-<policy>-<engine>-<key>.snapshot`; saving
//! one removes the other keys of the same slot, so a library keeps at most one
//! boot per firmware, boot, policy and engine.
//!
//! A saved boot is reused only when its checkpoint imports and restores into a
//! freshly built session and the session still reports REC. Anything else
//! boots the camera and replaces the entry.
use crate::{Boot, Library, OpenError, Opened, Request, Stage};
use fujimod_catalog_configuration::RunEngine;
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_boot as boot;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// Bumped when a builder change makes saved boots unusable.
const FORMAT: u32 = 1;

/// One saved boot's place in the firmware library.
pub(crate) struct Key {
    directory: PathBuf,
    /// Shared by every key of one firmware, boot, policy and engine.
    slot: String,
    file: String,
}

impl Key {
    pub(crate) fn new(
        library: &Library,
        request: &Request,
        resolved: &ResolvedConfiguration,
    ) -> Result<Self, OpenError> {
        let boot = match &request.boot {
            Boot::Stock => "stock",
            Boot::SdCard(_) => "sd-card",
            Boot::AutomaticBoot(_) => "automatic",
        };
        let policy = request.policy.name();
        let engine = engine_name(resolved);
        let slot = format!("{boot}-{policy}-{engine}");
        let key = identity(library, request, resolved)?;
        let file = format!("{slot}-{key}.snapshot");
        Ok(Self {
            directory: library
                .path()
                .join(crate::library::CACHE_DIRECTORY)
                .join(&request.firmware),
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
        name.starts_with(&self.slot)
            && name != self.file
            && Path::new(name)
                .extension()
                .is_some_and(|extension| extension == "snapshot")
    }
}

/// Restore the saved boot when it still restores into a session built from the
/// same inputs and reports REC.
pub(crate) fn restore(
    library: &Library,
    request: &Request,
    resolved: &ResolvedConfiguration,
    key: &Key,
) -> Result<Option<Opened>, OpenError> {
    let path = key.path();
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
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
    opened.stage = Stage::Rec;
    opened.cached = true;
    Ok(Some(opened))
}

/// Write the boot's checkpoint, replacing the slot's other keys.
///
/// The cache is a convenience: a library that refuses the write leaves the
/// camera running and simply boots next time.
pub(crate) fn save(opened: &Opened, key: &Key) {
    let Ok(snapshot) = opened.session().checkpoint(None) else {
        return;
    };
    let Ok(bytes) = snapshot.export() else {
        return;
    };
    if fs::create_dir_all(&key.directory).is_err() {
        return;
    }
    let path = key.path();
    let temporary = path.with_extension("snapshot.tmp");
    if fs::write(&temporary, &bytes).is_err() {
        return;
    }
    if fs::rename(&temporary, &path).is_err() {
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
fn identity(
    library: &Library,
    request: &Request,
    resolved: &ResolvedConfiguration,
) -> Result<String, OpenError> {
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
    part(resolved.configuration.id.as_bytes());
    part(request.policy.name().as_bytes());
    part(engine_name(resolved).as_bytes());
    part(match &request.boot {
        Boot::Stock => b"stock",
        Boot::SdCard(_) => b"sd-card",
        Boot::AutomaticBoot(_) => b"automatic",
    });
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
    if let Some(image) = &request.image {
        part(b"ram-image");
        part(image.sha256.to_ascii_lowercase().as_bytes());
    }
    for (name, bytes) in card(library, request)? {
        part(name.as_bytes());
        part(&bytes);
    }
    Ok(format!("{:x}", hash.finalize())[..16].to_owned())
}

/// The card an open writes: the boot source's files, then the caller's extras,
/// which replace a file of the same name.
fn card(library: &Library, request: &Request) -> Result<BTreeMap<String, Vec<u8>>, OpenError> {
    let mut files = match &request.boot {
        Boot::Stock => BTreeMap::new(),
        Boot::SdCard(source) | Boot::AutomaticBoot(source) => {
            crate::open::card_contents(library, &request.firmware, source)?
                .into_iter()
                .collect()
        }
    };
    for (name, path) in &request.card_files {
        let bytes = fs::read(path).map_err(|error| {
            OpenError::at(Stage::BootRom, format!("{}: {error}", path.display()))
        })?;
        files.insert(name.clone(), bytes);
    }
    Ok(files)
}
