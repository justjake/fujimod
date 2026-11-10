//! Release manifests and content-addressed store files for OTA releases.
//!
//! A release directory contains `latest.json` and one file per distinct
//! SHA-256, named by the first 11 uppercase hexadecimal digits of the digest
//! as `XXXXXXXX.YYY`. The manifest maps each published path to that file's
//! byte size and full digest; the camera derives the store name from the
//! digest with the same rule as its update layer.

use serde::Serialize;
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// The manifest file written into a release directory.
pub const MANIFEST_NAME: &str = "latest.json";

const COPY_BUFFER_SIZE: usize = 64 * 1024;
const HEX_DIGITS: [u8; 16] = *b"0123456789ABCDEF";

/// Why a release could not be built or written.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "firmware id must start with an ASCII letter or digit and contain only letters, digits, '-', '_' or '.'"
    )]
    InvalidFirmware,
    #[error("version must not be empty and must not contain control characters")]
    InvalidVersion,
    #[error("date must be YYYY-MM-DD")]
    InvalidDate,
    #[error("commit must contain 7 to 40 lowercase hexadecimal digits")]
    InvalidCommit,
    #[error("release notes must contain at most 8 strings of at most 64 bytes each")]
    InvalidNotes,
    #[error("a release needs at least one path=file pair")]
    NoFiles,
    #[error(
        "invalid file path {0:?}: paths start with '/', contain no empty, '.' or '..' segments, and have no trailing slash"
    )]
    InvalidPath(String),
    #[error("duplicate file path {0:?}")]
    DuplicatePath(String),
    #[error("invalid SHA-256 {0:?}: expected 64 hexadecimal digits")]
    InvalidDigest(String),
    #[error("store name {name} is shared by {first} and {second}")]
    StoreCollision {
        name: String,
        first: String,
        second: String,
    },
    #[error("store file {name} already exists with different contents, needed by {path}")]
    StoreConflict { name: String, path: String },
    #[error("{} is not a regular file", path.display())]
    NotAFile { path: PathBuf },
    #[error("{} changed while the release was written", path.display())]
    SourceChanged { path: PathBuf },
    #[error("cannot {action} {}: {source}", path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// One published path and the file it comes from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseFile {
    path: String,
    source: PathBuf,
    size: u64,
    sha256: [u8; 32],
}

impl ReleaseFile {
    /// The path shown in the manifest and used by the camera file layer.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The host file copied into the store.
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// The file's size in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The file's SHA-256 digest.
    pub fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }

    /// The lowercase hexadecimal SHA-256 written to the manifest.
    pub fn sha256_hex(&self) -> String {
        digest_hex(&self.sha256)
    }

    /// The content-addressed store file name.
    pub fn store_name(&self) -> String {
        store_name(&self.sha256)
    }
}

/// A firmware release: a firmware id, a version and the files it publishes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Release {
    firmware: String,
    version: String,
    date: Option<String>,
    commit: Option<String>,
    notes: Option<Vec<String>>,
    files: Vec<ReleaseFile>,
}

impl Release {
    /// Hashes each source file and builds a release. Paths are sorted for a
    /// deterministic manifest, and two files with the same store name are
    /// refused even when their full digests differ.
    pub fn new<F, P>(
        firmware: impl Into<String>,
        version: impl Into<String>,
        files: impl IntoIterator<Item = (F, P)>,
    ) -> Result<Self, Error>
    where
        F: Into<String>,
        P: Into<PathBuf>,
    {
        let firmware = firmware.into();
        let version = version.into();
        validate_firmware(&firmware)?;
        validate_version(&version)?;

        let mut paths = BTreeSet::new();
        let mut entries = Vec::new();
        for (path, source) in files {
            let path = path.into();
            let source = source.into();
            validate_path(&path)?;
            if !paths.insert(path.clone()) {
                return Err(Error::DuplicatePath(path));
            }
            let (size, sha256) = hash_file(&source)?;
            entries.push(ReleaseFile {
                path,
                source,
                size,
                sha256,
            });
        }
        if entries.is_empty() {
            return Err(Error::NoFiles);
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        let release = Self {
            firmware,
            version,
            date: None,
            commit: None,
            notes: None,
            files: entries,
        };
        release.check_store_names()?;
        Ok(release)
    }

    /// The firmware id written to the manifest.
    pub fn firmware(&self) -> &str {
        &self.firmware
    }

    /// The version written to the manifest.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Sets optional release provenance and short release notes.
    pub fn with_metadata(
        mut self,
        date: Option<String>,
        commit: Option<String>,
        notes: Option<Vec<String>>,
    ) -> Result<Self, Error> {
        if date.as_deref().is_some_and(|value| !valid_date(value)) {
            return Err(Error::InvalidDate);
        }
        if commit.as_deref().is_some_and(|value| !valid_commit(value)) {
            return Err(Error::InvalidCommit);
        }
        if notes.as_ref().is_some_and(|values| {
            values.len() > 8 || values.iter().any(|value| value.len() > 64)
        }) {
            return Err(Error::InvalidNotes);
        }
        self.date = date;
        self.commit = commit;
        self.notes = notes;
        Ok(self)
    }

    /// The files in manifest order.
    pub fn files(&self) -> &[ReleaseFile] {
        &self.files
    }

    /// The manifest as pretty-printed JSON with a trailing newline.
    pub fn manifest_json(&self) -> String {
        #[derive(Serialize)]
        struct ManifestFile {
            sha256: String,
            size: u64,
        }
        #[derive(Serialize)]
        struct Manifest<'a> {
            firmware: &'a str,
            version: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            date: &'a Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            commit: &'a Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            notes: &'a Option<Vec<String>>,
            fs: BTreeMap<&'a str, ManifestFile>,
        }

        let fs = self
            .files
            .iter()
            .map(|file| {
                (
                    file.path.as_str(),
                    ManifestFile {
                        sha256: file.sha256_hex(),
                        size: file.size,
                    },
                )
            })
            .collect();
        let mut text = serde_json::to_string_pretty(&Manifest {
            firmware: &self.firmware,
            version: &self.version,
            date: &self.date,
            commit: &self.commit,
            notes: &self.notes,
            fs,
        })
        .expect("release manifests are serializable");
        text.push('\n');
        text
    }

    /// Writes every store file, then `latest.json`, into `output`.
    ///
    /// Store files are written through a temporary name and renamed into
    /// place. An existing store file is reused only when its size and SHA-256
    /// match; the manifest is written last so it never names a missing file.
    pub fn write(&self, output: &Path) -> Result<(), Error> {
        fs::create_dir_all(output).map_err(|error| io_error("create", output, error))?;
        for file in &self.files {
            write_store_file(output, file)?;
        }
        self.write_manifest(output)
    }

    fn check_store_names(&self) -> Result<(), Error> {
        let mut names: BTreeMap<String, &ReleaseFile> = BTreeMap::new();
        for file in &self.files {
            let name = file.store_name();
            if let Some(previous) = names.get(&name) {
                if previous.sha256 != file.sha256 {
                    return Err(Error::StoreCollision {
                        name,
                        first: previous.path.clone(),
                        second: file.path.clone(),
                    });
                }
            } else {
                names.insert(name, file);
            }
        }
        Ok(())
    }

    fn write_manifest(&self, output: &Path) -> Result<(), Error> {
        let destination = output.join(MANIFEST_NAME);
        let temporary = temporary_path(output, MANIFEST_NAME);
        let result = write_atomic(&temporary, self.manifest_json().as_bytes());
        match result {
            Ok(()) => fs::rename(&temporary, &destination)
                .map_err(|error| io_error("rename into", &destination, error)),
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                Err(error)
            }
        }
    }
}

fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || !bytes[0..4].iter().all(u8::is_ascii_digit)
        || bytes[4] != b'-'
        || !bytes[5..7].iter().all(u8::is_ascii_digit)
        || bytes[7] != b'-'
        || !bytes[8..10].iter().all(u8::is_ascii_digit)
    {
        return false;
    }
    let year = u16::from(bytes[0] - b'0') * 1000
        + u16::from(bytes[1] - b'0') * 100
        + u16::from(bytes[2] - b'0') * 10
        + u16::from(bytes[3] - b'0');
    let month = (bytes[5] - b'0') * 10 + (bytes[6] - b'0');
    let day = (bytes[8] - b'0') * 10 + (bytes[9] - b'0');
    if year == 0 || !(1..=12).contains(&month) {
        return false;
    }
    let leap_year = year % 400 == 0 || (year % 4 == 0 && year % 100 != 0);
    let days_in_month = match month {
        2 if leap_year => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=days_in_month).contains(&day)
}

fn valid_commit(value: &str) -> bool {
    (7..=40).contains(&value.len())
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

/// The first 11 uppercase hexadecimal digits of a SHA-256 as `XXXXXXXX.YYY`.
pub fn store_name(digest: &[u8; 32]) -> String {
    let mut name = String::with_capacity(12);
    for index in 0..11 {
        if index == 8 {
            name.push('.');
        }
        let byte = digest[index / 2];
        let nibble = if index % 2 == 0 {
            byte >> 4
        } else {
            byte & 0x0f
        };
        name.push(HEX_DIGITS[nibble as usize] as char);
    }
    name
}

/// Derives a store name from a 64-digit hexadecimal SHA-256 in either case.
pub fn store_name_hex(hash: &str) -> Result<String, Error> {
    if hash.len() != 64 || !hash.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        return Err(Error::InvalidDigest(hash.to_owned()));
    }
    let mut name = String::with_capacity(12);
    for (index, character) in hash.chars().take(11).enumerate() {
        if index == 8 {
            name.push('.');
        }
        name.push(character.to_ascii_uppercase());
    }
    Ok(name)
}

/// Lowercase hexadecimal for a digest.
pub fn digest_hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn validate_firmware(firmware: &str) -> Result<(), Error> {
    let mut characters = firmware.chars();
    if !characters
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric())
        || !characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(Error::InvalidFirmware);
    }
    Ok(())
}

fn validate_version(version: &str) -> Result<(), Error> {
    if version.trim().is_empty() || version.chars().any(char::is_control) {
        return Err(Error::InvalidVersion);
    }
    Ok(())
}

fn validate_path(path: &str) -> Result<(), Error> {
    let relative = path.strip_prefix('/').unwrap_or(path);
    let valid = path.starts_with('/')
        && path.len() > 1
        && !path.ends_with('/')
        && !path.contains("//")
        && !relative
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
        && !path
            .chars()
            .any(|character| character.is_control() || character == '\\');
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidPath(path.to_owned()))
    }
}

fn hash_file(path: &Path) -> Result<(u64, [u8; 32]), Error> {
    let metadata = fs::metadata(path).map_err(|error| io_error("inspect", path, error))?;
    if !metadata.is_file() {
        return Err(Error::NotAFile {
            path: path.to_owned(),
        });
    }
    let mut input = File::open(path).map_err(|error| io_error("read", path, error))?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_SIZE];
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|error| io_error("read", path, error))?;
        if read == 0 {
            break;
        }
        size += read as u64;
        hasher.update(&buffer[..read]);
    }
    Ok((size, hasher.finalize().into()))
}

fn write_store_file(output: &Path, file: &ReleaseFile) -> Result<(), Error> {
    let name = file.store_name();
    let destination = output.join(&name);
    match fs::metadata(&destination) {
        Ok(metadata) => {
            if metadata.is_file() {
                let (size, sha256) = hash_file(&destination)?;
                if size == file.size && sha256 == file.sha256 {
                    return Ok(());
                }
            }
            return Err(Error::StoreConflict {
                name,
                path: file.path.clone(),
            });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error("inspect", &destination, error)),
    }

    let temporary = temporary_path(output, &name);
    let result = copy_checked(&file.source, &temporary, file);
    match result {
        Ok(()) => match fs::rename(&temporary, &destination) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                Err(io_error("rename into", &destination, error))
            }
        },
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

fn copy_checked(source: &Path, destination: &Path, file: &ReleaseFile) -> Result<(), Error> {
    let mut input = File::open(source).map_err(|error| io_error("read", source, error))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| io_error("create", destination, error))?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_SIZE];
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|error| io_error("read", source, error))?;
        if read == 0 {
            break;
        }
        size += read as u64;
        if size > file.size {
            return Err(Error::SourceChanged {
                path: source.to_owned(),
            });
        }
        hasher.update(&buffer[..read]);
        output
            .write_all(&buffer[..read])
            .map_err(|error| io_error("write", destination, error))?;
    }
    let digest: [u8; 32] = hasher.finalize().into();
    if size != file.size || digest != file.sha256 {
        return Err(Error::SourceChanged {
            path: source.to_owned(),
        });
    }
    output
        .sync_all()
        .map_err(|error| io_error("sync", destination, error))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| io_error("create", path, error))?;
    file.write_all(bytes)
        .map_err(|error| io_error("write", path, error))?;
    file.sync_all()
        .map_err(|error| io_error("sync", path, error))
}

fn temporary_path(directory: &Path, name: &str) -> PathBuf {
    directory.join(format!(".{name}.{}.tmp", std::process::id()))
}

fn io_error(action: &'static str, path: &Path, source: io::Error) -> Error {
    Error::Io {
        action,
        path: path.to_owned(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
        Sha256::digest(bytes).into()
    }

    fn release(files: Vec<ReleaseFile>) -> Release {
        Release {
            firmware: "example-cam".into(),
            version: "1.2.3".into(),
            date: None,
            commit: None,
            notes: None,
            files,
        }
    }

    fn entry(path: &str, size: u64, sha256: [u8; 32]) -> ReleaseFile {
        ReleaseFile {
            path: path.into(),
            source: PathBuf::from(path.trim_start_matches('/')),
            size,
            sha256,
        }
    }

    fn schema_node<'a>(root: &'a Value, reference: &str) -> &'a Value {
        let mut node = root;
        for part in reference.trim_start_matches("#/").split('/') {
            node = &node[part];
        }
        node
    }

    fn validate(value: &Value, schema: &Value, root: &Value) -> Result<(), String> {
        if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            return validate(value, schema_node(root, reference), root);
        }
        match schema.get("type").and_then(Value::as_str) {
            Some("object") => {
                let object = value.as_object().ok_or("expected object")?;
                for required in schema
                    .get("required")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let name = required.as_str().ok_or("non-string required")?;
                    if !object.contains_key(name) {
                        return Err(format!("missing {name}"));
                    }
                }
                if let Some(limit) = schema.get("maxProperties").and_then(Value::as_u64)
                    && object.len() > limit as usize
                {
                    return Err("too many properties".into());
                }
                let properties = schema.get("properties").and_then(Value::as_object);
                if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                    for name in object.keys() {
                        if !properties.is_some_and(|items| items.contains_key(name)) {
                            return Err(format!("unknown property {name}"));
                        }
                    }
                }
                if let Some(properties) = properties {
                    for (name, child) in object {
                        if let Some(child_schema) = properties.get(name) {
                            validate(child, child_schema, root)
                                .map_err(|error| format!("{name}: {error}"))?;
                        }
                    }
                }
                if let Some(additional) = schema.get("additionalProperties")
                    && additional.is_object()
                {
                    for (name, child) in object {
                        if !properties.is_some_and(|items| items.contains_key(name)) {
                            validate(child, additional, root)
                                .map_err(|error| format!("{name}: {error}"))?;
                        }
                    }
                }
            }
            Some("string") => {
                let text = value.as_str().ok_or("expected string")?;
                if let Some(minimum) = schema.get("minLength").and_then(Value::as_u64)
                    && text.len() < minimum as usize
                {
                    return Err(format!("shorter than {minimum}"));
                }
                if let Some(maximum) = schema.get("maxLength").and_then(Value::as_u64)
                    && text.len() > maximum as usize
                {
                    return Err(format!("longer than {maximum}"));
                }
                if schema.get("pattern").and_then(Value::as_str)
                    == Some("^[0-9a-fA-F]{64}$")
                    && (text.len() != 64
                        || !text.bytes().all(|byte| byte.is_ascii_hexdigit()))
                {
                    return Err("not a full SHA-256".into());
                }
            }
            Some("integer") => {
                let number = value
                    .as_u64()
                    .or_else(|| value.as_i64().and_then(|v| u64::try_from(v).ok()))
                    .ok_or("expected non-negative integer")?;
                if let Some(minimum) = schema.get("minimum").and_then(Value::as_u64)
                    && number < minimum
                {
                    return Err(format!("less than {minimum}"));
                }
                if let Some(maximum) = schema.get("maximum").and_then(Value::as_u64)
                    && number > maximum
                {
                    return Err(format!("greater than {maximum}"));
                }
            }
            Some("boolean") => {
                value.as_bool().ok_or("expected boolean")?;
            }
            Some("array") => {
                let array = value.as_array().ok_or("expected array")?;
                if let Some(maximum) = schema.get("maxItems").and_then(Value::as_u64)
                    && array.len() > maximum as usize
                {
                    return Err(format!("longer than {maximum}"));
                }
                if let Some(items) = schema.get("items") {
                    for item in array {
                        validate(item, items, root)?;
                    }
                }
            }
            Some(other) => return Err(format!("unsupported schema type {other}")),
            None => {}
        }
        Ok(())
    }

    #[test]
    fn manifest_matches_the_published_schema() {
        let schema: Value = serde_json::from_str(include_str!(
            "../../../catalog/schemas/ota-release-manifest.schema.json"
        ))
        .unwrap();
        let manifest: Value = serde_json::from_str(
            &release(vec![
                entry("/firmware.patch", 3, sha256_bytes(b"abc")),
                entry("/etc/ssl/cert.pem", 0, sha256_bytes(b"")),
            ])
            .manifest_json(),
        )
        .unwrap();
        validate(&manifest, &schema, &schema).unwrap();
    }

    #[test]
    fn system_manifest_schema_accepts_a_camera_record() {
        let schema: Value = serde_json::from_str(include_str!(
            "../../../catalog/schemas/ota-system-manifest.schema.json"
        ))
        .unwrap();
        let manifest: Value = serde_json::from_str(
            r#"{
                "gen": 7,
                "version": "1.2.3",
                "fs": {"/firmware.patch": "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"}
            }"#,
        )
        .unwrap();
        validate(&manifest, &schema, &schema).unwrap();
    }

    #[test]
    fn store_names_match_the_camera_store() {
        let empty = sha256_bytes(b"");
        assert_eq!(
            digest_hex(&empty),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(store_name(&empty), "E3B0C442.98F");
        assert_eq!(store_name(&sha256_bytes(b"abc")), "BA7816BF.8F0");
        assert_eq!(
            store_name_hex("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
                .unwrap(),
            "E3B0C442.98F"
        );
        assert_eq!(
            store_name_hex("E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855")
                .unwrap(),
            "E3B0C442.98F"
        );
        assert!(store_name_hex("E3B0C442.98F").is_err());
        assert!(store_name_hex(&"g".repeat(64)).is_err());
        assert!(Release::new("example-cam", "1", Vec::<(String, PathBuf)>::new()).is_err());
    }

    #[test]
    fn manifest_lists_firmware_version_size_and_hash() {
        let manifest = release(vec![
            entry("/firmware.patch", 3, sha256_bytes(b"abc")),
            entry("/etc/ssl/cert.pem", 0, sha256_bytes(b"")),
        ])
        .manifest_json();
        let lines: Vec<&str> = manifest.lines().collect();
        assert_eq!(
            lines,
            [
                "{",
                "  \"firmware\": \"example-cam\",",
                "  \"version\": \"1.2.3\",",
                "  \"fs\": {",
                "    \"/etc/ssl/cert.pem\": {",
                "      \"sha256\": \"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\",",
                "      \"size\": 0",
                "    },",
                "    \"/firmware.patch\": {",
                "      \"sha256\": \"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\",",
                "      \"size\": 3",
                "    }",
                "  }",
                "}",
            ]
        );
        assert!(manifest.ends_with('\n'));
    }

    #[test]
    fn colliding_store_names_are_refused() {
        let first = [0x12; 32];
        let mut second = first;
        second[31] = 0x13;
        assert_eq!(store_name(&first), store_name(&second));
        let result =
            release(vec![entry("/one", 1, first), entry("/two", 1, second)]).check_store_names();
        match result {
            Err(Error::StoreCollision {
                name,
                first,
                second,
            }) => {
                assert_eq!(name, store_name(&[0x12; 32]));
                assert_eq!(first, "/one");
                assert_eq!(second, "/two");
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn duplicate_store_content_is_shared() {
        let digest = sha256_bytes(b"same");
        assert!(
            release(vec![entry("/one", 4, digest), entry("/two", 4, digest)])
                .check_store_names()
                .is_ok()
        );
    }

    #[test]
    fn paths_firmware_and_version_are_validated() {
        for path in [
            "etc/ssl/cert.pem",
            "/",
            "/etc/",
            "/a//b",
            "/a/./b",
            "/a/../b",
            "/a\\b",
        ] {
            assert!(validate_path(path).is_err(), "{path}");
        }
        for path in ["/etc/ssl/cert.pem", "/firmware.patch", "/a/b-c_d.txt"] {
            assert!(validate_path(path).is_ok(), "{path}");
        }
        for firmware in ["", ".", "..", "-camera", "camera/110", "camera 110"] {
            assert!(validate_firmware(firmware).is_err(), "{firmware}");
        }
        for firmware in ["camera-110", "example-cam", "example_cam.v2"] {
            assert!(validate_firmware(firmware).is_ok(), "{firmware}");
        }
        for version in ["", "  ", "1.0\u{0}"] {
            assert!(validate_version(version).is_err(), "{version}");
        }
        for version in ["1.2.3", "0.4.0-beta.1"] {
            assert!(validate_version(version).is_ok(), "{version}");
        }
    }
}
