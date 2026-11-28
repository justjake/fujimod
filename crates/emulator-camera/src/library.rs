//! Firmware source records, and the catalog resolution that turns a body and
//! a firmware choice into a resolved camera.
//!
//! A request names a body and, optionally, one of its firmwares. The body
//! record is the hardware and the firmware record says how it starts; a
//! firmware's run configuration, `configurations/emulator/<firmware-id>/run.json`
//! when it has one, adds only session defaults (execution, scene, settings).
//! Nothing maps a firmware to the hardware it runs on.
//!
//! A firmware library is a directory of source records:
//!
//! ```text
//! firmware/<model>/<version>/firmware.json
//! ```
//!
//! The root is `FUJIMOD_FIRMWARE_LIBRARY`, or the current directory when it is
//! unset. Paths in a record are relative to that root. The library never writes;
//! it reads a record's sources once and decodes zstd in memory.
use fujimod_catalog::{ReferenceCatalog, reference::ContentIdentity};
use fujimod_catalog_configuration::RunConfiguration;
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_boot::firmware_file;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

/// The environment variable naming the firmware library directory.
pub const LIBRARY_ENV: &str = "FUJIMOD_FIRMWARE_LIBRARY";
/// The directory under a library root that holds source records.
pub const RECORD_DIRECTORY: &str = "firmware";
/// The source record's file name in a `<model>/<version>` directory.
pub const RECORD_FILE: &str = "firmware.json";

/// A firmware library could not be read or did not hold the catalog's files.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("{path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {message}")]
    Record { path: PathBuf, message: String },
    #[error("{path}: {message}")]
    Input { path: PathBuf, message: String },
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
    #[error("body {0} is not in the catalog")]
    UnknownBody(String),
    #[error("body {0} has no firmware in the catalog")]
    NoFirmware(String),
    #[error("{firmware} is {owner} firmware, not {body}")]
    OtherBody {
        firmware: String,
        owner: String,
        body: String,
    },
    #[error("body {body} has no firmware {version}; the catalog has {known}")]
    UnknownVersion {
        body: String,
        version: String,
        known: String,
    },
    #[error(
        "body {body} cannot start from an empty flash yet: the {processor} camera builder starts only installed firmware"
    )]
    EmptyFlash { body: String, processor: String },
    #[error(
        "firmware {0} has no run configuration (configurations/emulator/{0}/run.json) for its \
         overlays"
    )]
    NoRunConfiguration(String),
    #[error("{0}")]
    Missing(String),
}

/// The firmware a request opens on its body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FirmwareChoice {
    /// The newest firmware the catalog has for the body, by version.
    #[default]
    Newest,
    /// An empty flash: the boot ROM alone.
    Empty,
    /// A version such as `7.30`, or a firmware identity such as `xh2s-730`.
    Named(String),
}

impl FirmwareChoice {
    /// A request's firmware field: omitted is the newest, `none` an empty
    /// flash, anything else a version or identity.
    pub fn parse(text: Option<&str>) -> Self {
        match text.map(str::trim) {
            None | Some("") => Self::Newest,
            Some("none") => Self::Empty,
            Some(name) => Self::Named(name.to_owned()),
        }
    }
}

/// A directory of firmware source records named by catalog identity.
pub struct Library {
    root: PathBuf,
    catalog: ReferenceCatalog,
    /// Records by firmware identity, read once when the library is made.
    records: BTreeMap<String, Record>,
}

/// A catalog firmware whose update and boot ROM this library holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryFirmware {
    pub id: String,
    pub body_id: String,
    pub version: String,
}

/// The catalog-named files of one firmware, decoded in memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirmwareFiles {
    pub update: Vec<u8>,
    /// SHA-256 of `update`.
    pub update_sha256: String,
    /// The boot ROM, for a start that runs it.
    pub boot_rom: Option<Vec<u8>>,
    /// SHA-256 of `boot_rom`.
    pub boot_rom_sha256: Option<String>,
}

/// One `firmware/<model>/<version>/firmware.json` record.
#[derive(Debug)]
struct Record {
    path: PathBuf,
    source: Option<Source>,
    boot_rom: Option<BootRom>,
}

/// A stored file and the bytes it holds.
#[derive(Clone, Debug, Deserialize)]
struct Source {
    path: String,
    size: u64,
    sha256: String,
    #[serde(default)]
    compression: Option<String>,
    #[serde(default)]
    original: Option<Identity>,
}

#[derive(Clone, Debug, Deserialize)]
struct Identity {
    size: u64,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
struct BootRom {
    content: Identity,
    source: Source,
}

#[derive(Deserialize)]
struct Document {
    id: String,
    #[serde(default)]
    source: Option<Source>,
    #[serde(default)]
    boot_rom: Option<BootRom>,
}

impl Source {
    /// The bytes a reader of the record sees: the decoded original when the
    /// file is compressed, else the stored file itself.
    fn identity(&self) -> Identity {
        self.original.clone().unwrap_or_else(|| Identity {
            size: self.size,
            sha256: self.sha256.clone(),
        })
    }
}

impl Library {
    /// Read the directory named by `FUJIMOD_FIRMWARE_LIBRARY`, or the current
    /// directory when it is unset.
    pub fn from_env() -> Result<Self, LibraryError> {
        let path = std::env::var_os(LIBRARY_ENV)
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        Self::new(path)
    }

    /// Read a library directory and its source records. No firmware file is
    /// read until a firmware is opened.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, LibraryError> {
        let root = path.into();
        let catalog = ReferenceCatalog::bundled().map_err(LibraryError::Catalog)?;
        let records = scan(&root)?;
        Ok(Self {
            root,
            catalog,
            records,
        })
    }

    /// The catalog's records without a directory of firmware files.
    ///
    /// An open that names its update and boot ROM reads no record, but still
    /// needs the catalog's hashes and run configuration.
    pub fn without_directory() -> Result<Self, LibraryError> {
        Ok(Self {
            root: PathBuf::new(),
            catalog: ReferenceCatalog::bundled().map_err(LibraryError::Catalog)?,
            records: BTreeMap::new(),
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

    /// The body record for a body identity.
    pub fn body(&self, id: &str) -> Result<&fujimod_catalog::reference::Body, LibraryError> {
        self.catalog
            .body(id)
            .ok_or_else(|| LibraryError::UnknownBody(id.into()))
    }

    /// A body's product name, such as `X-H2S`, from the catalog's firmware
    /// releases; the upper-case identity when the catalog lists none.
    pub fn body_name(&self, body_id: &str) -> String {
        self.catalog
            .firmware_versions()
            .iter()
            .find(|release| {
                self.catalog
                    .firmware(&release.firmware_id)
                    .is_some_and(|firmware| firmware.body_id == body_id)
            })
            .map(|release| release.product.clone())
            .unwrap_or_else(|| body_id.to_ascii_uppercase())
    }

    /// Every catalog firmware of a body, oldest version first.
    pub fn body_firmwares(&self, body_id: &str) -> Vec<&fujimod_catalog::Firmware> {
        let mut firmwares: Vec<_> = self
            .catalog
            .firmware_ids()
            .filter_map(|id| self.catalog.firmware(id))
            .filter(|firmware| firmware.body_id == body_id)
            .collect();
        firmwares.sort_by(|left, right| {
            version_key(&left.version)
                .cmp(&version_key(&right.version))
                .then_with(|| left.id.cmp(&right.id))
        });
        firmwares
    }

    /// The firmware identity a body and firmware choice select.
    pub fn select(&self, body_id: &str, choice: &FirmwareChoice) -> Result<String, LibraryError> {
        let body = self.body(body_id)?;
        let firmwares = self.body_firmwares(body_id);
        match choice {
            FirmwareChoice::Newest => firmwares
                .last()
                .map(|firmware| firmware.id.clone())
                .ok_or_else(|| LibraryError::NoFirmware(body_id.into())),
            FirmwareChoice::Empty => Err(LibraryError::EmptyFlash {
                body: body_id.into(),
                processor: processor_name(&body.soc_id),
            }),
            FirmwareChoice::Named(name) => {
                if let Some(firmware) = self.catalog.firmware(name) {
                    if firmware.body_id != body_id {
                        return Err(LibraryError::OtherBody {
                            firmware: firmware.id.clone(),
                            owner: self.body_name(&firmware.body_id),
                            body: self.body_name(body_id),
                        });
                    }
                    return Ok(firmware.id.clone());
                }
                if let Some(firmware) = firmwares.iter().find(|firmware| firmware.version == *name)
                {
                    return Ok(firmware.id.clone());
                }
                Err(LibraryError::UnknownVersion {
                    body: body_id.into(),
                    version: name.clone(),
                    known: if firmwares.is_empty() {
                        "none".into()
                    } else {
                        firmwares
                            .iter()
                            .map(|firmware| firmware.version.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    },
                })
            }
        }
    }

    /// Catalog firmwares this library holds the files of and that plan to
    /// open, in catalog identity order.
    pub fn firmwares(&self) -> Vec<LibraryFirmware> {
        self.catalog
            .firmware_ids()
            .filter_map(|id| {
                let firmware = self.catalog.firmware(id)?;
                let request =
                    crate::Request::new(&firmware.body_id, FirmwareChoice::Named(id.into()));
                let plan = crate::plan(self, &request).ok()?;
                let update = firmware.update.as_ref()?;
                let record = self.records.get(id)?;
                let source = record.source.as_ref()?;
                let identity = source.identity();
                if identity.size != update.size || identity.sha256 != update.sha256 {
                    return None;
                }
                if !self.source_present(record, source) {
                    return None;
                }
                if plan.boot_rom == fujimod_emulator_boot::BootRomUse::Required {
                    let rom = firmware.boot_rom.as_ref()?;
                    let rom_record = self.boot_rom_record(&rom.content.sha256)?;
                    if !self.source_present(rom_record, &rom_record.boot_rom.as_ref()?.source) {
                        return None;
                    }
                }
                Some(LibraryFirmware {
                    id: firmware.id.clone(),
                    body_id: firmware.body_id.clone(),
                    version: firmware.version.clone(),
                })
            })
            .collect()
    }

    /// The firmware's catalog-named update, and its boot ROM when the start
    /// runs one, decoded in memory.
    pub fn files(&self, firmware_id: &str, boot_rom: bool) -> Result<FirmwareFiles, LibraryError> {
        let firmware = self.firmware(firmware_id)?;
        let update_identity = firmware.update.as_ref().ok_or_else(|| {
            LibraryError::Missing(format!("firmware {firmware_id} has no update identity"))
        })?;
        let record = self.records.get(firmware_id).ok_or_else(|| {
            LibraryError::Missing(format!(
                "no {RECORD_FILE} record for {firmware_id} under {}",
                self.root.join(RECORD_DIRECTORY).display()
            ))
        })?;
        let source = record.source.as_ref().ok_or_else(|| {
            LibraryError::Missing(format!(
                "{}: firmware {firmware_id} record has no update source",
                record.path.display()
            ))
        })?;
        let (update, update_sha256) = self.update(record, source, update_identity, firmware_id)?;
        let (boot_rom, boot_rom_sha256) = if boot_rom {
            let boot_identity = firmware.boot_rom.as_ref().ok_or_else(|| {
                LibraryError::Missing(format!("firmware {firmware_id} has no boot ROM identity"))
            })?;
            let (bytes, sha256) = self.boot_rom(firmware_id, &boot_identity.content)?;
            (Some(bytes), Some(sha256))
        } else {
            (None, None)
        };
        Ok(FirmwareFiles {
            update,
            update_sha256,
            boot_rom,
            boot_rom_sha256,
        })
    }

    /// Read caller-named files, which must match the catalog's entry for the
    /// firmware. zstd frames are decoded by their magic number.
    pub fn files_from(
        &self,
        firmware_id: &str,
        update: &Path,
        boot_rom: Option<&Path>,
    ) -> Result<FirmwareFiles, LibraryError> {
        let firmware = self.firmware(firmware_id)?;
        let update_identity = firmware.update.as_ref().ok_or_else(|| {
            LibraryError::Missing(format!("firmware {firmware_id} has no update identity"))
        })?;
        let update_bytes = read_input(update)?;
        let update_sha256 = digest(&update_bytes);
        if update_bytes.len() as u64 != update_identity.size
            || update_sha256 != update_identity.sha256
        {
            return Err(LibraryError::Mismatch {
                path: update.to_owned(),
                sha256: update_sha256,
                kind: "stock update",
                firmware: firmware_id.into(),
            });
        }
        let (boot_rom, boot_rom_sha256) = match boot_rom {
            None => (None, None),
            Some(boot_rom) => {
                let boot_identity = firmware.boot_rom.as_ref().ok_or_else(|| {
                    LibraryError::Missing(format!(
                        "firmware {firmware_id} has no boot ROM identity"
                    ))
                })?;
                let captured = read_input(boot_rom)?;
                let content = &boot_identity.content;
                let bytes = capture(
                    &captured,
                    content.size,
                    &content.sha256,
                    boot_rom,
                    firmware_id,
                )?;
                let sha256 = digest(bytes);
                (Some(bytes.to_vec()), Some(sha256))
            }
        };
        Ok(FirmwareFiles {
            update: update_bytes,
            update_sha256,
            boot_rom,
            boot_rom_sha256,
        })
    }

    /// The firmware resolved on its body: its run configuration,
    /// `configurations/emulator/<firmware-id>`, with the overlays, or its body
    /// and firmware records alone when it has none.
    pub fn resolved(
        &self,
        firmware_id: &str,
        overlays: &[String],
    ) -> Result<ResolvedConfiguration, LibraryError> {
        self.firmware(firmware_id)?;
        let resolved = if RunConfiguration::bundled_ids().any(|id| id == firmware_id) {
            ResolvedConfiguration::with_overlays(firmware_id, overlays)
        } else if overlays.is_empty() {
            // Without a run configuration the camera opens from its body and
            // firmware records and the processor's own choices.
            ResolvedConfiguration::from_records(firmware_id)
        } else {
            return Err(LibraryError::NoRunConfiguration(firmware_id.into()));
        }
        .map_err(LibraryError::Catalog)?;
        if resolved.configuration.firmware_id != firmware_id {
            return Err(LibraryError::Missing(format!(
                "run configuration {firmware_id} names firmware {}, not {firmware_id}",
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

    /// The first record whose boot ROM content has `sha256`.
    fn boot_rom_record(&self, sha256: &str) -> Option<&Record> {
        self.records.values().find(|record| {
            record
                .boot_rom
                .as_ref()
                .is_some_and(|rom| rom.content.sha256 == sha256)
        })
    }

    /// Whether a source names a readable regular file of its declared size.
    fn source_present(&self, record: &Record, source: &Source) -> bool {
        self.source_path(record, source)
            .ok()
            .and_then(|path| fs::metadata(path).ok())
            .is_some_and(|metadata| metadata.is_file() && metadata.len() == source.size)
    }

    /// Read the record's update and confirm it is the catalog's.
    fn update(
        &self,
        record: &Record,
        source: &Source,
        catalog: &ContentIdentity,
        firmware: &str,
    ) -> Result<(Vec<u8>, String), LibraryError> {
        let (path, bytes) = self.read_source(record, source, "stock update", firmware)?;
        let sha256 = digest(&bytes);
        if bytes.len() as u64 != catalog.size || sha256 != catalog.sha256 {
            return Err(LibraryError::Mismatch {
                path,
                sha256,
                kind: "stock update",
                firmware: firmware.into(),
            });
        }
        Ok((bytes, sha256))
    }

    /// Read the boot ROM any record holds for the catalog's content hash.
    fn boot_rom(
        &self,
        firmware: &str,
        catalog: &ContentIdentity,
    ) -> Result<(Vec<u8>, String), LibraryError> {
        let record = self.boot_rom_record(&catalog.sha256).ok_or_else(|| {
            LibraryError::Missing(format!(
                "no {RECORD_FILE} under {} holds the boot ROM {}",
                self.root.join(RECORD_DIRECTORY).display(),
                catalog.sha256
            ))
        })?;
        let rom = record.boot_rom.as_ref().expect("selected boot ROM record");
        let (path, captured) = self.read_source(record, &rom.source, "boot ROM", firmware)?;
        let bytes = capture(
            &captured,
            rom.content.size,
            &rom.content.sha256,
            &path,
            firmware,
        )?;
        if rom.content.size != catalog.size || rom.content.sha256 != catalog.sha256 {
            return Err(LibraryError::Mismatch {
                path,
                sha256: rom.content.sha256.clone(),
                kind: "boot ROM",
                firmware: firmware.into(),
            });
        }
        Ok((bytes.to_vec(), rom.content.sha256.clone()))
    }

    /// Read a source once, confirming the stored file and, when compressed,
    /// the decoded original.
    fn read_source(
        &self,
        record: &Record,
        source: &Source,
        kind: &'static str,
        firmware: &str,
    ) -> Result<(PathBuf, Vec<u8>), LibraryError> {
        let path = self.source_path(record, source)?;
        let bytes = firmware_file::read_encoded(&path, source.size).map_err(|message| {
            LibraryError::Input {
                path: path.clone(),
                message,
            }
        })?;
        let stored = digest(&bytes);
        if bytes.len() as u64 != source.size || stored != source.sha256 {
            return Err(LibraryError::Mismatch {
                path,
                sha256: stored,
                kind,
                firmware: firmware.into(),
            });
        }
        match (&source.compression, &source.original) {
            (None, None) => Ok((path, bytes)),
            (Some(method), Some(original)) if method == "zstd" => {
                let decoded =
                    firmware_file::decode(bytes).map_err(|message| LibraryError::Input {
                        path: path.clone(),
                        message,
                    })?;
                let sha256 = digest(&decoded);
                if decoded.len() as u64 != original.size || sha256 != original.sha256 {
                    return Err(LibraryError::Mismatch {
                        path,
                        sha256,
                        kind,
                        firmware: firmware.into(),
                    });
                }
                Ok((path, decoded))
            }
            (Some(method), _) => Err(LibraryError::Record {
                path: record.path.clone(),
                message: format!("unsupported source compression {method:?}"),
            }),
            (None, Some(_)) => Err(LibraryError::Record {
                path: record.path.clone(),
                message: "a source with an original identity must name zstd compression".into(),
            }),
        }
    }

    /// Resolve a record's relative source path under the library root.
    fn source_path(&self, record: &Record, source: &Source) -> Result<PathBuf, LibraryError> {
        let relative = Path::new(&source.path);
        if self.root.as_os_str().is_empty()
            || relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative.to_string_lossy().contains('\\')
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(LibraryError::Record {
                path: record.path.clone(),
                message: format!(
                    "source path {:?} must be relative to the library root",
                    source.path
                ),
            });
        }
        let path = self.root.join(relative);
        if path.as_os_str().is_empty() {
            return Err(LibraryError::Record {
                path: record.path.clone(),
                message: "source path names no file".into(),
            });
        }
        Ok(path)
    }
}

/// A version's numeric parts, so `10.1` sorts after `9.30`.
fn version_key(version: &str) -> Vec<u64> {
    version
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().unwrap_or(u64::MAX))
        .collect()
}

/// A processor's name in refusals.
pub(crate) fn processor_name(soc_id: &str) -> String {
    match soc_id {
        "exr2" => "EXR II".into(),
        "xp4" => "X-Processor 4".into(),
        "xp5" => "X-Processor 5".into(),
        "xpp" => "X-Processor Pro".into(),
        other => other.into(),
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The ROM a capture holds: its first `content.size` bytes.
fn capture<'a>(
    bytes: &'a [u8],
    size: u64,
    expected: &str,
    path: &Path,
    firmware: &str,
) -> Result<&'a [u8], LibraryError> {
    let size = usize::try_from(size).map_err(|_| {
        LibraryError::Missing(format!(
            "{}: boot ROM content size does not fit in memory",
            path.display()
        ))
    })?;
    let rom = bytes.get(..size).ok_or_else(|| {
        LibraryError::Missing(format!(
            "{}: boot ROM capture is {} bytes, shorter than the {} the record names",
            path.display(),
            bytes.len(),
            size
        ))
    })?;
    let sha256 = digest(rom);
    if sha256 != expected {
        return Err(LibraryError::Mismatch {
            path: path.to_owned(),
            sha256,
            kind: "boot ROM",
            firmware: firmware.into(),
        });
    }
    Ok(rom)
}

fn read_input(path: &Path) -> Result<Vec<u8>, LibraryError> {
    firmware_file::read(path, firmware_file::MAX_BYTES).map_err(|message| LibraryError::Input {
        path: path.to_owned(),
        message,
    })
}

/// Read every `firmware/<model>/<version>/firmware.json` record under `root`.
fn scan(root: &Path) -> Result<BTreeMap<String, Record>, LibraryError> {
    let directory = root.join(RECORD_DIRECTORY);
    let models = match fs::read_dir(&directory) {
        Ok(models) => models,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BTreeMap::new());
        }
        Err(source) => {
            return Err(LibraryError::Read {
                path: directory,
                source,
            });
        }
    };
    let mut paths = Vec::new();
    for model in models {
        let model = model.map_err(|source| LibraryError::Read {
            path: directory.clone(),
            source,
        })?;
        if !model
            .file_type()
            .map_err(|source| LibraryError::Read {
                path: model.path(),
                source,
            })?
            .is_dir()
        {
            continue;
        }
        let versions = fs::read_dir(model.path()).map_err(|source| LibraryError::Read {
            path: model.path(),
            source,
        })?;
        for version in versions {
            let version = version.map_err(|source| LibraryError::Read {
                path: model.path(),
                source,
            })?;
            if !version
                .file_type()
                .map_err(|source| LibraryError::Read {
                    path: version.path(),
                    source,
                })?
                .is_dir()
            {
                continue;
            }
            let record = version.path().join(RECORD_FILE);
            if record.is_file() {
                paths.push(record);
            }
        }
    }
    paths.sort();
    let mut records = BTreeMap::new();
    for path in paths {
        let bytes = fs::read(&path).map_err(|source| LibraryError::Read {
            path: path.clone(),
            source,
        })?;
        let document: Document =
            serde_json::from_slice(&bytes).map_err(|error| LibraryError::Record {
                path: path.clone(),
                message: error.to_string(),
            })?;
        if document.id.is_empty() {
            return Err(LibraryError::Record {
                path,
                message: "record has no firmware identity".into(),
            });
        }
        if records.contains_key(&document.id) {
            return Err(LibraryError::Record {
                path,
                message: format!("duplicate firmware identity {}", document.id),
            });
        }
        records.insert(
            document.id,
            Record {
                path,
                source: document.source,
                boot_rom: document.boot_rom,
            },
        );
    }
    Ok(records)
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

    fn write_record(directory: &Path, id: &str, record: serde_json::Value) {
        write_record_at(directory, "x70", "1.10", id, record);
    }

    fn write_record_at(
        directory: &Path,
        model: &str,
        version: &str,
        id: &str,
        record: serde_json::Value,
    ) {
        let path = directory.join(format!("firmware/{model}/{version}"));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join(RECORD_FILE), serde_json::to_vec(&record).unwrap()).unwrap();
        assert_eq!(record["id"], id);
    }

    #[test]
    fn a_firmware_without_a_record_is_missing() {
        let directory = temporary("missing-record");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let library = Library::new(&directory).unwrap();
        let error = library.files("x70-110", true).unwrap_err();
        assert!(matches!(error, LibraryError::Missing(_)), "{error}");
        assert!(error.to_string().contains("no firmware.json record"));
        assert!(library.firmwares().is_empty());
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_compressed_update_is_decoded_before_it_is_compared() {
        let directory = temporary("zstd-update");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let original = b"a decoded update, not the catalog's";
        let packed = zstd::stream::encode_all(original.as_slice(), 1).unwrap();
        fs::write(directory.join("update.dat.zst"), &packed).unwrap();
        write_record(
            &directory,
            "x70-110",
            serde_json::json!({
                "id": "x70-110",
                "source": {
                    "path": "update.dat.zst",
                    "size": packed.len(),
                    "sha256": digest(&packed),
                    "compression": "zstd",
                    "original": { "size": original.len(), "sha256": digest(original) }
                }
            }),
        );
        let library = Library::new(&directory).unwrap();
        let error = library.files("x70-110", true).unwrap_err();
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
        assert!(error.to_string().contains(&digest(original)), "{error}");
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_source_path_must_stay_under_the_library_root() {
        let directory = temporary("escape");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        write_record(
            &directory,
            "x70-110",
            serde_json::json!({
                "id": "x70-110",
                "source": { "path": "../outside.dat", "size": 1, "sha256": "0".repeat(64) }
            }),
        );
        let library = Library::new(&directory).unwrap();
        let error = library.files("x70-110", true).unwrap_err();
        assert!(matches!(error, LibraryError::Record { .. }), "{error}");
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_boot_rom_comes_from_any_record_with_the_catalog_content_hash() {
        let directory = temporary("shared-rom");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let rom = b"the first bytes are the ROM and the rest is capture";
        let packed = zstd::stream::encode_all(rom.as_slice(), 1).unwrap();
        fs::write(directory.join("rom.zst"), &packed).unwrap();
        write_record(
            &directory,
            "x70-110",
            serde_json::json!({ "id": "x70-110" }),
        );
        write_record_at(
            &directory,
            "x-e3",
            "1.32",
            "xe3-132",
            serde_json::json!({
                "id": "xe3-132",
                "boot_rom": {
                    "content": { "size": 10, "sha256": digest(&rom[..10]) },
                    "source": {
                        "path": "rom.zst",
                        "size": packed.len(),
                        "sha256": digest(&packed),
                        "compression": "zstd",
                        "original": { "size": rom.len(), "sha256": digest(rom) }
                    }
                }
            }),
        );
        let library = Library::new(&directory).unwrap();
        let content = ContentIdentity {
            size: 10,
            sha256: digest(&rom[..10]),
        };
        let (bytes, sha256) = library.boot_rom("x70-110", &content).unwrap();
        assert_eq!(bytes, rom[..10]);
        assert_eq!(sha256, content.sha256);
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_boot_rom_whose_record_names_another_content_is_refused() {
        let directory = temporary("wrong-rom-content");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let rom = b"the boot rom";
        fs::write(directory.join("rom.bin"), rom).unwrap();
        write_record(
            &directory,
            "x70-110",
            serde_json::json!({
                "id": "x70-110",
                "boot_rom": {
                    "content": { "size": rom.len(), "sha256": "0".repeat(64) },
                    "source": {
                        "path": "rom.bin",
                        "size": rom.len(),
                        "sha256": digest(rom)
                    }
                }
            }),
        );
        let library = Library::new(&directory).unwrap();
        let content = ContentIdentity {
            size: rom.len() as u64,
            sha256: "0".repeat(64),
        };
        let error = library.boot_rom("x70-110", &content).unwrap_err();
        assert!(
            matches!(
                error,
                LibraryError::Mismatch {
                    kind: "boot ROM",
                    ..
                }
            ),
            "{error}"
        );
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
