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
    Teardown,
    Rollback,
}
impl Operation {
    pub fn code(self, image: &RuntimeImage) -> Result<u32> {
        let ops = &image.control.operations;
        Ok(match self {
            Self::Install => ops.install,
            Self::Init => ops.init,
            Self::Status => ops.status,
            Self::Teardown => ops
                .teardown
                .ok_or(Error::Invalid("image has no TEARDOWN operation"))?,
            Self::Rollback => ops.rollback,
        })
    }
}
/// Allocation and callback methods normally use operations::callback::invoke.
/// The body owns transport access and supplies all architecture-specific work.
pub trait Body: Send {
    fn allocate(&mut self, size: u64, align: u64) -> Pending<'_, u64>;
    fn free(&mut self, base: u64) -> Pending<'_, ()>;
    fn read(&mut self, range: Range) -> Pending<'_, Vec<u8>>;
    fn write(&mut self, address: u64, bytes: Vec<u8>) -> Pending<'_, ()>;
    /// Read several independent ranges. Transport bodies service the whole
    /// list with one device wait; this default reads them in order.
    fn read_many(&mut self, ranges: Vec<Range>) -> Pending<'_, Vec<Vec<u8>>> {
        Box::pin(async move {
            let mut out = Vec::with_capacity(ranges.len());
            for range in ranges {
                out.push(self.read(range).await?);
            }
            Ok(out)
        })
    }
    /// Apply several ordered writes. Transport bodies send the whole list with
    /// one device wait and no per-write readback; this default writes in order.
    fn write_many(&mut self, writes: Vec<(u64, Vec<u8>)>) -> Pending<'_, ()> {
        Box::pin(async move {
            for (address, bytes) in writes {
                self.write(address, bytes).await?;
            }
            Ok(())
        })
    }
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
    pub finis_done: Option<u32>,
    pub live: Option<u32>,
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
    /// Stop what INIT started. Repeat until `live` is zero: a task between
    /// safe points stays held until a later TEARDOWN.
    pub async fn teardown(&mut self, body: &mut impl Body, token: u32) -> Result<ControlResult> {
        self.control(body, Operation::Teardown, token).await
    }
    pub async fn rollback(&mut self, body: &mut impl Body, token: u32) -> Result<ControlResult> {
        self.control(body, Operation::Rollback, token).await
    }
    /// Free a published image that can no longer be entered: one STATUS must
    /// report every hook stock, TEARDOWN complete when INIT ran, and nothing
    /// held. The body checks its target's own registries before freeing.
    pub async fn release(&mut self, body: &mut impl Body, token: u32) -> Result<ControlResult> {
        let status = self.status(body, token).await?;
        let lease = self.lease.as_ref().ok_or(Error::Invalid("not allocated"))?;
        if lease.installed
            || status.words_new != 0
            || status.words_old as usize != self.image.hooks.len()
        {
            return Err(Error::Invalid("release requires every hook stock"));
        }
        let initialized = match status.inits_done {
            Some(done) => done != 0,
            None => self.image.control.init_count != 0,
        };
        if initialized && status.finis_done != Some(1) {
            return Err(Error::Invalid("release after INIT requires TEARDOWN"));
        }
        if status.live.unwrap_or(0) != 0 {
            return Err(Error::Invalid("release requires no held resources"));
        }
        let base = lease.base;
        body.free(base).await?;
        self.lease.as_mut().unwrap().freed = true;
        Ok(status)
    }
    async fn words(body: &mut impl Body, addresses: &[u64]) -> Result<Vec<u32>> {
        let ranges = addresses
            .iter()
            .map(|&address| Range { address, size: 4 })
            .collect();
        body.read_many(ranges)
            .await?
            .into_iter()
            .map(|bytes| {
                Ok(u32::from_le_bytes(
                    bytes
                        .try_into()
                        .map_err(|_| Error::Invalid("word read size"))?,
                ))
            })
            .collect()
    }
    /// One control operation. Device waits: one ownership read, one write
    /// batch, the body's synchronization and control call, one result read, and
    /// for INSTALL/ROLLBACK a synchronization and one hook read. The mailbox
    /// echo of nonce, operation and the fresh token verifies the control words;
    /// installed state and every hook word verify the final state.
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
        let word = |offset: u32| Range {
            address: (state + u64::from(offset)) | c.read_alias,
            size: 4,
        };
        let ownership = body
            .read_many(vec![
                Range {
                    address: self.image.link_base | c.read_alias,
                    size: u64::from(c.code_size),
                },
                word(c.magic_offset),
                word(c.nonce_offset),
            ])
            .await?;
        if ownership.len() != 3
            || ownership[0] != self.image.bytes[..c.code_size as usize]
            || ownership[1] != c.magic.to_le_bytes()
            || ownership[2] != c.nonce.to_le_bytes()
        {
            return Err(Error::Invalid("image ownership lost"));
        }
        let mut writes = vec![
            (
                state + u64::from(c.token_offset),
                token.to_le_bytes().to_vec(),
            ),
            (
                state + u64::from(c.mailbox.offset),
                vec![0xa5; c.mailbox.size as usize],
            ),
        ];
        let code = op.code(&self.image)?;
        if let Some(offset) = c.operation_offset {
            writes.push((state + u64::from(offset), code.to_le_bytes().to_vec()));
        }
        self.write_many(body, writes).await?;
        body.synchronize(self.range()).await?;
        // Even unknown completion can have exposed hooks. Never free afterward.
        if op == Operation::Install {
            self.lease.as_mut().unwrap().publication_attempted = true;
        }
        body.control(self.image.link_base + u64::from(c.control_offset), code)
            .await?;
        let mut ranges = vec![
            Range {
                address: (state + u64::from(c.mailbox.offset)) | c.read_alias,
                size: u64::from(c.mailbox.size),
            },
            word(c.installed_offset),
            word(c.inits_done_offset),
        ];
        ranges.extend(self.image.hooks.iter().map(|hook| Range {
            address: hook.address | c.read_alias,
            size: 4,
        }));
        let replies = body.read_many(ranges).await?;
        if replies.len() != 3 + self.image.hooks.len() {
            return Err(Error::Invalid("result read count"));
        }
        let mailbox = &replies[0];
        if mailbox.len() != c.mailbox.size as usize {
            return Err(Error::Invalid("mailbox read size"));
        }
        let value = |bytes: &[u8]| -> Result<u32> {
            Ok(u32::from_le_bytes(
                bytes
                    .try_into()
                    .map_err(|_| Error::Invalid("word read size"))?,
            ))
        };
        let at = |offset: u32| {
            u32::from_le_bytes(
                mailbox[offset as usize..offset as usize + 4]
                    .try_into()
                    .unwrap(),
            )
        };
        let m = &c.mailbox;
        if at(m.nonce) != c.nonce
            || at(m.operation) != code
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
            finis_done: m.finis_done.map(at),
            live: m.live.map(at),
        };
        if result.status != 0 {
            return Err(Error::Body(format!(
                "control {op:?} rejected: {}",
                result.status
            )));
        }
        let installed = value(&replies[1])?;
        if installed > 1
            || (op == Operation::Install && installed != 1)
            || (op == Operation::Rollback && installed != 0)
        {
            return Err(Error::Invalid("installed state differs"));
        }
        self.lease.as_mut().unwrap().installed = installed == 1;
        let installed = installed == 1;
        let wanted: Vec<u32> = self
            .image
            .hooks
            .iter()
            .map(|hook| if installed { hook.new } else { hook.old })
            .collect();
        for (bytes, wanted) in replies[3..].iter().zip(&wanted) {
            if value(bytes)? != *wanted {
                return Err(Error::Invalid("hook readback differs"));
            }
        }
        if op == Operation::Teardown && result.finis_done != Some(1) {
            return Err(Error::Invalid("TEARDOWN did not complete"));
        }
        if op == Operation::Init
            && (result.inits_done != Some(1)
                || result.result != c.init_count
                || value(&replies[2])? != 1)
        {
            return Err(Error::Invalid("INIT count or completion differs"));
        }
        if matches!(op, Operation::Install | Operation::Rollback) {
            body.synchronize(self.range()).await?;
            let addresses: Vec<u64> = self
                .image
                .hooks
                .iter()
                .map(|hook| hook.address | c.read_alias)
                .collect();
            if Self::words(body, &addresses).await? != wanted {
                return Err(Error::Invalid("hook changed after synchronization"));
            }
        }
        Ok(result)
    }
    fn check_write(&self, address: u64, length: usize) -> Result<()> {
        let end = address
            .checked_add(length as u64)
            .ok_or(Error::Invalid("write overflow"))?;
        if self.lease.as_ref().is_none_or(|l| l.freed)
            || address < self.image.link_base
            || end > self.image.link_base + self.image.bytes.len() as u64
        {
            return Err(Error::Invalid("write outside image"));
        }
        Ok(())
    }
    async fn write_many(&self, body: &mut impl Body, writes: Vec<(u64, Vec<u8>)>) -> Result<()> {
        for (address, bytes) in &writes {
            self.check_write(*address, bytes.len())?;
        }
        body.write_many(writes).await
    }
    /// Write owned image bytes. No readback: the caller verifies the state the
    /// write is meant to produce, since the image code may consume it at once.
    pub async fn set(&self, body: &mut impl Body, address: u64, bytes: Vec<u8>) -> Result<()> {
        self.write_many(body, vec![(address, bytes)]).await
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
