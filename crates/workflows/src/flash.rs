//! Whole-flash backup through processor-specific stock readers and owned RAM.
pub mod exr2;
mod xp5;
pub use crate::callback::{CodeCheck, Range, Transport, WordCheck};
use crate::{Limits, bulk};
use fujimod_catalog::capture::FirmwareFlash;
use fujimod_catalog_bulk::FirmwareBulk;
use fujimod_usb::ff80::Session;
use serde::{Deserialize, Serialize};
pub use xp5::Backup;

/// Data delivery and logical flash-byte progress. Progress starts at zero and
/// advances after each successfully delivered chunk, excluding the control read.
pub struct Callbacks<'a> {
    pub chunk: &'a mut dyn FnMut(u64, Vec<u8>) -> Result<(), String>,
    pub progress: &'a mut dyn FnMut(u64, u64) -> Result<(), String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Payload {
    Xp5Command(xp5::Payload),
    Exr2Query(exr2::Payload),
}
impl Payload {
    pub fn firmware_id(&self) -> &str {
        match self {
            Self::Xp5Command(p) => &p.firmware_id,
            Self::Exr2Query(p) => &p.firmware_id,
        }
    }
    pub fn buffer_capacity(&self) -> u32 {
        match self {
            Self::Xp5Command(p) => p.buffer_capacity,
            Self::Exr2Query(_) => 0x8000,
        }
    }
    pub fn firmware_sha256(&self) -> &str {
        match self {
            Self::Xp5Command(p) => &p.firmware_sha256,
            Self::Exr2Query(p) => &p.firmware_sha256,
        }
    }
    pub fn validate(
        &self,
        flash: &FirmwareFlash,
        bulk: &FirmwareBulk,
        image: &[u8],
    ) -> bulk::Result<()> {
        match self {
            Self::Xp5Command(p) => p.validate(flash, bulk, image),
            Self::Exr2Query(p) => p.validate(flash, bulk, image),
        }
    }
}
pub async fn backup(
    session: &mut Session,
    bulk: &FirmwareBulk,
    flash: &FirmwareFlash,
    payload: &Payload,
    image: &[u8],
    limits: Limits,
    callbacks: &mut Callbacks<'_>,
) -> bulk::Result<Backup> {
    match payload {
        Payload::Xp5Command(p) => {
            xp5::backup(session, bulk, flash, p, image, limits, callbacks).await
        }
        Payload::Exr2Query(p) => {
            exr2::backup(session, bulk, flash, p, image, limits, callbacks).await
        }
    }
}
#[cfg(feature = "artifact-build")]
pub mod build;
