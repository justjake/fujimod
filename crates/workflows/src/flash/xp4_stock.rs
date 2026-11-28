//! X-Processor 4 flash reads through the stock flash reader alone.
//!
//! No code is uploaded. For one diagnostic query per chunk, the `0x450006`
//! query callback slot points at the stock reader. The query passes the
//! address of a request in the registered diagnostic buffer, and the reader
//! copies flash into that buffer. The slot is restored after every query and
//! the borrowed buffer bytes are restored after the capture.
//!
//! The stock reader refuses a request that ends at the end of the flash
//! window, so the final `TAIL_BYTES` come from CPU reads of the mapped window,
//! after a stock read and a CPU read of the bytes before them agree.
use super::xp5::OrderedDigest;
use crate::{
    Limits,
    bulk::{self, BulkError},
    callback::{CodeCheck, require, word},
};
use fujimod_catalog::capture::FirmwareFlash;
use fujimod_catalog_bulk::{BulkRange, BulkSource, FirmwareBulk};
use fujimod_usb::{
    ff80::{
        Session, SessionState,
        commands::diagnostic::{CommandStatus, DiagnosticTag},
    },
    operations::checks::sha256_bytes,
};
use serde::{Deserialize, Serialize};

/// The flash contract transport this payload serves.
pub const TRANSPORT: &str = "xp4-stock-query";
/// Destination, source and size alignment of every stock request.
pub(super) const READ_ALIGNMENT: u64 = 128;
/// The largest stock read per query.
pub(super) const CHUNK_CAPACITY: u64 = 0x8000;
/// Request bytes at the start of the borrowed buffer.
const REQUEST_BYTES: u64 = 16;
/// Offset of the reader's destination in the borrowed buffer.
const DESTINATION: u64 = 128;
/// Bytes of the borrowed diagnostic buffer.
const BORROWED_BYTES: u64 = DESTINATION + CHUNK_CAPACITY;
/// Final window bytes read through the CPU aperture.
pub(super) const TAIL_BYTES: u64 = 0x1000;
/// Bytes before the tail read both ways and compared.
const OVERLAP_BYTES: u64 = 256;

/// Metadata for a stock-reader capture; it carries no image.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub schema: u32,
    pub firmware_id: String,
    pub firmware_sha256: String,
    pub stock_reader: u64,
    pub stock_reader_symbol: String,
    pub callback: Callback,
    pub checks: Vec<CodeCheck>,
    pub control_sha256: String,
    pub update_size: u64,
}

/// The stock `0x450006` query callback: the slot at `offset` in the table
/// whose address the word at `table` holds, and the slot's stock value.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Callback {
    pub table: u64,
    pub offset: u32,
    pub original: u32,
    pub checks: Vec<CodeCheck>,
}

impl Payload {
    /// Validate the payload against the catalog before the first USB command.
    pub fn validate(
        &self,
        flash: &FirmwareFlash,
        bulk: &FirmwareBulk,
        image: &[u8],
    ) -> bulk::Result<()> {
        flash.validate().map_err(BulkError::Check)?;
        require(
            self.schema == 1
                && self.firmware_id == flash.firmware_id
                && self.firmware_id == bulk.firmware_id,
            "XP4 stock payload and catalog firmware differ",
        )?;
        require(
            flash.transport.as_deref() == Some(TRANSPORT),
            format!("Unsupported XP4 flash transport {:?}", flash.transport),
        )?;
        require(image.is_empty(), "The XP4 stock reader takes no image")?;
        require(
            valid_hash(&self.firmware_sha256)
                && valid_hash(&self.control_sha256)
                && self.control_sha256 == flash.control.sha256,
            "XP4 stock payload hash fields differ",
        )?;
        registered_buffer(bulk)?;
        let reader = flash
            .reader
            .as_ref()
            .ok_or_else(|| BulkError::Check("XP4 stock flash reader is absent".into()))?;
        require(
            self.stock_reader_symbol == reader.symbol
                && flash
                    .code_ranges
                    .iter()
                    .any(|range| range.name == reader.symbol && range.address == self.stock_reader)
                && u32::try_from(self.stock_reader).is_ok_and(|entry| entry.is_multiple_of(4))
                && reader.alignment != 0
                && READ_ALIGNMENT.is_multiple_of(u64::from(reader.alignment)),
            "XP4 stock reader differs from the catalog",
        )?;
        let window = &flash.window;
        require(
            window.address.is_multiple_of(READ_ALIGNMENT)
                && window.size.is_multiple_of(READ_ALIGNMENT)
                && window.size >= TAIL_BYTES + OVERLAP_BYTES
                && window
                    .address
                    .checked_add(window.size)
                    .is_some_and(|end| end <= 1 << 32)
                && flash.control.size != 0
                && flash.control.size <= CHUNK_CAPACITY
                && flash.control.offset.is_multiple_of(READ_ALIGNMENT)
                && flash.control.size.is_multiple_of(READ_ALIGNMENT)
                && flash
                    .control
                    .offset
                    .checked_add(flash.control.size)
                    .is_some_and(|end| end <= self.update_size && end <= window.size - TAIL_BYTES)
                && self.update_size <= window.size,
            "XP4 stock reader, control, or update geometry differs",
        )?;
        require(
            self.callback.table.is_multiple_of(4)
                && self.callback.offset.is_multiple_of(4)
                && self.callback.original != 0
                && self.callback.original.is_multiple_of(4)
                && u64::from(self.callback.original) != self.stock_reader,
            "Invalid XP4 query callback",
        )?;
        bulk::memory_range(range(self.callback.table, 4))?;
        for check in self.checks.iter().chain(&self.callback.checks) {
            require(
                check.size != 0 && valid_hash(&check.sha256),
                "Invalid XP4 stock code check",
            )?;
            bulk::memory_range(range(check.address, check.size))?;
        }
        require(
            self.checks.iter().any(|check| {
                check.address == self.stock_reader
                    && flash
                        .code_ranges
                        .iter()
                        .any(|code| code.address == check.address && code.size == check.size)
            }),
            "The XP4 stock reader code is not checked",
        )
    }
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn range(address: u64, size: u64) -> BulkRange {
    BulkRange { address, size }
}

/// Address of the registered diagnostic buffer's pointer and capacity words.
fn registered_buffer(bulk: &FirmwareBulk) -> bulk::Result<u64> {
    match &bulk.source {
        BulkSource::Pattern {
            registered_buffer, ..
        } => Ok(*registered_buffer),
        _ => Err(BulkError::Check(
            "The XP4 stock reader needs a registered 32-bit diagnostic buffer".into(),
        )),
    }
}

/// Read a whole flash or a bounded aligned range with the stock XP4 reader.
pub(crate) async fn read(
    session: &mut Session,
    bulk: &FirmwareBulk,
    flash: &FirmwareFlash,
    payload: &Payload,
    image: &[u8],
    limits: Limits,
    selection: super::Selection<'_>,
    callbacks: &mut super::Callbacks<'_>,
) -> bulk::Result<super::Backup> {
    payload.validate(flash, bulk, image)?;
    let target = selection.range(flash);
    require(
        target.size != 0
            && target
                .address
                .checked_add(target.size)
                .is_some_and(|end| end <= flash.window.size),
        "XP4 read target is outside the flash window",
    )?;
    (callbacks.progress)(0, target.size).map_err(BulkError::Check)?;
    bulk::check_identity(session, bulk).await?;
    let mut access = bulk::Access { session, limits };
    execute(&mut access, bulk, flash, payload, target, callbacks).await
}

async fn execute(
    access: &mut bulk::Access<'_>,
    bulk: &FirmwareBulk,
    flash: &FirmwareFlash,
    payload: &Payload,
    target: BulkRange,
    callbacks: &mut super::Callbacks<'_>,
) -> bulk::Result<super::Backup> {
    check_code(access, &payload.callback.checks).await?;
    check_code(access, &payload.checks).await?;
    for check in &flash.state_checks {
        require(
            word(&access.raw(check.address, u64::from(check.size)).await?) == check.value,
            format!("Flash driver state differs: {}", check.name),
        )?;
    }
    let descriptor_address = registered_buffer(bulk)?;
    let descriptor = access.raw(descriptor_address, 8).await?;
    let (base, capacity) = (word(&descriptor[..4]), word(&descriptor[4..]));
    require(
        base != 0
            && base.is_multiple_of(READ_ALIGNMENT)
            && capacity >= BORROWED_BYTES
            && base.checked_add(capacity).is_some_and(|end| end <= 1 << 32),
        "Invalid registered diagnostic buffer",
    )?;
    let overlaps =
        |address: u64, size: u64| address < base + BORROWED_BYTES && base < address + size;
    let table = word(&access.raw(payload.callback.table, 4).await?);
    let slot = table + u64::from(payload.callback.offset);
    require(
        table != 0
            && !overlaps(slot, 4)
            && !overlaps(descriptor_address, 8)
            && !overlaps(payload.callback.table, 4)
            && !payload
                .checks
                .iter()
                .chain(&payload.callback.checks)
                .any(|check| overlaps(check.address, check.size))
            && !flash
                .state_checks
                .iter()
                .any(|check| overlaps(check.address, u64::from(check.size))),
        "The borrowed diagnostic buffer overlaps checked state",
    )?;
    bulk::memory_range(range(slot, 4))?;
    let mut borrow = Borrow {
        base,
        saved: access.raw(base, BORROWED_BYTES).await?,
        request: None,
    };
    let mut slot = Slot {
        address: slot,
        original: payload.callback.original,
        reader: u32::try_from(payload.stock_reader)
            .map_err(|_| BulkError::Check("XP4 stock reader exceeds A32".into()))?,
        published: false,
    };
    let reader = Reader {
        flash,
        descriptor_address,
        descriptor,
    };
    let capture = async {
        let control = reader
            .read(
                access,
                &mut borrow,
                &mut slot,
                flash.control.offset,
                flash.control.size,
            )
            .await?;
        require(
            bulk::hex(&sha256_bytes(&control)) == payload.control_sha256,
            "Control chunk differs from update; no backup read",
        )?;
        let tail = flash.window.size - TAIL_BYTES;
        let mut digest = OrderedDigest::default();
        let mut done = 0;
        let stock = BulkRange {
            address: target.address,
            size: target.size.min(tail.saturating_sub(target.address)),
        };
        if stock.size != 0 {
            for (offset, size) in
                super::read_pieces(stock, CHUNK_CAPACITY, payload.update_size, tail)
            {
                let bytes = reader
                    .read(access, &mut borrow, &mut slot, offset, size)
                    .await?;
                digest.push(offset - target.address, &bytes);
                (callbacks.chunk)(offset, bytes).map_err(BulkError::Check)?;
                done += size;
                (callbacks.progress)(done, target.size).map_err(BulkError::Check)?;
            }
        }
        let end = target.address + target.size;
        if end > tail {
            let stocked = reader
                .read(
                    access,
                    &mut borrow,
                    &mut slot,
                    tail - OVERLAP_BYTES,
                    OVERLAP_BYTES,
                )
                .await?;
            let window = flash.window.address;
            require(
                access
                    .raw(window + tail - OVERLAP_BYTES, OVERLAP_BYTES)
                    .await?
                    == stocked,
                "CPU flash window differs from the stock reader",
            )?;
            let start = target.address.max(tail);
            let bytes = access.raw(window + start, end - start).await?;
            digest.push(start - target.address, &bytes);
            (callbacks.chunk)(start, bytes).map_err(BulkError::Check)?;
            done += end - start;
            (callbacks.progress)(done, target.size).map_err(BulkError::Check)?;
        }
        Ok(super::Backup {
            device: flash.device.clone(),
            size: target.size,
            sha256: digest.finish(target.size)?,
            uncorrectable_page_offsets: Vec::new(),
            uncorrectable_granularity_bytes: 0,
            nand_read_stats: None,
        })
    }
    .await;
    let restored = slot.restore(access).await;
    let capture = bulk::finish(capture, restored);
    let restored = borrow.restore(access, &slot).await;
    bulk::finish(capture, restored)
}

struct Reader<'a> {
    flash: &'a FirmwareFlash,
    descriptor_address: u64,
    descriptor: Vec<u8>,
}

impl Reader<'_> {
    /// One stock read of `size` flash bytes at window offset `offset`.
    async fn read(
        &self,
        access: &mut bulk::Access<'_>,
        borrow: &mut Borrow,
        slot: &mut Slot,
        offset: u64,
        size: u64,
    ) -> bulk::Result<Vec<u8>> {
        let source = self
            .flash
            .window
            .address
            .checked_add(offset)
            .ok_or_else(|| BulkError::Check("XP4 flash source overflow".into()))?;
        require(
            size != 0
                && size <= CHUNK_CAPACITY
                && source.is_multiple_of(READ_ALIGNMENT)
                && size.is_multiple_of(READ_ALIGNMENT)
                && offset + size <= self.flash.window.size - TAIL_BYTES,
            "XP4 stock request is outside its aligned transfer",
        )?;
        let destination = borrow.base + DESTINATION;
        let a32 = |value: u64| {
            u32::try_from(value)
                .map_err(|_| BulkError::Check("XP4 stock request exceeds A32".into()))
        };
        let mut request = Vec::with_capacity(REQUEST_BYTES as usize);
        for value in [destination, source, size] {
            request.extend(a32(value)?.to_le_bytes());
        }
        // Polled completion (wait 0) with the reader's cache maintenance (1).
        request.extend([0, 1, 0, 0]);
        borrow.request = Some(request.clone());
        write_verified(access, borrow.base, &request).await?;
        require(
            access.raw(self.descriptor_address, 8).await? == self.descriptor,
            "The registered diagnostic buffer changed",
        )?;
        slot.publish(access).await?;
        let reply = access
            .session
            .diagnostic_query(DiagnosticTag::Research(a32(borrow.base)?))
            .await;
        let restored = slot.restore(access).await;
        let reply = reply?;
        restored?;
        require(
            reply.status == CommandStatus::Success,
            "The stock flash reader returned an error",
        )?;
        access.raw(destination, size).await
    }
}

async fn check_code(access: &mut bulk::Access<'_>, checks: &[CodeCheck]) -> bulk::Result<()> {
    for check in checks {
        let bytes = access.raw(check.address, check.size).await?;
        require(
            bulk::hex(&sha256_bytes(&bytes)) == check.sha256,
            format!("Stock code differs at {:#x}", check.address),
        )?;
    }
    Ok(())
}

async fn write_verified(
    access: &mut bulk::Access<'_>,
    address: u64,
    bytes: &[u8],
) -> bulk::Result<()> {
    access.write(address, bytes).await?;
    require(
        access.raw(address, bytes.len() as u64).await? == bytes,
        "RAM write readback differs",
    )
}

/// The query callback slot, pointing at the stock reader only during a query.
struct Slot {
    address: u64,
    original: u32,
    reader: u32,
    published: bool,
}

impl Slot {
    async fn publish(&mut self, access: &mut bulk::Access<'_>) -> bulk::Result<()> {
        require(
            access.raw(self.address, 4).await? == self.original.to_le_bytes(),
            "XP4 query callback differs from its stock value",
        )?;
        self.published = true;
        write_verified(access, self.address, &self.reader.to_le_bytes()).await
    }

    async fn restore(&mut self, access: &mut bulk::Access<'_>) -> bulk::Result<()> {
        if !self.published {
            return Ok(());
        }
        let cleanup = async {
            require(
                access.session.state() == SessionState::Ready,
                "Completion unknown; XP4 callback restoration withheld",
            )?;
            let current = access.raw(self.address, 4).await?;
            require(
                current == self.reader.to_le_bytes() || current == self.original.to_le_bytes(),
                "XP4 callback ownership changed; restoration withheld",
            )?;
            write_verified(access, self.address, &self.original.to_le_bytes()).await
        }
        .await;
        match cleanup {
            Ok(()) => {
                self.published = false;
                Ok(())
            }
            Err(error) => {
                access.session.invalidate_completion();
                Err(error)
            }
        }
    }
}

/// The borrowed start of the registered diagnostic buffer and its saved bytes.
struct Borrow {
    base: u64,
    saved: Vec<u8>,
    /// The last request written, when any.
    request: Option<Vec<u8>>,
}

impl Borrow {
    async fn restore(&self, access: &mut bulk::Access<'_>, slot: &Slot) -> bulk::Result<()> {
        let Some(request) = &self.request else {
            return Ok(());
        };
        let cleanup = async {
            require(
                access.session.state() == SessionState::Ready && !slot.published,
                "Completion unknown; diagnostic buffer restoration withheld",
            )?;
            let current = access.raw(self.base, DESTINATION).await?;
            require(
                current[..REQUEST_BYTES as usize] == request[..]
                    && current[REQUEST_BYTES as usize..]
                        == self.saved[REQUEST_BYTES as usize..DESTINATION as usize],
                "Diagnostic buffer ownership changed; restoration withheld",
            )?;
            write_verified(access, self.base, &self.saved).await
        }
        .await;
        if cleanup.is_err() {
            access.session.invalidate_completion();
        }
        cleanup
    }
}

#[cfg(test)]
#[path = "xp4_stock_tests.rs"]
mod tests;
