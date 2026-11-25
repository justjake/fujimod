//! Firmware files found by content hash, and the catalog resolution that names
//! the run configuration for a firmware.
use fujimod_catalog::ReferenceCatalog;
use fujimod_catalog_configuration::RunOverlay;
use fujimod_catalog_resolution::ResolvedConfiguration;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

/// The environment variable naming the firmware library directory.
pub const LIBRARY_ENV: &str = "FUJIMOD_FIRMWARE_LIBRARY";

/// A firmware library could not be read or did not hold the catalog's files.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("{LIBRARY_ENV} is not set")]
    Environment,
    #[error("{path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("catalog: {0}")]
    Catalog(String),
    #[error("{path}: sha256 {sha256} is not a catalog firmware update or boot ROM")]
    UnknownFile { path: PathBuf, sha256: String },
    #[error("firmware {0} is not in the catalog")]
    UnknownFirmware(String),
    #[error("firmware {0} declares no browser run configuration")]
    UnregisteredFirmware(String),
    #[error("{0}")]
    Missing(String),
}

/// A directory of firmware files, found by SHA-256 against the catalog.
pub struct Library {
    root: PathBuf,
    catalog: ReferenceCatalog,
    /// SHA-256 identity of every catalog update and boot ROM in the directory.
    files: BTreeMap<String, PathBuf>,
    /// Explicit camera section directories by camera identity.
    cameras: BTreeMap<String, PathBuf>,
}

/// A catalog firmware whose update and boot ROM this library holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryFirmware {
    pub id: String,
    pub body_id: String,
    pub version: String,
}

impl Library {
    /// Read the directory named by the library environment variable.
    pub fn from_env() -> Result<Self, LibraryError> {
        let path = std::env::var_os(LIBRARY_ENV).ok_or(LibraryError::Environment)?;
        Self::new(PathBuf::from(path))
    }

    /// Read a directory of firmware files.
    ///
    /// Every regular file outside a cameras directory is identified by its
    /// SHA-256 and must be a catalog firmware update or boot ROM. A
    /// cameras/<id> directory holds the zstd-compressed NAND sections of the
    /// named camera.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, LibraryError> {
        let root = path.into();
        let catalog = ReferenceCatalog::bundled().map_err(LibraryError::Catalog)?;
        let known = known_files(&catalog);
        let mut files = BTreeMap::new();
        let mut cameras = BTreeMap::new();
        let camera_root = root.join("cameras");
        if camera_root.is_dir() {
            for entry in read_dir(&camera_root)? {
                if entry.is_dir() {
                    let id = entry
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(|| {
                            LibraryError::Missing(format!(
                                "{}: camera directory is not UTF-8",
                                entry.display()
                            ))
                        })?
                        .to_owned();
                    cameras.insert(id, entry);
                }
            }
        }
        let mut pending = vec![root.clone()];
        while let Some(directory) = pending.pop() {
            for entry in read_dir(&directory)? {
                if entry.starts_with(&camera_root) {
                    continue;
                }
                if entry.is_dir() {
                    pending.push(entry);
                    continue;
                }
                let metadata = fs::metadata(&entry).map_err(|source| LibraryError::Read {
                    path: entry.clone(),
                    source,
                })?;
                if !metadata.is_file() || metadata.len() > 1 << 30 {
                    continue;
                }
                let bytes = fs::read(&entry).map_err(|source| LibraryError::Read {
                    path: entry.clone(),
                    source,
                })?;
                let sha256 = format!("{:x}", Sha256::digest(&bytes));
                if !known.contains(&sha256) {
                    return Err(LibraryError::UnknownFile {
                        path: entry,
                        sha256,
                    });
                }
                files.insert(sha256, entry);
            }
        }
        Ok(Self {
            root,
            catalog,
            files,
            cameras,
        })
    }

    /// The firmware library directory.
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// The catalog firmware record for a firmware identity.
    pub fn firmware(&self, id: &str) -> Result<&fujimod_catalog::Firmware, LibraryError> {
        self.catalog
            .firmware(id)
            .ok_or_else(|| LibraryError::UnknownFirmware(id.into()))
    }

    /// Catalog firmwares with both files and a browser run configuration in
    /// this library, in catalog identity order.
    pub fn firmwares(&self) -> Vec<LibraryFirmware> {
        self.catalog
            .firmware_ids()
            .filter_map(|id| {
                let firmware = self.catalog.firmware(id)?;
                let update = firmware.update.as_ref()?;
                let boot_rom = firmware.boot_rom.as_ref()?;
                if !self.files.contains_key(&update.sha256)
                    || !self.files.contains_key(&boot_rom.content.sha256)
                {
                    return None;
                }
                self.resolved(id).ok()?;
                Some(LibraryFirmware {
                    id: firmware.id.clone(),
                    body_id: firmware.body_id.clone(),
                    version: firmware.version.clone(),
                })
            })
            .collect()
    }

    /// The firmware's registered browser run configuration and overlay.
    pub fn resolved(&self, firmware_id: &str) -> Result<ResolvedConfiguration, LibraryError> {
        let firmware = self.firmware(firmware_id)?;
        let overlay_id = firmware
            .layouts
            .as_ref()
            .and_then(|layouts| layouts.profiles.get(firmware_id))
            .and_then(|profile| profile.browser_configuration_id.clone())
            .ok_or_else(|| LibraryError::UnregisteredFirmware(firmware_id.into()))?;
        let overlay = RunOverlay::bundled(&overlay_id).map_err(LibraryError::Catalog)?;
        let configuration_id = overlay.configuration_id.clone();
        let resolved = ResolvedConfiguration::with_overlays(
            &configuration_id,
            std::slice::from_ref(&overlay_id),
        )
        .map_err(LibraryError::Catalog)?;
        if resolved.configuration.firmware_id != firmware_id {
            return Err(LibraryError::Missing(format!(
                "run configuration {configuration_id} names firmware {}, not {firmware_id}",
                resolved.configuration.firmware_id
            )));
        }
        Ok(resolved)
    }

    pub(crate) fn update(&self, firmware_id: &str) -> Result<PathBuf, LibraryError> {
        let content = self.firmware(firmware_id)?.update.as_ref().ok_or_else(|| {
            LibraryError::Missing(format!(
                "firmware {firmware_id} has no stock update identity"
            ))
        })?;
        self.file(&content.sha256, firmware_id, "stock update")
    }

    pub(crate) fn boot_rom(&self, firmware_id: &str) -> Result<PathBuf, LibraryError> {
        let content = self
            .firmware(firmware_id)?
            .boot_rom
            .as_ref()
            .ok_or_else(|| {
                LibraryError::Missing(format!("firmware {firmware_id} has no boot ROM identity"))
            })?;
        self.file(&content.content.sha256, firmware_id, "boot ROM")
    }

    fn file(&self, sha256: &str, firmware_id: &str, kind: &str) -> Result<PathBuf, LibraryError> {
        self.files.get(sha256).cloned().ok_or_else(|| {
            LibraryError::Missing(format!(
                "firmware {firmware_id}: no {kind} with sha256 {sha256} in {}",
                self.root.display()
            ))
        })
    }

    /// The zstd-compressed NAND sections of a body's camera.
    pub(crate) fn camera(&self, body_id: &str, camera_id: &str) -> Result<PathBuf, LibraryError> {
        if let Some(directory) = self.cameras.get(camera_id) {
            return Ok(directory.clone());
        }
        if let Some(directory) = embedded_camera(body_id, camera_id) {
            return directory.map_err(|source| LibraryError::Read {
                path: self.root.clone(),
                source,
            });
        }
        Err(LibraryError::Missing(format!(
            "{body_id} camera {camera_id} has no NAND sections in {}",
            self.root.display()
        )))
    }
}

fn read_dir(path: &Path) -> Result<Vec<PathBuf>, LibraryError> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path).map_err(|source| LibraryError::Read {
        path: path.to_owned(),
        source,
    })? {
        entries.push(
            entry
                .map_err(|source| LibraryError::Read {
                    path: path.to_owned(),
                    source,
                })?
                .path(),
        );
    }
    entries.sort();
    Ok(entries)
}

/// Every update and boot ROM identity the catalog declares.
fn known_files(catalog: &ReferenceCatalog) -> BTreeSet<String> {
    let mut known = BTreeSet::new();
    for id in catalog.firmware_ids() {
        let Some(firmware) = catalog.firmware(id) else {
            continue;
        };
        if let Some(update) = &firmware.update {
            known.insert(update.sha256.clone());
        }
        if let Some(rom) = &firmware.boot_rom {
            known.insert(rom.content.sha256.clone());
        }
    }
    known
}

/// The catalog's checked reference camera sections.
fn embedded_camera(body_id: &str, camera_id: &str) -> Option<Result<PathBuf, std::io::Error>> {
    if body_id != "x70" || camera_id != "reference" {
        return None;
    }
    Some(Ok(PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../catalog/bodies/x70/cameras/reference"
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fujimod-camera-library-{}-{name}",
            std::process::id()
        ))
    }

    #[test]
    fn a_file_the_catalog_does_not_name_is_refused() {
        let directory = temporary("unknown");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("unrelated.bin"), b"not a firmware").unwrap();
        let error = match Library::new(&directory) {
            Err(error) => error,
            Ok(_) => panic!("expected the unknown file to be refused"),
        };
        match error {
            LibraryError::UnknownFile { path, sha256 } => {
                assert_eq!(path, directory.join("unrelated.bin"));
                assert_eq!(sha256, format!("{:x}", Sha256::digest(b"not a firmware")));
            }
            other => panic!("expected the unknown file to be refused: {other:?}"),
        }
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn the_reference_camera_comes_from_the_catalog() {
        let directory = temporary("reference");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let library = Library::new(&directory).unwrap();
        let camera = library.camera("x70", "reference").unwrap();
        assert!(camera.is_dir(), "{} is not a directory", camera.display());
        assert!(camera.join("nand-configuration.bin.zst").is_file());
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn firmwares_need_both_files_and_a_run_configuration() {
        let catalog = ReferenceCatalog::bundled().unwrap();
        let firmware = catalog.firmware("x70-110").unwrap();
        let update = firmware.update.as_ref().unwrap().sha256.clone();
        let boot_rom = firmware.boot_rom.as_ref().unwrap().content.sha256.clone();
        let mut files = BTreeMap::new();
        files.insert(update.clone(), PathBuf::from("update"));
        files.insert(boot_rom.clone(), PathBuf::from("boot-rom"));
        let library = Library {
            root: PathBuf::new(),
            catalog,
            files,
            cameras: BTreeMap::new(),
        };
        assert_eq!(
            library.firmwares(),
            [LibraryFirmware {
                id: "x70-110".into(),
                body_id: "x70".into(),
                version: "1.10".into(),
            }]
        );
        let mut partial = BTreeMap::new();
        partial.insert(update, PathBuf::from("update"));
        let library = Library {
            root: PathBuf::new(),
            catalog: ReferenceCatalog::bundled().unwrap(),
            files: partial,
            cameras: BTreeMap::new(),
        };
        assert!(library.firmwares().is_empty());
    }
}
