//! Direct E1 reads from a cataloged CPU-mapped flash window.
use super::{Backup, Callbacks};
use crate::{
    Limits,
    bulk::{self, BulkError, Qualified},
};
use fujimod_catalog::capture::FirmwareFlash;
use fujimod_catalog_bulk::{BulkRange, FirmwareBulk};
use fujimod_usb::ff80::Session;
use sha2::{Digest, Sha256};

pub(super) async fn read(
    session: &mut Session,
    bulk: &FirmwareBulk,
    flash: &FirmwareFlash,
    limits: Limits,
    target: BulkRange,
    callbacks: &mut Callbacks<'_>,
) -> bulk::Result<Backup> {
    flash.validate().map_err(BulkError::Check)?;
    let aperture = flash
        .e1_window
        .as_ref()
        .ok_or_else(|| BulkError::Check("Direct E1 flash window is absent".into()))?;
    let target_end = target
        .address
        .checked_add(target.size)
        .ok_or_else(|| BulkError::Check("Flash range overflow".into()))?;
    if target.size == 0 || target_end > flash.window.size {
        return Err(BulkError::Check(
            "Flash range is outside the mapped device".into(),
        ));
    }
    (callbacks.progress)(0, target.size).map_err(BulkError::Check)?;

    let aperture_end = aperture
        .address
        .checked_add(aperture.size)
        .ok_or_else(|| BulkError::Check("Direct E1 aperture overflow".into()))?;
    let bulk_end = bulk
        .window
        .address
        .checked_add(bulk.window.size)
        .ok_or_else(|| BulkError::Check("Bulk window overflow".into()))?;
    let start = bulk.window.address.min(aperture.address);
    let end = bulk_end.max(aperture_end);
    let mut extended = bulk.clone();
    extended.window = BulkRange {
        address: start,
        size: end - start,
    };
    bulk::memory_range(extended.window)?;
    bulk::check_identity(session, &extended).await?;
    let mut reader = Qualified::prepare_preserving(session, &extended, limits).await?;
    let outcome = async {
        let control_range = mapped_range(flash, flash.control.offset, flash.control.size)?;
        let control = reader.read(control_range).await?;
        if sha256(&control) != flash.control.sha256 {
            return Err(BulkError::Check(
                "Mapped E1 control bytes differ from the update".into(),
            ));
        }

        let alignment = u64::from(flash.reader.as_ref().map_or(1, |reader| reader.alignment));
        let capacity = reader
            .capacity()
            .min(reader.access.session.max_reply_bytes().get());
        let piece = u64::from(capacity) / alignment * alignment;
        if piece == 0 {
            return Err(BulkError::Check(
                "Direct E1 capacity is below the flash alignment".into(),
            ));
        }

        let mut digest = Sha256::new();
        let mut delivered = 0;
        let mut offset = target.address;
        while offset < target_end {
            let size = piece.min(target_end - offset);
            let range = mapped_range(flash, offset, size)?;
            let bytes = reader.read(range).await?;
            if bytes.len() as u64 != size {
                return Err(BulkError::Check(
                    "Direct E1 flash read returned another length".into(),
                ));
            }
            digest.update(&bytes);
            (callbacks.chunk)(offset, bytes).map_err(BulkError::Check)?;
            delivered += size;
            (callbacks.progress)(delivered, target.size).map_err(BulkError::Check)?;
            offset += size;
        }
        Ok(Backup {
            device: flash.device.clone(),
            size: target.size,
            sha256: hex(&digest.finalize()),
            uncorrectable_page_offsets: Vec::new(),
            uncorrectable_granularity_bytes: 0,
            nand_read_stats: None,
        })
    }
    .await;
    let restored = reader.restore().await;
    bulk::finish(outcome, restored)
}

fn mapped_range(flash: &FirmwareFlash, offset: u64, size: u64) -> bulk::Result<BulkRange> {
    let aperture = flash
        .e1_window
        .as_ref()
        .ok_or_else(|| BulkError::Check("Direct E1 flash window is absent".into()))?;
    let address = flash
        .window
        .address
        .checked_add(offset)
        .ok_or_else(|| BulkError::Check("Direct E1 flash address overflow".into()))?;
    let end = address
        .checked_add(size)
        .ok_or_else(|| BulkError::Check("Direct E1 flash range overflow".into()))?;
    let aperture_end = aperture
        .address
        .checked_add(aperture.size)
        .ok_or_else(|| BulkError::Check("Direct E1 aperture overflow".into()))?;
    if size == 0 || address < aperture.address || end > aperture_end {
        return Err(BulkError::Check(
            "Flash range is outside the direct E1 window".into(),
        ));
    }
    let range = BulkRange { address, size };
    bulk::memory_range(range)?;
    Ok(range)
}

fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
