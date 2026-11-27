//! X-Processor 4 stock flash reads staged through owned RAM and the
//! `0x450006` diagnostic query callback.
use super::xp5::OrderedDigest;
use crate::{
    Limits,
    bulk::{self, BulkError},
    callback::{CodeCheck, Range, owns_image, require, word},
};
use fujimod_catalog::capture::FirmwareFlash;
use fujimod_catalog_bulk::FirmwareBulk;
use fujimod_usb::{
    ff80::{
        Session, SessionState,
        commands::diagnostic::{CommandStatus, DiagnosticTag},
    },
    operations::checks::sha256_bytes,
};
use serde::{Deserialize, Serialize};

const REQUEST_BYTES: u64 = 16;
const RESULT_BYTES: u64 = 24;
pub(super) const READ_ALIGNMENT: u64 = 32;
/// The largest control read and the largest owned-RAM transfer piece.
const CHUNK_CAPACITY: u64 = 0x8000;
/// The stock diagnostic query that invokes the published callback.
pub const QUERY_COMMAND: u32 = 0x450006;
/// Marker field returned by the query and echoed through the result object.
pub const MAGIC: u32 = 0x5058_3401;

/// Metadata beside the compiled A32 callback helper.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub schema: u32,
    pub firmware_id: String,
    pub firmware_sha256: String,
    pub address: u64,
    pub capacity: u64,
    pub sha256: String,
    pub entry: u64,
    pub request: u64,
    pub result: u64,
    pub buffer: u64,
    pub buffer_capacity: u32,
    pub stock_reader: u64,
    pub stock_reader_symbol: String,
    /// Research selector carried by the `0x450006` query request.
    pub tag: u32,
    pub mutable: Vec<Range>,
    pub transport: Callback,
    pub checks: Vec<CodeCheck>,
    pub control_sha256: String,
    pub update_size: u64,
}

/// The stock `0x450006` query callback: a 32-bit slot and its stock value.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Callback {
    pub address: u64,
    pub original: u32,
    pub checks: Vec<CodeCheck>,
}

impl Payload {
    /// Effective transfer bound; refused above `CHUNK_CAPACITY` during
    /// validation rather than silently clamped.
    pub fn chunk_capacity(&self) -> u64 {
        u64::from(self.buffer_capacity).min(CHUNK_CAPACITY)
    }

    /// Validate addresses and source geometry before the first USB command.
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
            "XP4 payload and catalog firmware differ",
        )?;
        require(
            flash.transport.as_deref() == Some("xp4-camera-code"),
            format!("Unsupported XP4 flash transport {:?}", flash.transport),
        )?;
        require(
            !image.is_empty()
                && image.len() as u64 <= self.capacity
                && self.capacity <= CHUNK_CAPACITY
                && self.sha256 == bulk::hex(&sha256_bytes(image)),
            "XP4 payload image identity or capacity differs",
        )?;
        require(
            valid_hash(&self.firmware_sha256) && valid_hash(&self.control_sha256),
            "XP4 payload hash fields differ",
        )?;
        let chunk = self.chunk_capacity();
        require(
            chunk == u64::from(self.buffer_capacity)
                && chunk != 0
                && chunk.is_multiple_of(READ_ALIGNMENT)
                && u64::from(self.buffer_capacity) >= flash.control.size
                && self.tag != 0,
            "XP4 chunk capacity exceeds its bound",
        )?;
        let image_end = self
            .address
            .checked_add(image.len() as u64)
            .ok_or(BulkError::Check("XP4 payload address overflow".into()))?;
        let in_image = |at: u64, size: u64| {
            size != 0
                && at >= self.address
                && at.checked_add(size).is_some_and(|end| end <= image_end)
        };
        let mutable = |at: u64, size: u64| {
            self.mutable.iter().any(|range| {
                at >= range.address
                    && at
                        .checked_add(size)
                        .is_some_and(|end| end <= range.address.saturating_add(range.size))
            })
        };
        require(
            self.mutable
                .iter()
                .all(|range| in_image(range.address, range.size)),
            "Mutable XP4 object exceeds payload",
        )?;
        require(
            in_image(self.entry, 4)
                && !mutable(self.entry, 4)
                && in_image(self.request, REQUEST_BYTES)
                && mutable(self.request, REQUEST_BYTES)
                && in_image(self.result, RESULT_BYTES)
                && mutable(self.result, RESULT_BYTES),
            "XP4 reader ABI exceeds payload or mutable objects",
        )?;
        let overlaps = |a: u64, size_a: u64, b: u64, size_b: u64| {
            a < b.saturating_add(size_b) && b < a.saturating_add(size_a)
        };
        let buffer = u64::from(self.buffer_capacity);
        require(
            !overlaps(self.entry, 4, self.request, REQUEST_BYTES)
                && !overlaps(self.entry, 4, self.result, RESULT_BYTES)
                && !overlaps(self.request, REQUEST_BYTES, self.result, RESULT_BYTES)
                && !self
                    .mutable
                    .iter()
                    .any(|range| overlaps(self.entry, 4, range.address, range.size))
                && !overlaps(self.address, self.capacity, self.transport.address, 4)
                && !overlaps(self.address, self.capacity, self.buffer, buffer)
                && !overlaps(self.buffer, buffer, self.transport.address, 4)
                && !overlaps(self.buffer, buffer, self.request, REQUEST_BYTES)
                && !overlaps(self.buffer, buffer, self.result, RESULT_BYTES),
            "XP4 executable, ABI, buffer, or callback ranges overlap",
        )?;
        require(
            self.transport.address.is_multiple_of(4)
                && self.transport.original.is_multiple_of(4)
                && self.buffer.is_multiple_of(READ_ALIGNMENT)
                && u64::from(self.buffer_capacity).is_multiple_of(READ_ALIGNMENT),
            "Invalid XP4 callback slot or transfer alignment",
        )?;
        bulk::memory_range(bulk_range(self.address, self.capacity))?;
        bulk::memory_range(bulk_range(self.transport.address, 4))?;
        bulk::memory_range(bulk_range(self.request, REQUEST_BYTES))?;
        bulk::memory_range(bulk_range(self.result, RESULT_BYTES))?;
        bulk::memory_range(bulk_range(self.buffer, buffer))?;
        for check in self.checks.iter().chain(&self.transport.checks) {
            require(
                check.size != 0
                    && !overlaps(self.address, self.capacity, check.address, check.size)
                    && !overlaps(self.buffer, buffer, check.address, check.size)
                    && valid_hash(&check.sha256),
                "Invalid or overlapping XP4 code check",
            )?;
            bulk::memory_range(bulk_range(check.address, check.size))?;
        }
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
                && reader.alignment != 0
                && READ_ALIGNMENT.is_multiple_of(u64::from(reader.alignment))
                && flash.window.address.is_multiple_of(READ_ALIGNMENT)
                && flash.control.size <= u64::from(self.buffer_capacity)
                && flash
                    .control
                    .offset
                    .checked_add(flash.control.size)
                    .is_some_and(|end| end <= self.update_size)
                && self.update_size <= flash.window.size,
            "XP4 reader, transfer buffer, or update geometry differs",
        )?;
        Ok(())
    }
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn bulk_range(address: u64, size: u64) -> fujimod_catalog_bulk::BulkRange {
    fujimod_catalog_bulk::BulkRange { address, size }
}

/// Read a whole flash or bounded aligned range with the stock XP4 reader.
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
    execute(&mut access, flash, payload, image, target, callbacks).await
}

async fn execute(
    access: &mut bulk::Access<'_>,
    flash: &FirmwareFlash,
    payload: &Payload,
    image: &[u8],
    target: fujimod_catalog_bulk::BulkRange,
    callbacks: &mut super::Callbacks<'_>,
) -> bulk::Result<super::Backup> {
    check_code(access, &payload.transport.checks).await?;
    check_code(access, &payload.checks).await?;
    for check in &flash.state_checks {
        require(
            word(&access.raw(check.address, u64::from(check.size)).await?) == check.value,
            format!("Flash driver state differs: {}", check.name),
        )?;
    }
    let mut reservation = Reservation {
        address: payload.address,
        capacity: payload.capacity,
        image,
        sha256: &payload.sha256,
        mutable: &payload.mutable,
        touched: false,
        uploaded: false,
    };
    let outcome = async {
        reservation.upload(access, payload.tag).await?;
        let mut slot = Slot {
            address: payload.transport.address,
            original: payload.transport.original,
            entry: u32::try_from(payload.entry)
                .map_err(|_| BulkError::Check("XP4 entry exceeds A32".into()))?,
            touched: false,
        };
        let capture = async {
            slot.publish(access).await?;
            let mut calls = 0u64;
            let control = read_chunk(
                access,
                flash,
                payload,
                flash.control.offset,
                flash.control.size,
                &mut calls,
                &slot,
            )
            .await?;
            require(
                bulk::hex(&sha256_bytes(&control)) == payload.control_sha256,
                "Control chunk differs from update; no backup read",
            )?;
            let mut digest = OrderedDigest::default();
            let mut done = 0;
            for (offset, size) in super::read_pieces(
                target,
                payload.chunk_capacity(),
                payload.update_size,
                flash.window.size,
            ) {
                let bytes =
                    read_chunk(access, flash, payload, offset, size, &mut calls, &slot).await?;
                digest.push(offset - target.address, &bytes);
                (callbacks.chunk)(offset, bytes).map_err(BulkError::Check)?;
                done += size;
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
        bulk::finish(capture, restored)
    }
    .await;
    let restored = reservation
        .restore(
            access,
            payload.transport.address,
            payload.transport.original,
        )
        .await;
    if restored.is_err() {
        access.session.invalidate_completion();
    }
    bulk::finish(outcome, restored)
}

async fn read_chunk(
    access: &mut bulk::Access<'_>,
    flash: &FirmwareFlash,
    payload: &Payload,
    offset: u64,
    size: u64,
    calls: &mut u64,
    slot: &Slot,
) -> bulk::Result<Vec<u8>> {
    let source = flash
        .window
        .address
        .checked_add(offset)
        .ok_or_else(|| BulkError::Check("XP4 flash source overflow".into()))?;
    require(
        size != 0
            && size <= payload.chunk_capacity()
            && source.is_multiple_of(READ_ALIGNMENT)
            && size.is_multiple_of(READ_ALIGNMENT)
            && payload.buffer.is_multiple_of(READ_ALIGNMENT),
        "XP4 flash request is outside its aligned transfer buffer",
    )?;
    let request = [
        1u32,
        u32::try_from(source)
            .map_err(|_| BulkError::Check("XP4 flash address exceeds A32".into()))?,
        u32::try_from(size)
            .map_err(|_| BulkError::Check("XP4 flash request is too large".into()))?,
        u32::try_from(payload.buffer)
            .map_err(|_| BulkError::Check("XP4 destination exceeds A32".into()))?,
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect::<Vec<_>>();
    write_verified(access, payload.request, &request).await?;
    slot.check(access).await?;
    query(access, payload.tag).await?;
    let result = access.raw(payload.result, RESULT_BYTES).await?;
    let at = |index: usize| word(&result[index..index + 4]);
    require(
        at(0) == 1
            && at(4) == 0
            && at(8) == *calls + 1
            && at(12) == source
            && at(16) == size
            && at(20) == u64::from(MAGIC),
        "XP4 reader result does not describe this request",
    )?;
    *calls += 1;
    read_all(access, payload.buffer, size).await
}

/// Send the stock `0x450006` 32-byte query and require its magic field.
///
/// The stock handler serializes the word the callback stores through its
/// `r2` argument at reply byte 20, which `DiagnosticReply` decodes as
/// `fields[1]`. On an X-T3 5.11 a helper that stored `MAGIC` there replied
/// with fields `[0, MAGIC, 0, 0]`.
async fn query(access: &mut bulk::Access<'_>, tag: u32) -> bulk::Result<()> {
    let reply = access
        .session
        .diagnostic_query(DiagnosticTag::Research(tag))
        .await?;
    require(
        reply.status == CommandStatus::Success,
        "XP4 query callback returned an error",
    )?;
    require(reply.fields[1] == MAGIC, "XP4 query reply magic differs")?;
    Ok(())
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

async fn read_all(access: &mut bulk::Access<'_>, address: u64, size: u64) -> bulk::Result<Vec<u8>> {
    let piece = u64::from(access.limits.memory.read.get());
    let mut bytes = Vec::with_capacity(
        usize::try_from(size)
            .map_err(|_| BulkError::Check("XP4 range exceeds host size".into()))?,
    );
    let mut offset = 0u64;
    while offset < size {
        let count = piece.min(size - offset);
        bytes.extend(access.raw(address + offset, count).await?);
        offset += count;
    }
    Ok(bytes)
}

async fn write_all(access: &mut bulk::Access<'_>, address: u64, size: u64) -> bulk::Result<()> {
    let piece = access.limits.memory.write.get() as usize;
    let zeros = vec![0u8; piece];
    let mut offset = 0u64;
    while offset < size {
        let count = (size - offset).min(piece as u64) as usize;
        write_verified(access, address + offset, &zeros[..count]).await?;
        offset += count as u64;
    }
    Ok(())
}

struct Slot {
    address: u64,
    original: u32,
    entry: u32,
    touched: bool,
}

impl Slot {
    async fn publish(&mut self, access: &mut bulk::Access<'_>) -> bulk::Result<()> {
        require(
            access.raw(self.address, 4).await? == self.original.to_le_bytes(),
            "XP4 service callback differs from its stock value",
        )?;
        self.touched = true;
        write_verified(access, self.address, &self.entry.to_le_bytes()).await
    }

    async fn check(&self, access: &mut bulk::Access<'_>) -> bulk::Result<()> {
        require(
            access.raw(self.address, 4).await? == self.entry.to_le_bytes(),
            "XP4 callback ownership changed",
        )
    }

    async fn restore(&self, access: &mut bulk::Access<'_>) -> bulk::Result<()> {
        if !self.touched {
            return Ok(());
        }
        let cleanup = async {
            require(
                access.session.state() == SessionState::Ready,
                "Completion unknown; XP4 callback restoration withheld",
            )?;
            let current = access.raw(self.address, 4).await?;
            require(
                current == self.entry.to_le_bytes() || current == self.original.to_le_bytes(),
                "XP4 callback ownership changed; restoration withheld",
            )?;
            write_verified(access, self.address, &self.original.to_le_bytes()).await
        }
        .await;
        if cleanup.is_err() {
            access.session.invalidate_completion();
        }
        cleanup
    }
}

struct Reservation<'a> {
    address: u64,
    capacity: u64,
    image: &'a [u8],
    sha256: &'a str,
    mutable: &'a [Range],
    touched: bool,
    uploaded: bool,
}

impl Reservation<'_> {
    async fn upload(&mut self, access: &mut bulk::Access<'_>, _tag: u32) -> bulk::Result<()> {
        let zeros = read_all(access, self.address, self.capacity).await?;
        require(
            zeros.iter().all(|&byte| byte == 0),
            "XP4 executable reservation is occupied",
        )?;
        self.touched = true;
        let write_size = access.limits.memory.write.get() as usize;
        for (index, piece) in self.image.chunks(write_size).enumerate() {
            access
                .write(self.address + (index * write_size) as u64, piece)
                .await?;
        }
        let observed = read_all(access, self.address, self.image.len() as u64).await?;
        require(
            observed == self.image && bulk::hex(&sha256_bytes(&observed)) == self.sha256,
            "DMA readback of uploaded XP4 code differs",
        )?;
        self.uploaded = true;
        Ok(())
    }

    async fn restore(
        &mut self,
        access: &mut bulk::Access<'_>,
        slot: u64,
        original: u32,
    ) -> bulk::Result<()> {
        if !self.touched {
            return Ok(());
        }
        let cleanup = async {
            require(
                access.session.state() == SessionState::Ready,
                "Completion unknown; XP4 executable reservation restoration withheld",
            )?;
            require(
                access.raw(slot, 4).await? == original.to_le_bytes(),
                "XP4 callback remains published; executable reservation retained",
            )?;
            let observed = read_all(access, self.address, self.capacity).await?;
            require(
                owns_image(
                    &observed,
                    self.image,
                    usize::try_from(self.capacity).map_err(|_| {
                        BulkError::Check("XP4 reservation exceeds host size".into())
                    })?,
                    self.mutable,
                    self.address,
                    self.uploaded,
                ),
                "XP4 executable reservation ownership changed; restoration withheld",
            )?;
            write_all(access, self.address, self.image.len() as u64).await?;
            require(
                read_all(access, self.address, self.capacity)
                    .await?
                    .iter()
                    .all(|&byte| byte == 0),
                "XP4 executable reservation restoration differs",
            )
        }
        .await;
        if cleanup.is_err() {
            access.session.invalidate_completion();
        }
        cleanup
    }
}

#[cfg(test)]
#[path = "xp4_tests.rs"]
mod tests;
