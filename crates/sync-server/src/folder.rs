//! Folder destination for the photo sync receiver.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use crc32fast::Hasher;

use crate::{
    AssetReference, CameraId, Decision, DestinationError, MediaEntry, MediaSyncDestination, Upload,
};

/// Writes accepted files below one root directory and leaves .part files
/// beside them while a transfer is incomplete.
pub struct FolderDestination {
    root: PathBuf,
    extensions: Vec<String>,
}

impl FolderDestination {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            extensions: vec![
                "jpg".to_string(),
                "jpeg".to_string(),
                "raf".to_string(),
                "mov".to_string(),
            ],
        }
    }

    pub fn with_extensions(
        root: impl Into<PathBuf>,
        extensions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            root: root.into(),
            extensions: extensions
                .into_iter()
                .map(Into::into)
                .map(|extension| extension.to_ascii_lowercase())
                .collect(),
        }
    }

    fn relative(&self, entry: &MediaEntry) -> Result<PathBuf, DestinationError> {
        let path = Path::new(&entry.path);
        if path.is_absolute() {
            return Err(DestinationError::Invalid(format!(
                "absolute camera path {}",
                entry.path
            )));
        }
        let mut relative = PathBuf::new();
        for component in path.components() {
            match component {
                Component::Normal(part) => relative.push(part),
                Component::CurDir => {}
                _ => {
                    return Err(DestinationError::Invalid(format!(
                        "camera path {} leaves the destination",
                        entry.path
                    )));
                }
            }
        }
        if relative.as_os_str().is_empty() {
            return Err(DestinationError::Invalid("empty camera path".to_string()));
        }
        Ok(relative)
    }

    fn part_path(path: &Path) -> Result<PathBuf, DestinationError> {
        let name = path
            .file_name()
            .ok_or_else(|| DestinationError::Invalid("path has no file name".to_string()))?;
        let mut part = name.to_os_string();
        part.push(".part");
        Ok(path.with_file_name(part))
    }

    fn accepted(&self, entry: &MediaEntry) -> bool {
        Path::new(&entry.path)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| {
                self.extensions
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(extension))
            })
            .unwrap_or(false)
    }
}

impl MediaSyncDestination for FolderDestination {
    fn classify(
        &mut self,
        _camera: &CameraId,
        entries: &[MediaEntry],
    ) -> Result<Vec<Decision>, DestinationError> {
        entries
            .iter()
            .map(|entry| {
                if !self.accepted(entry) {
                    return Ok(Decision::Reject);
                }
                let relative = self.relative(entry)?;
                let final_path = self.root.join(&relative);
                if let Ok(metadata) = fs::metadata(&final_path) {
                    if metadata.len() == entry.size_bytes {
                        return Ok(Decision::Reject);
                    }
                }
                let part = Self::part_path(&final_path)?;
                let resume_offset = match fs::metadata(&part) {
                    Ok(metadata) if metadata.len() <= entry.size_bytes => metadata.len(),
                    Ok(_) => {
                        fs::remove_file(&part)?;
                        0
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
                    Err(error) => return Err(error.into()),
                };
                Ok(Decision::Accept { resume_offset })
            })
            .collect()
    }

    fn begin(
        &mut self,
        _camera: &CameraId,
        entry: &MediaEntry,
        resume_offset: u64,
    ) -> Result<Box<dyn Upload>, DestinationError> {
        let relative = self.relative(entry)?;
        let final_path = self.root.join(&relative);
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let part_path = Self::part_path(&final_path)?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&part_path)?;
        let mut hasher = Hasher::new();
        if resume_offset != 0 {
            let length = file.metadata()?.len();
            if length < resume_offset {
                return Err(DestinationError::Invalid(format!(
                    "partial file for {} is shorter than its resume offset",
                    entry.path
                )));
            }
            file.seek(SeekFrom::Start(0))?;
            let mut remaining = resume_offset;
            let mut buffer = [0u8; 64 * 1024];
            while remaining != 0 {
                let wanted = remaining.min(buffer.len() as u64) as usize;
                file.read_exact(&mut buffer[..wanted])?;
                hasher.update(&buffer[..wanted]);
                remaining -= wanted as u64;
            }
            file.set_len(resume_offset)?;
        } else {
            file.set_len(0)?;
        }
        file.seek(SeekFrom::Start(resume_offset))?;
        Ok(Box::new(FolderUpload {
            file,
            part_path,
            final_path,
            asset_reference: entry.path.clone(),
            expected_size: entry.size_bytes,
            hasher,
            next_offset: resume_offset,
        }))
    }
}

struct FolderUpload {
    file: File,
    part_path: PathBuf,
    final_path: PathBuf,
    asset_reference: String,
    expected_size: u64,
    hasher: Hasher,
    next_offset: u64,
}

impl Upload for FolderUpload {
    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), DestinationError> {
        if offset != self.next_offset {
            return Err(DestinationError::Invalid(format!(
                "write at {offset} does not follow {}",
                self.next_offset
            )));
        }
        self.file.write_all(bytes)?;
        self.hasher.update(bytes);
        self.next_offset += bytes.len() as u64;
        Ok(())
    }

    fn commit(
        mut self: Box<Self>,
        expected_crc32: u32,
    ) -> Result<AssetReference, DestinationError> {
        if self.next_offset != self.expected_size {
            return Err(DestinationError::Invalid(format!(
                "stored {} bytes, expected {}",
                self.next_offset, self.expected_size
            )));
        }
        self.file.flush()?;
        self.file.sync_all()?;
        let actual = self.hasher.clone().finalize();
        if actual != expected_crc32 {
            let part = self.part_path.clone();
            drop(self);
            let _ = fs::remove_file(part);
            return Err(DestinationError::Crc {
                expected: expected_crc32,
                actual,
            });
        }
        let FolderUpload {
            file,
            part_path,
            final_path,
            asset_reference,
            ..
        } = *self;
        drop(file);
        fs::rename(part_path, final_path)?;
        Ok(AssetReference(asset_reference))
    }

    fn abort(self: Box<Self>) -> Result<(), DestinationError> {
        let FolderUpload {
            file, part_path, ..
        } = *self;
        drop(file);
        match fs::remove_file(part_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}
