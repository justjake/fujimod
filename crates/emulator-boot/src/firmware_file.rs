//! Firmware update and boot ROM inputs as callers supply them.
//!
//! A file may be stored plain or as one zstd frame. The frame's magic number
//! decides, so a compressed file needs no particular name. Decoding happens in
//! memory; no host writes a decoded copy to disk.
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

/// Zstandard frame magic: `28 b5 2f fd`.
pub const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

/// Largest firmware file a caller may name.
pub const MAX_BYTES: u64 = 512 << 20;

/// An update or boot ROM as a path, or as bytes a host already holds.
#[derive(Clone, Debug)]
pub enum FirmwareFile {
    Path(PathBuf),
    Bytes(Vec<u8>),
}

impl Default for FirmwareFile {
    fn default() -> Self {
        Self::Path(PathBuf::new())
    }
}

impl From<PathBuf> for FirmwareFile {
    fn from(path: PathBuf) -> Self {
        Self::Path(path)
    }
}

impl From<&Path> for FirmwareFile {
    fn from(path: &Path) -> Self {
        Self::Path(path.to_owned())
    }
}

impl FirmwareFile {
    /// Read and, when it starts with the zstd magic, decode the file.
    pub fn read(self, limit: u64) -> Result<Vec<u8>, String> {
        match self {
            Self::Path(path) => read(&path, limit),
            Self::Bytes(bytes) => decode(bytes),
        }
    }
}

/// Read a bounded regular file without decoding it.
pub fn read_encoded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > limit {
        return Err(format!(
            "{} is {} bytes, over the {limit}-byte firmware input bound",
            path.display(),
            metadata.len()
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() as u64 != metadata.len() {
        return Err(format!("{} changed while it was read", path.display()));
    }
    Ok(bytes)
}

/// Read a file and decode it when its first bytes are the zstd magic.
pub fn read(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let bytes = read_encoded(path, limit)?;
    decode(bytes).map_err(|error| format!("{}: {error}", path.display()))
}

/// Decode a zstd frame in memory; bytes without the magic pass through.
pub fn decode(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if !bytes.starts_with(&ZSTD_MAGIC) {
        return Ok(bytes);
    }
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(bytes.as_slice())
        .map_err(|error| format!("zstd frame: {error}"))?;
    let mut decoded = Vec::new();
    decoder
        .read_to_end(&mut decoded)
        .map_err(|error| format!("zstd frame: {error}"))?;
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zstd_is_found_by_magic_and_plain_bytes_pass_through() {
        let plain = b"an update file".to_vec();
        assert_eq!(decode(plain.clone()).unwrap(), plain);
        let compressed = zstd::stream::encode_all(plain.as_slice(), 1).unwrap();
        assert!(compressed.starts_with(&ZSTD_MAGIC));
        assert_eq!(decode(compressed).unwrap(), plain);
    }
}
