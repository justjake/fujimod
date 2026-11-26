//! Catalog-selected camera state through the qualified stock bulk reader.
use crate::{Limits, bulk};
use fujimod_catalog::capture::{FirmwareState, StateRegion};
use fujimod_catalog_bulk::{BulkRange, FirmwareBulk};
use fujimod_usb::{ff80::Session, operations::checks::sha256_bytes};

#[derive(Debug)]
pub struct CapturedRegion {
    pub region: StateRegion,
    pub data: Vec<u8>,
    pub sha256: String,
}

/// Read a region, splitting at the E1 window boundary when needed. Bytes
/// inside the window use fast E1 bulk reads; bytes outside use direct
/// memory reads.
async fn read_region(
    reader: &mut bulk::Qualified<'_>,
    range: BulkRange,
    window: BulkRange,
    advance: &mut dyn FnMut(u64) -> std::result::Result<(), String>,
) -> bulk::Result<Vec<u8>> {
    if window.contains(range) {
        return reader.read_progress(range, advance).await;
    }
    let (Some(range_end), Some(win_end)) = (
        range.address.checked_add(range.size),
        window.address.checked_add(window.size),
    ) else {
        return reader.read_direct_progress(range, advance).await;
    };
    let overlap_start = range.address.max(window.address);
    let overlap_end = range_end.min(win_end);
    if overlap_start >= overlap_end || window.size == 0 {
        return reader.read_direct_progress(range, advance).await;
    }
    // Partial overlap: split into zero-filled prefix, E1 middle, direct suffix.
    // Addresses below the E1 window (e.g. 0x0..0x4000 on XP5) may be
    // unmapped or protected and crash the camera on direct memory reads.
    let mut data = Vec::with_capacity(range.size as usize);
    if range.address < overlap_start {
        let prefix_size = (overlap_start - range.address) as usize;
        data.resize(prefix_size, 0);
        advance(prefix_size as u64).map_err(bulk::BulkError::Check)?;
    }
    data.extend(
        reader
            .read_progress(
                BulkRange {
                    address: overlap_start,
                    size: overlap_end - overlap_start,
                },
                advance,
            )
            .await?,
    );
    if overlap_end < range_end {
        data.extend(
            reader
                .read_direct_progress(
                    BulkRange {
                        address: overlap_end,
                        size: range_end - overlap_end,
                    },
                    advance,
                )
                .await?,
        );
    }
    Ok(data)
}

/// Capture each region in catalog order. Constant regions are compared before
/// delivery. All-zero or all-FF constants are masked and omitted. A sink error
/// stops the capture and still restores the borrowed bulk mapping.
/// Byte progress includes both reads of constant regions, excluding qualification.
pub async fn dump(
    session: &mut Session,
    bulk: &FirmwareBulk,
    state: &FirmwareState,
    limits: Limits,
    sink: &mut dyn FnMut(CapturedRegion) -> std::result::Result<(), String>,
    progress: &mut dyn FnMut(u64, u64) -> std::result::Result<(), String>,
) -> bulk::Result<()> {
    state.validate().map_err(bulk::BulkError::Check)?;
    if state.firmware_id != bulk.firmware_id {
        return Err(bulk::BulkError::Check(
            "State and bulk firmware differ".into(),
        ));
    }
    let total = state
        .regions
        .iter()
        .try_fold(0u64, |total, region| {
            region
                .size
                .checked_mul(if region.constant { 2 } else { 1 })
                .and_then(|size| total.checked_add(size))
        })
        .ok_or(bulk::BulkError::Check("State byte total overflows".into()))?;
    progress(0, total).map_err(bulk::BulkError::Check)?;
    let mut done = 0;
    let mut advance = |bytes| {
        done += bytes;
        progress(done, total)
    };
    // The E1 source reaches only the DRAM window: a source address outside it
    // wraps into DRAM. Other regions (boot ROM) use direct memory reads.
    for region in &state.regions {
        super::bulk::memory_range(BulkRange {
            address: region.address,
            size: region.size,
        })?;
    }
    let facts = bulk;
    let mut reader = bulk::Qualified::prepare(session, facts, limits).await?;
    let outcome = async {
        for region in &state.regions {
            let range = BulkRange {
                address: region.address,
                size: region.size,
            };
            let data = read_region(&mut reader, range, bulk.window, &mut advance).await?;
            if region.constant {
                // Regions inside the E1 window get a reliable double-read
                // comparison. Regions entirely outside it (e.g. boot ROM at
                // 0xFFE00000) use ff80 direct reads that can return different
                // bytes between passes on some processors — skip the check
                // and deliver the first read.
                if bulk.window.contains(range) {
                    if read_region(&mut reader, range, bulk.window, &mut advance).await? != data {
                        return Err(bulk::BulkError::Check(format!(
                            "Constant state region {} changed between reads",
                            region.name
                        )));
                    }
                } else {
                    advance(range.size).map_err(bulk::BulkError::Check)?;
                }
                if masked(&data) {
                    continue;
                }
            }
            let sha256 = super::bulk::hex(&sha256_bytes(&data));
            sink(CapturedRegion {
                region: region.clone(),
                data,
                sha256,
            })
            .map_err(bulk::BulkError::Check)?;
        }
        Ok(())
    }
    .await;
    let restored = reader.restore().await;
    bulk::finish(outcome, restored)
}
fn masked(bytes: &[u8]) -> bool {
    bytes.iter().all(|&b| b == 0) || bytes.iter().all(|&b| b == 0xff)
}

#[cfg(test)]
mod tests;
