//! FSKit adapter through the version-pinned FSKitBridge protocol. Storage,
//! addressing, namespace, bounds and USB ownership remain in Rust.
use crate::{CameraFs, Device, IO_BYTES, MEMORY, MEMORY_IO_BYTES, Node, ROOT};
use async_trait::async_trait;
use fskit_rs::*;
use std::{
    ffi::OsStr,
    sync::{Arc, Mutex},
};

/// FSKitBridge's raw resource must have a nonzero extent before attachment.
/// The bridge crate otherwise creates an empty file on a fresh installation.
pub fn prepare_bridge_resource() -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open("/tmp/fskitbridge.dmg")
    {
        Ok(file) => file.set_len(1 << 20),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

pub struct Adapter<D> {
    fs: Arc<Mutex<CameraFs<D>>>,
    uid: u32,
    gid: u32,
}

impl<D> Clone for Adapter<D> {
    fn clone(&self) -> Self {
        Self {
            fs: self.fs.clone(),
            uid: self.uid,
            gid: self.gid,
        }
    }
}

impl<D: Device + 'static> Adapter<D> {
    pub async fn shutdown(&self) -> Result<()> {
        self.work(|fs| fs.shutdown()).await
    }
    pub fn new(fs: CameraFs<D>, uid: u32, gid: u32) -> Self {
        Self {
            fs: Arc::new(Mutex::new(fs)),
            uid,
            gid,
        }
    }

    async fn work<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut CameraFs<D>) -> crate::Result<T> + Send + 'static,
    ) -> Result<T> {
        let state = self.fs.clone();
        tokio::task::spawn_blocking(move || f(&mut *state.lock().map_err(|_| libc::EIO)?))
            .await
            .map_err(|_| Error::Posix(libc::EIO))?
            .map_err(Error::Posix)
    }

    fn attributes(&self, node: Node) -> ItemAttributes {
        ItemAttributes {
            uid: Some(self.uid),
            gid: Some(self.gid),
            mode: Some(if node.directory {
                if node.writable { 0o755 } else { 0o555 }
            } else if node.writable {
                0o644
            } else {
                0o444
            }),
            r#type: Some(if node.directory {
                ItemType::Directory as i32
            } else {
                ItemType::File as i32
            }),
            link_count: Some(1),
            size: Some(node.size),
            alloc_size: Some(if node.id == MEMORY { 0 } else { node.size }),
            file_id: Some(node.id),
            parent_id: Some(if node.id == ROOT { 1 } else { node.parent }),
            supports_limited_xattrs: Some(false),
            inhibit_kernel_offloaded_io: Some(true),
            ..Default::default()
        }
    }
    fn item(&self, node: Node) -> Item {
        Item {
            name: node.name.as_bytes().to_vec(),
            attributes: Some(self.attributes(node)),
        }
    }
}

fn name(value: &OsStr) -> Result<String> {
    value
        .to_str()
        .map(str::to_owned)
        .ok_or(Error::Posix(libc::EILSEQ))
}

#[async_trait]
impl<D: Device + 'static> Filesystem for Adapter<D> {
    async fn get_resource_identifier(&mut self) -> Result<ResourceIdentifier> {
        Ok(ResourceIdentifier {
            name: Some("Fuji".into()),
            container_id: None,
        })
    }
    async fn get_volume_identifier(&mut self) -> Result<VolumeIdentifier> {
        Ok(VolumeIdentifier {
            name: Some("Fuji".into()),
            id: None,
        })
    }
    async fn get_volume_behavior(&mut self) -> Result<VolumeBehavior> {
        Ok(VolumeBehavior {
            enable_open_unlink_emulation: Some(false),
            xattr_operations_inhibited: Some(true),
            is_open_close_inhibited: Some(false),
            is_access_check_inhibited: Some(false),
            is_volume_rename_inhibited: Some(true),
            is_preallocate_inhibited: Some(true),
            item_deactivation_options: Vec::new(),
        })
    }
    async fn get_path_conf_operations(&mut self) -> Result<PathConfOperations> {
        Ok(PathConfOperations {
            maximum_link_count: 1,
            maximum_name_length: 12,
            maximum_file_size: Some(1u64 << 32),
            maximum_xattr_size: Some(0),
            restricts_ownership_changes: true,
            truncates_long_names: false,
            ..Default::default()
        })
    }
    async fn get_volume_capabilities(&mut self) -> Result<SupportedCapabilities> {
        Ok(SupportedCapabilities {
            supports_persistent_object_ids: Some(false),
            case_format: Some(CaseFormat::InsensitiveCasePreserving as i32),
            supports_symbolic_links: Some(false),
            supports_hard_links: Some(false),
            supports_journal: Some(false),
            supports_active_journal: Some(false),
            supports_sparse_files: Some(false),
            supports_fast_statfs: Some(true),
            supports_64bit_object_ids: Some(true),
            does_not_support_volume_sizes: Some(true),
            does_not_support_setting_file_permissions: Some(true),
            ..Default::default()
        })
    }
    async fn get_volume_statistics(&mut self) -> Result<StatFsResult> {
        Ok(StatFsResult {
            block_size: 512,
            io_size: IO_BYTES as i64,
            ..Default::default()
        })
    }
    async fn mount(&mut self, _: TaskOptions) -> Result<()> {
        Ok(())
    }
    async fn unmount(&mut self) -> Result<()> {
        self.work(|fs| fs.synchronize()).await
    }
    async fn synchronize(&mut self, _: SyncFlags) -> Result<()> {
        self.work(|fs| fs.synchronize()).await
    }
    async fn get_attributes(&mut self, id: u64) -> Result<ItemAttributes> {
        Ok(self.attributes(self.work(move |fs| fs.metadata(id)).await?))
    }
    async fn set_attributes(
        &mut self,
        id: u64,
        attributes: ItemAttributes,
    ) -> Result<ItemAttributes> {
        let node = self
            .work(move |fs| {
                let node = fs.node(id)?;
                if !node.writable {
                    return Err(libc::EROFS);
                }
                if attributes
                    != (ItemAttributes {
                        size: attributes.size,
                        ..Default::default()
                    })
                {
                    return Err(libc::ENOTSUP);
                }
                if let Some(size) = attributes.size {
                    fs.truncate(id, size)
                } else {
                    Ok(node)
                }
            })
            .await?;
        Ok(self.attributes(node))
    }
    async fn lookup_item(&mut self, value: &OsStr, parent: u64) -> Result<Item> {
        let value = name(value)?;
        Ok(self.item(self.work(move |fs| fs.lookup(parent, &value)).await?))
    }
    async fn reclaim_item(&mut self, _: u64) -> Result<()> {
        Ok(())
    }
    async fn read_symbolic_link(&mut self, _: u64) -> Result<Vec<u8>> {
        Err(Error::Posix(libc::ENOTSUP))
    }
    async fn create_item(
        &mut self,
        value: &OsStr,
        kind: ItemType,
        parent: u64,
        _: ItemAttributes,
    ) -> Result<Item> {
        let value = name(value)?;
        if !matches!(kind, ItemType::File | ItemType::Directory) {
            return Err(Error::Posix(libc::ENOTSUP));
        }
        Ok(self.item(
            self.work(move |fs| fs.create(parent, &value, kind == ItemType::Directory))
                .await?,
        ))
    }
    async fn create_symbolic_link(
        &mut self,
        _: &OsStr,
        _: u64,
        _: ItemAttributes,
        _: Vec<u8>,
    ) -> Result<Item> {
        Err(Error::Posix(libc::ENOTSUP))
    }
    async fn create_link(&mut self, _: u64, _: &OsStr, _: u64) -> Result<Vec<u8>> {
        Err(Error::Posix(libc::ENOTSUP))
    }
    async fn remove_item(&mut self, _: u64, value: &OsStr, parent: u64) -> Result<()> {
        let value = name(value)?;
        self.work(move |fs| fs.remove(parent, &value)).await
    }
    async fn rename_item(
        &mut self,
        id: u64,
        _: u64,
        _: &OsStr,
        _: &OsStr,
        _: u64,
        _: Option<u64>,
    ) -> Result<Vec<u8>> {
        let node = self.work(move |fs| fs.node(id)).await?;
        Err(Error::Posix(if node.writable {
            libc::ENOTSUP
        } else {
            libc::EROFS
        }))
    }
    async fn enumerate_directory(
        &mut self,
        id: u64,
        cookie: u64,
        verifier: u64,
    ) -> Result<DirectoryEntries> {
        let (generation, nodes) = self.work(move |fs| fs.directory(id, cookie != 0)).await?;
        if cookie != 0 && verifier != generation {
            return Err(Error::Posix(libc::ESTALE));
        }
        let start = usize::try_from(cookie).map_err(|_| Error::Posix(libc::EINVAL))?;
        if start > nodes.len() {
            return Err(Error::Posix(libc::EINVAL));
        }
        let entries = nodes
            .into_iter()
            .enumerate()
            .skip(start)
            .take(256)
            .map(|(i, node)| directory_entries::Entry {
                item: Some(self.item(node)),
                next_cookie: i as u64 + 1,
            })
            .collect();
        Ok(DirectoryEntries {
            entries,
            verifier: generation,
        })
    }
    async fn activate(&mut self, _: TaskOptions) -> Result<Item> {
        Ok(self.item(self.work(|fs| fs.node(ROOT)).await?))
    }
    async fn deactivate(&mut self) -> Result<()> {
        self.work(|fs| fs.synchronize()).await
    }
    async fn get_supported_xattr_names(&mut self, _: u64) -> Result<Xattrs> {
        Ok(Xattrs::default())
    }
    async fn get_xattr(&mut self, _: &OsStr, _: u64) -> Result<Vec<u8>> {
        Err(Error::Posix(libc::ENOATTR))
    }
    async fn set_xattr(
        &mut self,
        _: &OsStr,
        _: Option<Vec<u8>>,
        _: u64,
        _: SetXattrPolicy,
    ) -> Result<()> {
        Err(Error::Posix(libc::ENOTSUP))
    }
    async fn get_xattrs(&mut self, _: u64) -> Result<Xattrs> {
        Ok(Xattrs::default())
    }
    async fn open_item(&mut self, id: u64, modes: Vec<OpenMode>) -> Result<()> {
        self.work(move |fs| {
            let node = fs.node(id)?;
            if modes.contains(&OpenMode::Write) && !node.writable {
                return Err(libc::EROFS);
            }
            Ok(())
        })
        .await
    }
    async fn close_item(&mut self, _: u64, _: Vec<OpenMode>) -> Result<()> {
        self.work(|fs| fs.synchronize()).await
    }
    async fn read(&mut self, id: u64, offset: i64, length: i64) -> Result<Vec<u8>> {
        let offset = u64::try_from(offset).map_err(|_| Error::Posix(libc::EINVAL))?;
        let length = usize::try_from(length).map_err(|_| Error::Posix(libc::EINVAL))?;
        if length > MEMORY_IO_BYTES {
            return Err(Error::Posix(libc::EINVAL));
        }
        self.work(move |fs| {
            let mut data = Vec::with_capacity(length);
            while data.len() < length {
                let limit = if id == MEMORY {
                    MEMORY_IO_BYTES
                } else {
                    IO_BYTES
                };
                let n = limit.min(length - data.len());
                let part = fs.read(
                    id,
                    offset.checked_add(data.len() as u64).ok_or(libc::EINVAL)?,
                    n,
                )?;
                let eof = part.len() < n;
                data.extend(part);
                if eof {
                    break;
                }
            }
            Ok(data)
        })
        .await
    }
    async fn write(&mut self, contents: Vec<u8>, id: u64, offset: i64) -> Result<i64> {
        let offset = u64::try_from(offset).map_err(|_| Error::Posix(libc::EINVAL))?;
        self.work(move |fs| {
            let mut count = 0;
            for part in contents.chunks(IO_BYTES) {
                match fs.write(id, offset.checked_add(count).ok_or(libc::EFBIG)?, part) {
                    Ok(n) => count += n as u64,
                    Err(_) if count > 0 => break,
                    Err(error) => return Err(error),
                }
            }
            Ok(count as i64)
        })
        .await
    }
    async fn check_access(&mut self, id: u64, access: Vec<AccessMask>) -> Result<bool> {
        let node = self.work(move |fs| fs.node(id)).await?;
        Ok(access.iter().all(|a| match a {
            AccessMask::WriteData
            | AccessMask::AddFile
            | AccessMask::Delete
            | AccessMask::AppendData
            | AccessMask::AddSubdirectory
            | AccessMask::DeleteChild
            | AccessMask::WriteAttributes
            | AccessMask::WriteXattr
            | AccessMask::WriteSecurity
            | AccessMask::TakeOwnership => node.writable,
            AccessMask::Execute => node.directory,
            _ => true,
        }))
    }
    async fn set_volume_name(&mut self, _: Vec<u8>) -> Result<Vec<u8>> {
        Err(Error::Posix(libc::EROFS))
    }
    async fn preallocate_space(
        &mut self,
        _: u64,
        _: i64,
        _: i64,
        _: Vec<PreallocateFlag>,
    ) -> Result<i64> {
        Err(Error::Posix(libc::ENOTSUP))
    }
    async fn deactivate_item(&mut self, _: u64) -> Result<()> {
        Ok(())
    }
}
