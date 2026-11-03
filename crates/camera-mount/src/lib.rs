//! Platform-independent namespace over bounded camera operations.
//! Memory is always read-only. Only the SD subtree delegates mutations.
#[cfg(target_os = "macos")]
pub mod bridge;
mod flash;
#[cfg(target_os = "macos")]
pub mod fskit;
mod service;
mod transport;
pub mod usb;

use fujimod_usb::operations::card::DirectoryEntry;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

pub const ROOT: u64 = 2; // FSKit reserves item 1 for its parent-of-root sentinel.
pub const SD: u64 = 3;
pub const MEMORY: u64 = 4;
pub const REGIONS: u64 = 5;
pub const FLASH: u64 = 6;
pub const IO_BYTES: usize = fujimod_usb::operations::card::MAX_REQUEST_BYTES as usize;
pub const MEMORY_IO_BYTES: usize = 64 << 20;
pub type Result<T> = std::result::Result<T, i32>;

/// No memory-write operation exists at this boundary.
pub trait Device: Send {
    fn shutdown(&mut self) -> Result<()> {
        self.synchronize()
    }
    fn has_card(&self) -> bool;
    fn list(&mut self, path: &str) -> Result<Vec<DirectoryEntry>>;
    fn read(&mut self, path: &str, offset: u32, size: u32) -> Result<Vec<u8>>;
    fn write(&mut self, path: &str, offset: u32, data: &[u8]) -> Result<()>;
    fn create(&mut self, path: &str) -> Result<()>;
    fn delete(&mut self, path: &str) -> Result<()>;
    fn shrink(&mut self, path: &str, size: u32) -> Result<()>;
    fn memory(&mut self, address: u32, size: u32) -> Result<Vec<u8>>;
    /// Logical internal-storage bytes, available with a checked reader.
    fn flash_size(&self) -> Option<u64>;
    fn flash(&mut self, offset: u64, size: u32) -> Result<Vec<u8>>;
    fn synchronize(&mut self) -> Result<()>;
}

#[derive(Clone, Debug, Serialize)]
pub struct Region {
    pub name: String,
    pub address: u64,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub id: u64,
    pub parent: u64,
    pub name: String,
    pub path: Option<String>,
    pub size: u64,
    pub directory: bool,
    pub writable: bool,
}

struct Listing {
    when: Instant,
    generation: u64,
    entries: Vec<u64>,
}

pub struct CameraFs<D> {
    device: D,
    nodes: BTreeMap<u64, Node>,
    paths: BTreeMap<String, u64>,
    listings: BTreeMap<u64, Listing>,
    regions: Vec<Region>,
    regions_json: Vec<u8>,
    next_inode: u64,
    generation: u64,
}

impl<D: Device> CameraFs<D> {
    pub fn shutdown(&mut self) -> Result<()> {
        self.device.shutdown()
    }
    pub fn new(device: D, mut regions: Vec<Region>) -> Result<Self> {
        regions.sort_by_key(|r| r.address);
        for region in &regions {
            if region.size == 0
                || region
                    .address
                    .checked_add(region.size)
                    .is_none_or(|end| end > 1u64 << 32)
            {
                return Err(libc::EINVAL);
            }
        }
        let mut regions_json = serde_json::to_vec_pretty(&regions).map_err(|_| libc::EINVAL)?;
        regions_json.push(b'\n');
        let mut nodes = BTreeMap::new();
        for (id, name, directory, writable, size, path) in [
            (ROOT, "", true, false, 0, None),
            (SD, "sd", true, true, 0, Some("A:\\".to_owned())),
            (MEMORY, "memory", false, false, 1u64 << 32, None),
            (
                REGIONS,
                "regions.json",
                false,
                false,
                regions_json.len() as u64,
                None,
            ),
        ] {
            nodes.insert(
                id,
                Node {
                    id,
                    parent: ROOT,
                    name: name.into(),
                    path,
                    size,
                    directory,
                    writable,
                },
            );
        }
        if !device.has_card() {
            nodes.remove(&SD);
        }
        if let Some(size) = device.flash_size() {
            if size == 0 {
                return Err(libc::EINVAL);
            }
            nodes.insert(
                FLASH,
                Node {
                    id: FLASH,
                    parent: ROOT,
                    name: "flash".into(),
                    path: None,
                    size,
                    directory: false,
                    writable: false,
                },
            );
        }
        Ok(Self {
            device,
            nodes,
            paths: BTreeMap::new(),
            listings: BTreeMap::new(),
            regions,
            regions_json,
            next_inode: 7,
            generation: 1,
        })
    }

    pub fn node(&self, id: u64) -> Result<Node> {
        self.nodes.get(&id).cloned().ok_or(libc::ESTALE)
    }

    pub fn metadata(&mut self, id: u64) -> Result<Node> {
        let node = self.node(id)?;
        if id > FLASH {
            let (_, entries) = self.directory(node.parent, false)?;
            return entries.into_iter().find(|n| n.id == id).ok_or(libc::ESTALE);
        }
        Ok(node)
    }

    fn name(name: &str) -> Result<()> {
        if name.is_empty()
            || name == "."
            || name == ".."
            || !name
                .bytes()
                .all(fujimod_usb::operations::card::is_short_name_byte)
        {
            return Err(libc::EINVAL);
        }
        let (stem, extension) = name.split_once('.').unwrap_or((name, ""));
        if stem.is_empty() || stem.len() > 8 || extension.len() > 3 || extension.contains('.') {
            return Err(libc::ENAMETOOLONG);
        }
        Ok(())
    }

    fn child_path(&self, parent: u64, name: &str) -> Result<String> {
        if parent == ROOT {
            return Err(libc::EROFS);
        }
        let node = self.node(parent)?;
        if !node.directory {
            return Err(libc::ENOTDIR);
        }
        Self::name(name)?;
        let folder = node.path.ok_or(libc::EROFS)?;
        let path = format!(
            "{}\\{}",
            folder.trim_end_matches('\\'),
            name.to_ascii_uppercase()
        );
        if path.len() >= 48 {
            return Err(libc::ENAMETOOLONG);
        }
        Ok(path)
    }

    pub fn directory(&mut self, id: u64, continuation: bool) -> Result<(u64, Vec<Node>)> {
        if id == ROOT {
            return Ok((
                1,
                [SD, MEMORY, FLASH, REGIONS]
                    .into_iter()
                    .filter(|id| self.nodes.contains_key(id))
                    .map(|i| self.node(i))
                    .collect::<Result<_>>()?,
            ));
        }
        let node = self.node(id)?;
        if !node.directory {
            return Err(libc::ENOTDIR);
        }
        let cached = self
            .listings
            .get(&id)
            .is_some_and(|l| continuation || l.when.elapsed() < Duration::from_millis(250));
        if !cached {
            let entries = self
                .device
                .list(node.path.as_deref().ok_or(libc::ENOTDIR)?)?;
            let mut ids = Vec::new();
            for entry in entries {
                if entry.name == "." || entry.name == ".." {
                    continue;
                }
                let path = self.child_path(id, &entry.name)?;
                let key = path.to_ascii_uppercase();
                let child = if let Some(id) = self.paths.get(&key) {
                    *id
                } else {
                    let id = self.next_inode;
                    self.next_inode += 1;
                    self.paths.insert(key, id);
                    id
                };
                self.nodes.insert(
                    child,
                    Node {
                        id: child,
                        parent: id,
                        name: entry.name,
                        path: Some(path),
                        size: u64::from(entry.bytes),
                        directory: entry.directory,
                        writable: !entry.read_only,
                    },
                );
                ids.push(child);
            }
            self.generation += 1;
            self.listings.insert(
                id,
                Listing {
                    when: Instant::now(),
                    generation: self.generation,
                    entries: ids,
                },
            );
        }
        let list = self.listings.get(&id).ok_or(libc::EIO)?;
        Ok((
            list.generation,
            list.entries
                .iter()
                .map(|i| self.node(*i))
                .collect::<Result<_>>()?,
        ))
    }

    pub fn lookup(&mut self, parent: u64, name: &str) -> Result<Node> {
        if name == "." {
            return self.node(parent);
        }
        if name == ".." {
            return self.node(self.node(parent)?.parent);
        }
        self.directory(parent, false)?
            .1
            .into_iter()
            .find(|n| n.name.eq_ignore_ascii_case(name))
            .ok_or(libc::ENOENT)
    }

    fn readable(&self, address: u64, size: u64) -> bool {
        let Some(end) = address.checked_add(size) else {
            return false;
        };
        let mut cursor = address;
        for region in &self.regions {
            if region.address > cursor {
                break;
            }
            cursor = cursor.max(region.address + region.size);
            if cursor >= end {
                return true;
            }
        }
        false
    }

    pub fn read(&mut self, id: u64, offset: u64, size: usize) -> Result<Vec<u8>> {
        let node = self.node(id)?;
        if node.directory {
            return Err(libc::EISDIR);
        }
        if offset >= node.size || size == 0 {
            return Ok(Vec::new());
        }
        let size = (size as u64).min(node.size - offset) as usize;
        if size
            > if id == MEMORY {
                MEMORY_IO_BYTES
            } else {
                IO_BYTES
            }
        {
            return Err(libc::EINVAL);
        }
        if id == REGIONS {
            return Ok(self.regions_json[offset as usize..offset as usize + size].to_vec());
        }
        if id == FLASH {
            let data = self.device.flash(offset, size as u32)?;
            return if data.len() == size {
                Ok(data)
            } else {
                Err(libc::EIO)
            };
        }
        let address = u32::try_from(offset).map_err(|_| libc::EINVAL)?;
        let data = if id == MEMORY {
            if !self.readable(offset, size as u64) {
                return Err(libc::EIO);
            }
            self.device.memory(address, size as u32)?
        } else {
            self.device.read(
                node.path.as_deref().ok_or(libc::EINVAL)?,
                address,
                size as u32,
            )?
        };
        if data.len() > size || id == MEMORY && data.len() != size {
            return Err(libc::EIO);
        }
        Ok(data)
    }

    pub fn write(&mut self, id: u64, offset: u64, data: &[u8]) -> Result<usize> {
        let node = self.node(id)?;
        if !node.writable {
            return Err(libc::EROFS);
        }
        if node.directory {
            return Err(libc::EISDIR);
        }
        if data.is_empty() {
            return Ok(0);
        }
        if data.len() > IO_BYTES {
            return Err(libc::EINVAL);
        }
        let end = offset
            .checked_add(data.len() as u64)
            .filter(|end| *end <= u64::from(u32::MAX))
            .ok_or(libc::EFBIG)?;
        let path = node.path.as_deref().ok_or(libc::EROFS)?;
        // Cameras don't promise zero-filled seek gaps. Fill only the gap with
        // bounded writes instead of downloading or rewriting existing contents.
        let mut cursor = node.size;
        let zero = vec![0; IO_BYTES.min(offset.saturating_sub(cursor) as usize)];
        while cursor < offset {
            let n = zero.len().min((offset - cursor) as usize);
            self.device.write(path, cursor as u32, &zero[..n])?;
            cursor += n as u64;
            self.nodes.get_mut(&id).ok_or(libc::ESTALE)?.size = cursor;
            self.listings.remove(&node.parent);
        }
        self.device.write(path, offset as u32, data)?;
        self.nodes.get_mut(&id).ok_or(libc::ESTALE)?.size = node.size.max(end);
        self.listings.remove(&node.parent);
        Ok(data.len())
    }

    pub fn create(&mut self, parent: u64, name: &str, directory: bool) -> Result<Node> {
        if !self.node(parent)?.writable {
            return Err(libc::EROFS);
        }
        let path = self.child_path(parent, name)?;
        match self.lookup(parent, name) {
            Ok(_) => return Err(libc::EEXIST),
            Err(libc::ENOENT) => (),
            Err(e) => return Err(e),
        }
        if directory {
            return Err(libc::ENOTSUP);
        }
        self.device.create(&path)?;
        self.listings.remove(&parent);
        let id = self.next_inode;
        self.next_inode += 1;
        let node = Node {
            id,
            parent,
            name: name.to_ascii_uppercase(),
            path: Some(path.clone()),
            size: 0,
            directory: false,
            writable: true,
        };
        self.paths.insert(path, id);
        self.nodes.insert(id, node.clone());
        Ok(node)
    }

    pub fn truncate(&mut self, id: u64, size: u64) -> Result<Node> {
        let node = self.node(id)?;
        if !node.writable {
            return Err(libc::EROFS);
        }
        if node.directory {
            return Err(libc::EISDIR);
        }
        if size == node.size {
            return Ok(node);
        }
        if size == 0 {
            self.device
                .create(node.path.as_deref().ok_or(libc::EROFS)?)?;
            self.nodes.get_mut(&id).ok_or(libc::ESTALE)?.size = 0;
        } else if size > node.size {
            self.write(id, size.checked_sub(1).ok_or(libc::EINVAL)?, &[0])?;
        } else {
            self.device.shrink(
                node.path.as_deref().ok_or(libc::EROFS)?,
                size.try_into().map_err(|_| libc::EFBIG)?,
            )?;
            self.nodes.get_mut(&id).ok_or(libc::ESTALE)?.size = size;
        }
        self.listings.remove(&node.parent);
        self.node(id)
    }

    pub fn remove(&mut self, parent: u64, name: &str) -> Result<()> {
        if !self.node(parent)?.writable {
            return Err(libc::EROFS);
        }
        self.child_path(parent, name)?;
        let node = self.lookup(parent, name)?;
        if !node.writable {
            return Err(libc::EROFS);
        }
        if node.directory {
            return Err(libc::ENOTSUP);
        }
        self.device
            .delete(node.path.as_deref().ok_or(libc::EROFS)?)?;
        self.paths.remove(node.path.as_deref().ok_or(libc::EROFS)?);
        self.nodes.remove(&node.id);
        self.listings.remove(&parent);
        Ok(())
    }

    pub fn synchronize(&mut self) -> Result<()> {
        self.device.synchronize()
    }
}

#[cfg(test)]
mod tests;
