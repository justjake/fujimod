//! Whole-flash backup through processor-specific stock readers and owned RAM.
pub mod exr2;
mod xp5;
pub use crate::callback::{CodeCheck, Range, Transport, WordCheck};
use crate::{Limits, bulk};
use fujimod_catalog::capture::FirmwareFlash;
use fujimod_catalog_bulk::{BulkRange, FirmwareBulk};
use fujimod_usb::ff80::Session;
use serde::{Deserialize, Serialize};
pub use xp5::Backup;
/// Capture buffers use the stock bulk transport's MiB-scale transfers.
pub const CAPTURE_CHUNK_BYTES: u32 = 8 << 20;

/// Stock NAND execution counts, including the control read and ECC fallback.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct NandReadStats {
    pub buffer_bytes: u64,
    pub stock_calls: u64,
    pub burst_calls: u64,
    pub fallback_pages: u64,
    pub uncorrectable_burst_pages: u64,
}

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
            Self::Exr2Query(_) => CAPTURE_CHUNK_BYTES,
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
/// Whole-device backups retain their recorded qualification sequence. File
/// ranges borrow publication state without generating diagnostic scratch.
#[derive(Clone, Copy)]
pub(super) enum Selection {
    Whole,
    Range(BulkRange),
    DiagnosticNand(BulkRange, NandReadMode),
}
impl Selection {
    fn range(self, flash: &FirmwareFlash) -> BulkRange {
        match self {
            Self::Whole => BulkRange {
                address: 0,
                size: flash.window.size,
            },
            Self::Range(range) | Self::DiagnosticNand(range, _) => range,
        }
    }
    fn controller_mode(self) -> u32 {
        match self {
            Self::DiagnosticNand(_, NandReadMode::Uncorrected) => 1,
            _ => 0,
        }
    }
    fn allows_uncorrectable(self) -> bool {
        !matches!(self, Self::Range(_))
    }
    fn preserves_source(self) -> bool {
        !matches!(self, Self::Whole)
    }
}

/// Diagnostic comparison only; uncorrected reads do not establish data integrity.
#[derive(Clone, Copy)]
pub enum NandReadMode {
    Corrected,
    Uncorrected,
}

/// Read a bounded diagnostic NAND range with explicit correction mode. The
/// corrected control read is always required, and correction is restored after
/// each uncorrected stock call, including completed error returns.
pub async fn diagnose_nand(
    session: &mut Session,
    bulk: &FirmwareBulk,
    flash: &FirmwareFlash,
    payload: &Payload,
    image: &[u8],
    limits: Limits,
    offset: u64,
    size: u32,
    mode: NandReadMode,
    callbacks: &mut Callbacks<'_>,
) -> bulk::Result<Backup> {
    let target = enclosing(flash, offset, size)?;
    if target.address != offset || target.size != u64::from(size) {
        return Err(bulk::BulkError::Check(
            "Diagnostic range must be page aligned".into(),
        ));
    }
    let Payload::Exr2Query(p) = payload else {
        return Err(bulk::BulkError::Check(
            "Selected reader has no diagnostic NAND mode".into(),
        ));
    };
    exr2::read(
        session,
        bulk,
        flash,
        p,
        image,
        limits,
        Selection::DiagnosticNand(target, mode),
        callbacks,
    )
    .await
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
            xp5::read(
                session,
                bulk,
                flash,
                p,
                image,
                limits,
                Selection::Whole,
                callbacks,
            )
            .await
        }
        Payload::Exr2Query(p) => {
            exr2::read(
                session,
                bulk,
                flash,
                p,
                image,
                limits,
                Selection::Whole,
                callbacks,
            )
            .await
        }
    }
}

/// Prepared code and its metadata; image bytes remain a separate binary file.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub payload: Payload,
    pub image: String,
}

/// Read only the pages containing the requested logical storage bytes. The
/// processor backend checks a control page and restores its RAM and callbacks
/// before returning; it never programs or erases internal storage.
pub async fn read_range(
    session: &mut Session,
    bulk: &FirmwareBulk,
    flash: &FirmwareFlash,
    payload: &Payload,
    image: &[u8],
    limits: Limits,
    offset: u64,
    size: u32,
) -> bulk::Result<Vec<u8>> {
    let target = enclosing(flash, offset, size)?;
    payload.validate(flash, bulk, image)?;
    let mut data = vec![0; size as usize];
    let mut spans = std::collections::BTreeMap::<u64, u64>::new();
    let mut delivered = 0u64;
    let mut chunk = |at: u64, bytes: Vec<u8>| {
        let first = offset.max(at);
        let last = (offset + u64::from(size)).min(at + bytes.len() as u64);
        if first < last {
            if spans
                .range(..=first)
                .next_back()
                .is_some_and(|(_, end)| *end > first)
                || spans
                    .range(first..)
                    .next()
                    .is_some_and(|(start, _)| *start < last)
            {
                return Err("Overlapping flash range".into());
            }
            data[(first - offset) as usize..(last - offset) as usize]
                .copy_from_slice(&bytes[(first - at) as usize..(last - at) as usize]);
            spans.insert(first, last);
            delivered += last - first;
        }
        Ok(())
    };
    let mut progress = |_, _| Ok(());
    let mut callbacks = Callbacks {
        chunk: &mut chunk,
        progress: &mut progress,
    };
    let report = match payload {
        Payload::Xp5Command(p) => {
            xp5::read(
                session,
                bulk,
                flash,
                p,
                image,
                limits,
                Selection::Range(target),
                &mut callbacks,
            )
            .await?
        }
        Payload::Exr2Query(p) => {
            exr2::read(
                session,
                bulk,
                flash,
                p,
                image,
                limits,
                Selection::Range(target),
                &mut callbacks,
            )
            .await?
        }
    };
    if !report.uncorrectable_page_offsets.is_empty() {
        return Err(bulk::BulkError::Check(
            "Requested flash range has uncorrectable NAND pages".into(),
        ));
    }
    if delivered != u64::from(size) {
        return Err(bulk::BulkError::Check("Incomplete flash range".into()));
    }
    Ok(data)
}

fn enclosing(flash: &FirmwareFlash, offset: u64, size: u32) -> bulk::Result<BulkRange> {
    let alignment = u64::from(flash.reader.alignment);
    let end = offset
        .checked_add(u64::from(size))
        .ok_or_else(|| bulk::BulkError::Check("Flash range overflow".into()))?;
    if size == 0 || alignment == 0 || end > flash.window.size {
        return Err(bulk::BulkError::Check(
            "Flash range is outside storage".into(),
        ));
    }
    let first = offset / alignment * alignment;
    let last = end
        .div_ceil(alignment)
        .checked_mul(alignment)
        .filter(|last| *last <= flash.window.size)
        .ok_or_else(|| bulk::BulkError::Check("Flash page exceeds storage".into()))?;
    Ok(BulkRange {
        address: first,
        size: last - first,
    })
}

pub(super) fn read_pieces(
    target: BulkRange,
    capacity: u64,
    update_size: u64,
    total: u64,
) -> Vec<(u64, u64)> {
    if target.address == 0 && target.size == total {
        xp5::pieces(total, capacity, update_size)
    } else {
        (target.address..target.address + target.size)
            .step_by(capacity as usize)
            .map(|at| (at, capacity.min(target.address + target.size - at)))
            .collect()
    }
}

#[cfg(feature = "artifact-build")]
pub mod build;
