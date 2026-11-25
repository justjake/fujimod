//! Firmware files found by name, and the catalog resolution that names the run
//! configuration for a firmware.
//!
//! A firmware library is a directory laid out by name:
//!
//! ```text
//! <firmware-id>/FPUPDATE.DAT
//! <body-id>/boot-rom.bin
//! ```
//!
//! The library is never scanned for files and never written. Each file is
//! found by its name and hashed once to confirm it matches the catalog's
//! entry for the firmware.
use fujimod_catalog::ReferenceCatalog;
use fujimod_catalog_configuration::RunOverlay;
use fujimod_catalog_resolution::ResolvedConfiguration;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

/// The environment variable naming the firmware library directory.
pub const LIBRARY_ENV: &str = "FUJIMOD_FIRMWARE_LIBRARY";
/// The stock update file name inside a `<firmware-id>` directory.
pub const UPDATE_FILE: &str = "FPUPDATE.DAT";
/// The boot ROM file name inside a `<body-id>` directory.
pub const BOOT_ROM_FILE: &str = "boot-rom.bin";

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
    #[error("{path}: sha256 {sha256} is not the catalog's {kind} for {firmware}")]
    Mismatch {
        path: PathBuf,
        sha256: String,
        kind: &'static str,
        firmware: String,
    },
    #[error("firmware {0} is not in the catalog")]
    UnknownFirmware(String),
    #[error("firmware {0} declares no browser run configuration")]
    UnregisteredFirmware(String),
    #[error("{0}")]
    Missing(String),
}

/// A directory of firmware files named by catalog identity.
pub struct Library {
    root: PathBuf,
    catalog: ReferenceCatalog,
    /// SHA-256 of each file this library already read, by path.
    hashed: Mutex<BTreeMap<PathBuf, String>>,
}

/// A catalog firmware whose update and boot ROM this library holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryFirmware {
    pub id: String,
    pub body_id: String,
    pub version: String,
}

/// The catalog-named files of one firmware.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirmwareFiles {
    pub update: PathBuf,
    /// SHA-256 of `update`.
    pub update_sha256: String,
    pub boot_rom: PathBuf,
    /// SHA-256 of `boot_rom`.
    pub boot_rom_sha256: String,
}

impl Library {
    /// Read the directory named by the library environment variable.
    pub fn from_env() -> Result<Self, LibraryError> {
        let path = std::env::var_os(LIBRARY_ENV).ok_or(LibraryError::Environment)?;
        Self::new(PathBuf::from(path))
    }

    /// Read a library directory. No file is read until a firmware is opened.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, LibraryError> {
        let root = path.into();
        let catalog = ReferenceCatalog::bundled().map_err(LibraryError::Catalog)?;
        Ok(Self {
            root,
            catalog,
            hashed: Mutex::new(BTreeMap::new()),
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

    /// Catalog firmwares this library holds both files for and has a browser
    /// run configuration for, in catalog identity order.
    pub fn firmwares(&self) -> Vec<LibraryFirmware> {
        self.catalog
            .firmware_ids()
            .filter_map(|id| {
                let firmware = self.catalog.firmware(id)?;
                firmware.update.as_ref()?;
                firmware.boot_rom.as_ref()?;
                // Listing confirms each file against the catalog, once per
                // library; the launcher then shows only files it can open.
                self.files(id).ok()?;
                self.resolved(id).ok()?;
                Some(LibraryFirmware {
                    id: firmware.id.clone(),
                    body_id: firmware.body_id.clone(),
                    version: firmware.version.clone(),
                })
            })
            .collect()
    }

    /// The firmware's catalog-named update and boot ROM, each hashed once.
    pub fn files(&self, firmware_id: &str) -> Result<FirmwareFiles, LibraryError> {
        self.firmware(firmware_id)?;
        let update = self.update_path(firmware_id);
        let boot_rom = self.boot_rom_path(firmware_id);
        let update_sha256 = self.hash(&update, firmware_id, "stock update")?;
        let boot_rom_sha256 = self.hash(&boot_rom, firmware_id, "boot ROM")?;
        Ok(FirmwareFiles {
            update,
            update_sha256,
            boot_rom,
            boot_rom_sha256,
        })
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

    /// The firmware's located NAND bootloader facts.
    pub fn nand_boot(
        &self,
        firmware_id: &str,
    ) -> Result<&fujimod_catalog::nand_boot::FirmwareNandBoot, LibraryError> {
        self.catalog.nand_boot(firmware_id).ok_or_else(|| {
            LibraryError::Missing(format!("firmware {firmware_id} has no NAND boot facts"))
        })
    }

    /// The zstd-compressed NAND sections of a body's camera.
    pub(crate) fn camera(&self, body_id: &str, camera_id: &str) -> Result<PathBuf, LibraryError> {
        embedded_camera(body_id, camera_id).ok_or_else(|| {
            LibraryError::Missing(format!(
                "{body_id} camera {camera_id} has no NAND sections in the catalog"
            ))
        })
    }

    /// `<root>/<firmware-id>/FPUPDATE.DAT`.
    fn update_path(&self, firmware_id: &str) -> PathBuf {
        self.root.join(firmware_id).join(UPDATE_FILE)
    }

    /// `<root>/<body-id>/boot-rom.bin`.
    fn boot_rom_path(&self, firmware_id: &str) -> PathBuf {
        let body = self
            .catalog
            .firmware(firmware_id)
            .map(|firmware| firmware.body_id.as_str())
            .unwrap_or(firmware_id);
        self.root.join(body).join(BOOT_ROM_FILE)
    }

    /// Read a file once and confirm it is the catalog's `kind` for the firmware.
    fn hash(
        &self,
        path: &Path,
        firmware_id: &str,
        kind: &'static str,
    ) -> Result<String, LibraryError> {
        let catalog = self.firmware(firmware_id)?;
        let expected = match kind {
            "stock update" => catalog.update.as_ref().map(|update| update.sha256.as_str()),
            _ => catalog
                .boot_rom
                .as_ref()
                .map(|rom| rom.content.sha256.as_str()),
        }
        .ok_or_else(|| {
            LibraryError::Missing(format!("firmware {firmware_id} has no {kind} identity"))
        })?;
        if let Some(sha256) = self
            .hashed
            .lock()
            .ok()
            .and_then(|hashed| hashed.get(path).cloned())
        {
            if sha256 != expected {
                return Err(LibraryError::Mismatch {
                    path: path.to_owned(),
                    sha256,
                    kind,
                    firmware: firmware_id.into(),
                });
            }
            return Ok(sha256);
        }
        let bytes = fs::read(path).map_err(|source| LibraryError::Read {
            path: path.to_owned(),
            source,
        })?;
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        if let Ok(mut hashed) = self.hashed.lock() {
            hashed.insert(path.to_owned(), sha256.clone());
        }
        if sha256 != expected {
            return Err(LibraryError::Mismatch {
                path: path.to_owned(),
                sha256,
                kind,
                firmware: firmware_id.into(),
            });
        }
        Ok(sha256)
    }
}

/// The catalog's checked reference camera sections.
fn embedded_camera(body_id: &str, camera_id: &str) -> Option<PathBuf> {
    if body_id != "x70" || camera_id != "reference" {
        return None;
    }
    Some(PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../catalog/bodies/x70/cameras/reference"
    )))
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
    fn a_firmware_is_found_by_name_and_must_match_the_catalog() {
        let directory = temporary("byname");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(directory.join("x70-110")).unwrap();
        fs::create_dir_all(directory.join("x70")).unwrap();
        fs::write(directory.join("x70-110/FPUPDATE.DAT"), b"not the update").unwrap();
        fs::write(directory.join("x70/boot-rom.bin"), b"not the rom").unwrap();
        let library = Library::new(&directory).unwrap();
        let error = library.files("x70-110").unwrap_err();
        assert!(
            matches!(
                error,
                LibraryError::Mismatch {
                    kind: "stock update",
                    ..
                }
            ),
            "{error}"
        );
        assert!(library.firmwares().is_empty());
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_missing_file_names_the_firmware_and_kind() {
        let directory = temporary("missing");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let library = Library::new(&directory).unwrap();
        let error = library.files("x70-110").unwrap_err();
        assert!(matches!(error, LibraryError::Read { .. }), "{error}");
        assert!(error.to_string().contains("x70-110/FPUPDATE.DAT"));
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
}
