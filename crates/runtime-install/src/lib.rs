//! Runtime installation mechanisms. Callers supply allocator, cache, permission,
//! callback and image facts; this crate contains no target-specific addresses.
mod image;
pub mod relocation;
pub use image::*;
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("body operation failed: {0}")]
    Body(String),
    #[error("{source}; allocation cleanup also failed: {cleanup}")]
    Cleanup {
        source: Box<Error>,
        cleanup: Box<Error>,
    },
    #[error(transparent)]
    Usb(#[from] fujimod_usb::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
pub type Pending<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Range {
    pub address: u64,
    pub size: u64,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Install,
    Init,
    Status,
    Rollback,
}
impl Operation {
    pub fn code(self, image: &RuntimeImage) -> u32 {
        let ops = &image.control.operations;
        match self {
            Self::Install => ops.install,
            Self::Init => ops.init,
            Self::Status => ops.status,
            Self::Rollback => ops.rollback,
        }
    }
}
/// Allocation and callback methods normally use operations::callback::invoke.
/// The body owns transport access and supplies all architecture-specific work.
pub trait Body: Send {
    fn allocate(&mut self, size: u64, align: u64) -> Pending<'_, u64>;
    fn free(&mut self, base: u64) -> Pending<'_, ()>;
    fn read(&mut self, range: Range) -> Pending<'_, Vec<u8>>;
    fn write(&mut self, address: u64, bytes: Vec<u8>) -> Pending<'_, ()>;
    fn synchronize(&mut self, range: Range) -> Pending<'_, ()>;
    fn protect(&mut self, range: Range) -> Pending<'_, ()>;
    fn control(&mut self, entry: u64, operation: u32) -> Pending<'_, ()>;
    /// Persist placement and ownership before upload. Default needs no storage.
    fn placed(&mut self, _image: &RuntimeImage, _lease: &Lease) -> Pending<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}
pub enum Placement<'a> {
    Relocate,
    Relink(&'a mut (dyn FnMut(u64) -> Result<RuntimeImage> + Send)),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Lease {
    pub version: u32,
    pub base: u64,
    pub size: u64,
    pub nonce: u32,
    pub publication_attempted: bool,
    pub installed: bool,
    pub freed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlResult {
    pub status: i32,
    pub result: u32,
    pub words_new: u32,
    pub words_old: u32,
    pub inits_done: Option<u32>,
}
#[derive(Clone, Debug)]
pub struct Installer {
    pub image: RuntimeImage,
    pub lease: Option<Lease>,
}
impl Installer {
    pub fn plan(image: RuntimeImage) -> Result<Self> {
        image.validate()?;
        Ok(Self { image, lease: None })
    }
    pub fn attach(image: RuntimeImage, lease: Lease) -> Result<Self> {
        image.validate()?;
        if lease.version != 1
            || lease.freed
            || lease.base != image.link_base
            || lease.size != image.capacity
            || lease.nonce != image.control.nonce
        {
            return Err(Error::Invalid("lease differs from image"));
        }
        Ok(Self {
            image,
            lease: Some(lease),
        })
    }
    fn range(&self) -> Range {
        Range {
            address: self.image.link_base,
            size: self.image.bytes.len() as u64,
        }
    }
    /// Split preparation supports caller pre-publication policy and persistence.
    pub async fn prepare(&mut self, body: &mut impl Body, placement: Placement<'_>) -> Result<()> {
        if self.lease.is_some() {
            return Err(Error::Invalid("already allocated"));
        }
        let base = body
            .allocate(self.image.capacity, self.image.alignment)
            .await?;
        self.lease = Some(Lease {
            version: 1,
            base,
            size: self.image.capacity,
            nonce: self.image.control.nonce,
            publication_attempted: false,
            installed: false,
            freed: false,
        });
        let result = async {
            let image = match placement {
                Placement::Relocate => self.image.relocate(base)?,
                Placement::Relink(callback) => callback(base)?,
            };
            image.validate()?;
            if image.link_base != base
                || image.capacity != self.image.capacity
                || image.control.nonce != self.image.control.nonce
            {
                return Err(Error::Invalid("relinked image differs from allocation"));
            }
            self.image = image;
            body.placed(&self.image, self.lease.as_ref().unwrap())
                .await?;
            self.upload(body).await?;
            body.synchronize(self.range()).await?;
            body.protect(self.range()).await
        }
        .await;
        if let Err(source) = result {
            match body.free(base).await {
                Ok(()) => {
                    self.lease.as_mut().unwrap().freed = true;
                    Err(source)
                }
                Err(cleanup) => Err(Error::Cleanup {
                    source: Box::new(source),
                    cleanup: Box::new(cleanup),
                }),
            }
        } else {
            Ok(())
        }
    }
    pub async fn install(
        &mut self,
        body: &mut impl Body,
        placement: Placement<'_>,
        token: u32,
    ) -> Result<ControlResult> {
        self.prepare(body, placement).await?;
        if let Err(source) = self.publish(body, token).await {
            if !self.lease.as_ref().unwrap().publication_attempted {
                match body.free(self.lease.as_ref().unwrap().base).await {
                    Ok(()) => self.lease.as_mut().unwrap().freed = true,
                    Err(cleanup) => {
                        return Err(Error::Cleanup {
                            source: Box::new(source),
                            cleanup: Box::new(cleanup),
                        });
                    }
                }
            }
            return Err(source);
        }
        self.init(body, token.wrapping_add(1).max(1)).await
    }
    pub async fn upload(&self, body: &mut impl Body) -> Result<()> {
        let lease = self.lease.as_ref().ok_or(Error::Invalid("not allocated"))?;
        if lease.freed || lease.publication_attempted {
            return Err(Error::Invalid("upload after publication or free"));
        }
        body.write(self.image.link_base, self.image.bytes.clone())
            .await?;
        let range = Range {
            address: self.image.link_base | self.image.control.read_alias,
            size: self.image.bytes.len() as u64,
        };
        if body.read(range).await? != self.image.bytes {
            return Err(Error::Invalid("image upload readback differs"));
        }
        Ok(())
    }
    pub async fn publish(&mut self, body: &mut impl Body, token: u32) -> Result<ControlResult> {
        self.control(body, Operation::Install, token).await
    }
    pub async fn init(&mut self, body: &mut impl Body, token: u32) -> Result<ControlResult> {
        self.control(body, Operation::Init, token).await
    }
    pub async fn status(&mut self, body: &mut impl Body, token: u32) -> Result<ControlResult> {
        self.control(body, Operation::Status, token).await
    }
    pub async fn rollback(&mut self, body: &mut impl Body, token: u32) -> Result<ControlResult> {
        self.control(body, Operation::Rollback, token).await
    }
    async fn word(body: &mut impl Body, address: u64) -> Result<u32> {
        let bytes = body.read(Range { address, size: 4 }).await?;
        Ok(u32::from_le_bytes(
            bytes
                .try_into()
                .map_err(|_| Error::Invalid("word read size"))?,
        ))
    }
    pub async fn control(
        &mut self,
        body: &mut impl Body,
        op: Operation,
        token: u32,
    ) -> Result<ControlResult> {
        let lease = self.lease.as_ref().ok_or(Error::Invalid("not allocated"))?;
        if lease.freed || token == 0 {
            return Err(Error::Invalid("freed lease or empty token"));
        }
        let c = self.image.control.clone();
        let state = self.image.link_base + u64::from(c.state_offset);
        let code = body
            .read(Range {
                address: self.image.link_base | c.read_alias,
                size: u64::from(c.code_size),
            })
            .await?;
        if code != self.image.bytes[..c.code_size as usize]
            || Self::word(body, (state + u64::from(c.magic_offset)) | c.read_alias).await?
                != c.magic
            || Self::word(body, (state + u64::from(c.nonce_offset)) | c.read_alias).await?
                != c.nonce
        {
            return Err(Error::Invalid("image ownership lost"));
        }
        self.set(
            body,
            state + u64::from(c.token_offset),
            token.to_le_bytes().to_vec(),
        )
        .await?;
        self.set(
            body,
            state + u64::from(c.mailbox.offset),
            vec![0xa5; c.mailbox.size as usize],
        )
        .await?;
        if let Some(offset) = c.operation_offset {
            self.set(
                body,
                state + u64::from(offset),
                op.code(&self.image).to_le_bytes().to_vec(),
            )
            .await?;
        }
        body.synchronize(self.range()).await?;
        // Even unknown completion can have exposed hooks. Never free afterward.
        if op == Operation::Install {
            self.lease.as_mut().unwrap().publication_attempted = true;
        }
        body.control(
            self.image.link_base + u64::from(c.control_offset),
            op.code(&self.image),
        )
        .await?;
        let mailbox = body
            .read(Range {
                address: (state + u64::from(c.mailbox.offset)) | c.read_alias,
                size: u64::from(c.mailbox.size),
            })
            .await?;
        if mailbox.len() != c.mailbox.size as usize {
            return Err(Error::Invalid("mailbox read size"));
        }
        let at = |offset: u32| {
            u32::from_le_bytes(
                mailbox[offset as usize..offset as usize + 4]
                    .try_into()
                    .unwrap(),
            )
        };
        let m = &c.mailbox;
        if at(m.nonce) != c.nonce
            || at(m.operation) != op.code(&self.image)
            || at(m.token) != token
            || at(m.core) != c.core
        {
            return Err(Error::Invalid(
                "control mailbox nonce/op/core/token mismatch",
            ));
        }
        let result = ControlResult {
            status: at(m.status) as i32,
            result: at(m.result),
            words_new: at(m.words_new),
            words_old: at(m.words_old),
            inits_done: m.inits_done.map(at),
        };
        if result.status != 0 {
            return Err(Error::Body(format!(
                "control {op:?} rejected: {}",
                result.status
            )));
        }
        let installed =
            Self::word(body, (state + u64::from(c.installed_offset)) | c.read_alias).await?;
        if installed > 1
            || (op == Operation::Install && installed != 1)
            || (op == Operation::Rollback && installed != 0)
        {
            return Err(Error::Invalid("installed state differs"));
        }
        self.lease.as_mut().unwrap().installed = installed == 1;
        let installed = installed == 1;
        for hook in &self.image.hooks {
            let wanted = if installed { hook.new } else { hook.old };
            if Self::word(body, hook.address | c.read_alias).await? != wanted {
                return Err(Error::Invalid("hook readback differs"));
            }
        }
        if op == Operation::Init
            && (result.inits_done != Some(1)
                || result.result != c.init_count
                || Self::word(
                    body,
                    (state + u64::from(c.inits_done_offset)) | c.read_alias,
                )
                .await?
                    != 1)
        {
            return Err(Error::Invalid("INIT count or completion differs"));
        }
        if matches!(op, Operation::Install | Operation::Rollback) {
            body.synchronize(self.range()).await?;
            for hook in &self.image.hooks {
                let wanted = if installed { hook.new } else { hook.old };
                if Self::word(body, hook.address | c.read_alias).await? != wanted {
                    return Err(Error::Invalid("hook changed after synchronization"));
                }
            }
        }
        Ok(result)
    }
    pub async fn set(&self, body: &mut impl Body, address: u64, bytes: Vec<u8>) -> Result<()> {
        let end = address
            .checked_add(bytes.len() as u64)
            .ok_or(Error::Invalid("write overflow"))?;
        if self.lease.as_ref().is_none_or(|l| l.freed)
            || address < self.image.link_base
            || end > self.image.link_base + self.image.bytes.len() as u64
        {
            return Err(Error::Invalid("write outside image"));
        }
        body.write(address, bytes.clone()).await?;
        if body
            .read(Range {
                address: address | self.image.control.read_alias,
                size: bytes.len() as u64,
            })
            .await?
            != bytes
        {
            return Err(Error::Invalid("write readback differs"));
        }
        Ok(())
    }
    pub async fn read(&self, body: &mut impl Body, range: Range) -> Result<Vec<u8>> {
        if range.size == 0 || range.address.checked_add(range.size).is_none() {
            return Err(Error::Invalid("invalid read range"));
        }
        let bytes = body.read(range).await?;
        if bytes.len() as u64 != range.size {
            return Err(Error::Invalid("read size differs"));
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests;
