//! Direct E1 reads from a cataloged CPU-mapped flash window.
use super::{Backup, Callbacks};
use crate::{
    Limits,
    bulk::{self, BulkError, MappedFailure, MappedPhase, Qualified},
};
use fujimod_catalog::capture::FirmwareFlash;
use fujimod_catalog_bulk::{BulkRange, FirmwareBulk};
use fujimod_usb::ff80::Session;
use sha2::{Digest, Sha256};

fn fail(phase: MappedPhase, address: u64, length: u64, message: &str) -> BulkError {
    receipt(phase, address, length, BulkError::Check(message.into()))
}

pub(super) fn receipt(
    phase: MappedPhase,
    address: u64,
    length: u64,
    source: BulkError,
) -> BulkError {
    let (source, cleanup) = match source {
        BulkError::Cleanup { operation, cleanup } => (operation, Some(cleanup)),
        other => (Box::new(other), None),
    };
    BulkError::Mapped(MappedFailure {
        phase,
        address,
        length,
        source,
        cleanup,
    })
}

fn context<T>(
    phase: MappedPhase,
    address: u64,
    length: u64,
    result: bulk::Result<T>,
) -> bulk::Result<T> {
    result.map_err(|source| receipt(phase, address, length, source))
}

pub(super) async fn read(
    session: &mut Session,
    bulk: &FirmwareBulk,
    flash: &FirmwareFlash,
    limits: Limits,
    target: BulkRange,
    callbacks: &mut Callbacks<'_>,
) -> bulk::Result<Backup> {
    context(
        MappedPhase::Window,
        target.address,
        target.size,
        flash.validate().map_err(BulkError::Check),
    )?;
    let aperture = flash.e1_window.as_ref().ok_or_else(|| {
        fail(
            MappedPhase::Window,
            target.address,
            target.size,
            "Direct E1 flash window is absent",
        )
    })?;
    let target_end = target.address.checked_add(target.size).ok_or_else(|| {
        fail(
            MappedPhase::Bounds,
            target.address,
            target.size,
            "Flash range overflow",
        )
    })?;
    if target.size == 0 || target_end > flash.window.size {
        return Err(fail(
            MappedPhase::Bounds,
            target.address,
            target.size,
            "Flash range is outside the mapped device",
        ));
    }
    context(
        MappedPhase::Progress,
        flash.window.address.saturating_add(target.address),
        target.size,
        (callbacks.progress)(0, target.size).map_err(BulkError::Check),
    )?;

    let aperture_end = aperture
        .address
        .checked_add(aperture.size)
        .ok_or_else(|| {
            fail(
                MappedPhase::Window,
                aperture.address,
                aperture.size,
                "Direct E1 aperture overflow",
            )
        })?;
    let bulk_end = bulk.window.address.checked_add(bulk.window.size).ok_or_else(|| {
        fail(
            MappedPhase::Window,
            bulk.window.address,
            bulk.window.size,
            "Bulk window overflow",
        )
    })?;
    let start = bulk.window.address.min(aperture.address);
    let end = bulk_end.max(aperture_end);
    let mut extended = bulk.clone();
    extended.window = BulkRange {
        address: start,
        size: end - start,
    };
    context(
        MappedPhase::Prepare,
        start,
        end - start,
        bulk::memory_range(extended.window),
    )?;
    context(
        MappedPhase::Prepare,
        start,
        end - start,
        bulk::check_identity(session, &extended).await,
    )?;
    let mut reader = context(
        MappedPhase::Prepare,
        start,
        end - start,
        Qualified::prepare_preserving(session, &extended, limits).await,
    )?;
    let outcome = async {
        let control_range = mapped_range(
            flash,
            flash.control.offset,
            flash.control.size,
            MappedPhase::ControlRead,
        )?;
        let control = context(
            MappedPhase::ControlRead,
            control_range.address,
            control_range.size,
            reader.read(control_range).await,
        )?;
        if sha256(&control) != flash.control.sha256 {
            return Err(fail(
                MappedPhase::ControlHash,
                control_range.address,
                control_range.size,
                "Mapped E1 control bytes differ from the update",
            ));
        }

        let alignment = u64::from(flash.reader.as_ref().map_or(1, |reader| reader.alignment));
        let capacity = reader
            .capacity()
            .min(reader.access.session.max_reply_bytes().get());
        let piece = u64::from(capacity) / alignment * alignment;
        if piece == 0 {
            return Err(fail(
                MappedPhase::Capacity,
                flash.window.address.saturating_add(target.address),
                target.size,
                "Direct E1 capacity is below the flash alignment",
            ));
        }

        let mut digest = Sha256::new();
        let mut delivered = 0;
        let mut offset = target.address;
        while offset < target_end {
            let size = piece.min(target_end - offset);
            let range = mapped_range(flash, offset, size, MappedPhase::Piece)?;
            let bytes = context(
                MappedPhase::Piece,
                range.address,
                range.size,
                reader.read(range).await,
            )?;
            if bytes.len() as u64 != size {
                return Err(fail(
                    MappedPhase::Piece,
                    range.address,
                    range.size,
                    "Direct E1 flash read returned another length",
                ));
            }
            digest.update(&bytes);
            context(
                MappedPhase::Delivery,
                range.address,
                range.size,
                (callbacks.chunk)(offset, bytes).map_err(BulkError::Check),
            )?;
            delivered += size;
            context(
                MappedPhase::Progress,
                range.address,
                range.size,
                (callbacks.progress)(delivered, target.size).map_err(BulkError::Check),
            )?;
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
    let saved = reader.saved();
    let restored = reader.restore().await;
    match (outcome, restored) {
        (Err(BulkError::Mapped(mut failure)), Err(cleanup)) => {
            failure.cleanup = Some(Box::new(cleanup));
            Err(BulkError::Mapped(failure))
        }
        (outcome, restored) => {
            let restored = restored.map_err(|source| {
                let (address, length) = saved.unwrap_or((target.address, target.size));
                BulkError::Mapped(MappedFailure {
                    phase: MappedPhase::Restore,
                    address,
                    length,
                    source: Box::new(source),
                    cleanup: None,
                })
            });
            bulk::finish(outcome, restored)
        }
    }
}

fn mapped_range(
    flash: &FirmwareFlash,
    offset: u64,
    size: u64,
    phase: MappedPhase,
) -> bulk::Result<BulkRange> {
    let aperture = flash
        .e1_window
        .as_ref()
        .ok_or_else(|| fail(phase, offset, size, "Direct E1 flash window is absent"))?;
    let address = flash
        .window
        .address
        .checked_add(offset)
        .ok_or_else(|| fail(phase, offset, size, "Direct E1 flash address overflow"))?;
    let end = address
        .checked_add(size)
        .ok_or_else(|| fail(phase, address, size, "Direct E1 flash range overflow"))?;
    let aperture_end = aperture
        .address
        .checked_add(aperture.size)
        .ok_or_else(|| fail(phase, aperture.address, aperture.size, "Direct E1 aperture overflow"))?;
    if size == 0 || address < aperture.address || end > aperture_end {
        return Err(fail(
            phase,
            address,
            size,
            "Flash range is outside the direct E1 window",
        ));
    }
    let range = BulkRange { address, size };
    context(phase, address, size, bulk::memory_range(range))?;
    Ok(range)
}

fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "mapped_tests.rs"]
mod receipt_tests;
