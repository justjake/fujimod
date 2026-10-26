//! Explicit file providers for catalog documents and external assets.
//! The caller selects both roots and every input location.

use crate::{Asset, Content, Key, Provider, Reference};
use fujimod_catalog_identity::validate_id;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Location {
    File { path: PathBuf },
    Window { path: PathBuf, offset: u64 },
}
impl Location {
    fn path(&self) -> &Path {
        match self {
            Self::File { path } | Self::Window { path, .. } => path,
        }
    }
    fn validate(&self) -> Result<(), String> {
        let path = self.path();
        if path.as_os_str().is_empty()
            || path.to_string_lossy().contains('\\')
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err("input location must be a relative path without parent traversal".into());
        }
        Ok(())
    }
}

pub struct Files {
    document_root: PathBuf,
    asset_root: PathBuf,
    documents: BTreeMap<Key, Location>,
    assets: BTreeMap<String, Location>,
    max_asset_bytes: u64,
}
impl Files {
    pub fn new(
        document_root: &Path,
        asset_root: &Path,
        documents: BTreeMap<Key, Location>,
        assets: BTreeMap<String, Location>,
        max_asset_bytes: u64,
    ) -> Result<Self, String> {
        fn root(path: &Path) -> Result<PathBuf, String> {
            let root = path
                .canonicalize()
                .map_err(|error| format!("input root: {error}"))?;
            if !root.is_dir() {
                return Err("input root must be a directory".into());
            }
            Ok(root)
        }
        for (key, location) in &documents {
            validate_id(&key.id)?;
            location.validate()?;
        }
        for (id, location) in &assets {
            validate_id(id)?;
            location.validate()?;
        }
        Ok(Self {
            document_root: root(document_root)?,
            asset_root: root(asset_root)?,
            documents,
            assets,
            max_asset_bytes,
        })
    }
    fn read(
        root: &Path,
        location: &Location,
        content: &Content,
        limit: u64,
    ) -> Result<Vec<u8>, String> {
        content.validate()?;
        location.validate()?;
        if content.size > limit {
            return Err("input exceeds the caller's byte limit".into());
        }
        let path = root
            .join(location.path())
            .canonicalize()
            .map_err(|error| format!("input is missing or unavailable: {error}"))?;
        if !path.starts_with(root) {
            return Err("input symlink escapes its selected root".into());
        }
        if !path
            .metadata()
            .map_err(|error| format!("input metadata: {error}"))?
            .is_file()
        {
            return Err("input must be a regular file".into());
        }
        let mut file = File::open(&path).map_err(|error| format!("cannot open input: {error}"))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("input metadata: {error}"))?;
        if !metadata.is_file() {
            return Err("input must be a regular file".into());
        }
        let offset = match location {
            Location::File { .. } => {
                if metadata.len() != content.size {
                    return Err("input file size differs".into());
                }
                0
            }
            Location::Window { offset, .. } => *offset,
        };
        let end = offset
            .checked_add(content.size)
            .ok_or("input window extent overflows")?;
        if end > metadata.len() {
            return Err("input window exceeds its source file".into());
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| format!("input seek: {error}"))?;
        let mut bytes = Vec::new();
        file.take(content.size)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("input read: {error}"))?;
        if bytes.len() as u64 != content.size {
            return Err("input changed during its bounded read".into());
        }
        Ok(bytes)
    }
}
impl Provider for Files {
    fn document(&self, reference: &Reference) -> Result<Vec<u8>, String> {
        reference.validate()?;
        let location = self
            .documents
            .get(&reference.key)
            .ok_or("document has no selected input location")?;
        Self::read(&self.document_root, location, &reference.content, 4 << 20)
    }
    fn asset(&self, asset: &Asset) -> Result<Vec<u8>, String> {
        asset.validate()?;
        let location = self
            .assets
            .get(&asset.id)
            .ok_or("asset has no selected input location")?;
        Self::read(
            &self.asset_root,
            location,
            &asset.content,
            self.max_asset_bytes,
        )
    }
}

#[cfg(test)]
mod tests;
