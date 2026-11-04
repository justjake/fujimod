//! X-Processor Pro stock flash reads staged through the FF80 direct-query buffer.
use super::xp5::OrderedDigest;
use crate::callback::{self, CallbackLease, CodeCheck, Range, require, word};
use crate::{
    Limits,
    bulk::{self, BulkError, Qualified},
};
use fujimod_catalog::capture::FirmwareFlash;
use fujimod_catalog_bulk::{BulkRange, BulkSource, FirmwareBulk};
use fujimod_usb::{
    ff80::{
        Session, SessionState,
        commands::{
            diagnostic::CommandStatus,
            research::{CommandCode, Request},
        },
    },
    operations::checks::sha256_bytes,
};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;

const REQUEST_BYTES: u64 = 16;
const RESULT_BYTES: u64 = 24;
pub(super) const READ_ALIGNMENT: u64 = 32;

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
    pub buffer_capacity: u32,
    pub stock_reader: u64,
    pub stock_reader_symbol: String,
    pub mutable: Vec<Range>,
    pub transport: Callback,
    pub checks: Vec<CodeCheck>,
    pub control_sha256: String,
    pub update_size: u64,
}

/// The XPP service callback is a 32-bit slot with an existing stock owner.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Callback {
    pub address: u64,
    pub original: u32,
    pub checks: Vec<CodeCheck>,
}

impl Payload {
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
            "XPP payload and catalog firmware differ",
        )?;
        require(
            flash.transport.as_deref() == Some("xpp-cmd-callback"),
            format!("Unsupported XPP flash transport {:?}", flash.transport),
        )?;
        require(
            self.sha256 == bulk::hex(&sha256_bytes(image))
                && !image.is_empty()
                && image.len() as u64 <= self.capacity
                && self.capacity <= 0x10000,
            "XPP payload image identity or capacity differs",
        )?;
        let image_end = self
            .address
            .checked_add(image.len() as u64)
            .ok_or(BulkError::Check("XPP payload address overflow".into()))?;
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
                        .is_some_and(|end| end <= range.address + range.size)
            })
        };
        for range in &self.mutable {
            require(
                in_image(range.address, range.size),
                "Mutable XPP object exceeds payload",
            )?;
        }
        require(
            in_image(self.entry, 4)
                && !mutable(self.entry, 4)
                && in_image(self.request, REQUEST_BYTES)
                && mutable(self.request, REQUEST_BYTES)
                && in_image(self.result, RESULT_BYTES)
                && mutable(self.result, RESULT_BYTES),
            "XPP reader ABI exceeds payload or mutable objects",
        )?;
        let overlaps = |a: u64, size_a: u64, b: u64, size_b: u64| {
            a < b.saturating_add(size_b) && b < a.saturating_add(size_a)
        };
        require(
            !overlaps(self.entry, 4, self.request, REQUEST_BYTES)
                && !overlaps(self.entry, 4, self.result, RESULT_BYTES)
                && !overlaps(self.request, REQUEST_BYTES, self.result, RESULT_BYTES)
                && !self
                    .mutable
                    .iter()
                    .any(|range| overlaps(self.entry, 4, range.address, range.size))
                && !overlaps(self.address, self.capacity, self.transport.address, 4),
            "XPP executable, ABI, or callback ranges overlap",
        )?;
        require(
            self.transport.address.is_multiple_of(4)
                && self.transport.original != 0
                && self.transport.original.is_multiple_of(4),
            "Invalid XPP callback slot or stock pointer",
        )?;
        bulk::memory_range(BulkRange {
            address: self.address,
            size: self.capacity,
        })?;
        bulk::memory_range(BulkRange {
            address: self.transport.address,
            size: 4,
        })?;
        bulk::memory_range(BulkRange {
            address: self.request,
            size: REQUEST_BYTES,
        })?;
        bulk::memory_range(BulkRange {
            address: self.result,
            size: RESULT_BYTES,
        })?;
        for check in self.checks.iter().chain(&self.transport.checks) {
            require(
                check.size != 0
                    && !overlaps(self.address, self.capacity, check.address, check.size)
                    && check.sha256.len() == 64
                    && check.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "Invalid or overlapping XPP code check",
            )?;
            bulk::memory_range(BulkRange {
                address: check.address,
                size: check.size,
            })?;
        }
        let reader = flash
            .reader
            .as_ref()
            .ok_or_else(|| BulkError::Check("XPP stock flash reader is absent".into()))?;
        require(
            self.stock_reader_symbol == reader.symbol
                && flash
                    .code_ranges
                    .iter()
                    .any(|range| range.name == reader.symbol && range.address == self.stock_reader)
                && self.buffer_capacity as u64 == bulk.window.size
                && reader.alignment != 0
                && READ_ALIGNMENT.is_multiple_of(u64::from(reader.alignment))
                && self.buffer_capacity.is_multiple_of(READ_ALIGNMENT as u32)
                && bulk.window.address.is_multiple_of(READ_ALIGNMENT)
                && flash.control.size <= u64::from(self.buffer_capacity)
                && flash
                    .control
                    .offset
                    .checked_add(flash.control.size)
                    .is_some_and(|end| end <= self.update_size)
                && self.update_size <= flash.window.size,
            "XPP reader, transfer buffer, or update geometry differs",
        )?;
        match bulk.source {
            BulkSource::DirectQuery {
                base,
                data_offset,
                ref capacities,
                ..
            } => {
                require(
                    base.checked_add(u64::from(data_offset)) == Some(bulk.window.address)
                        && !capacities.is_empty()
                        && capacities.iter().any(|capacity| {
                            u64::from(*capacity) >= u64::from(data_offset) + bulk.window.size
                        }),
                    "XPP transfer window differs from its direct-query source",
                )?;
            }
            _ => {
                return Err(BulkError::Check(
                    "XPP reader requires a direct-query bulk source".into(),
                ));
            }
        }
        for range in &flash.code_ranges {
            require(
                self.checks
                    .iter()
                    .any(|check| check.address == range.address && check.size == range.size),
                "XPP flash driver code check is missing",
            )?;
        }
        require(
            self.control_sha256 == flash.control.sha256
                && self.control_sha256.len() == 64
                && self.transport.checks.iter().any(|check| {
                    check.address <= u64::from(self.transport.original)
                        && u64::from(self.transport.original) + 4 <= check.address + check.size
                }),
            "XPP control identity or callback code check differs",
        )?;
        Ok(())
    }
}

/// Read a whole flash or bounded aligned range with the stock XPP reader.
pub(crate) async fn read(
    session: &mut Session,
    bulk: &FirmwareBulk,
    flash: &FirmwareFlash,
    payload: &Payload,
    image: &[u8],
    limits: Limits,
    selection: super::Selection,
    callbacks: &mut super::Callbacks<'_>,
) -> bulk::Result<super::Backup> {
    payload.validate(flash, bulk, image)?;
    let target = selection.range(flash);
    require(
        bulk.window.contains(BulkRange {
            address: bulk.window.address,
            size: u64::from(payload.buffer_capacity),
        }),
        "XPP transfer buffer exceeds the E1 window",
    )?;
    (callbacks.progress)(0, target.size).map_err(BulkError::Check)?;
    let mut reader = Qualified::prepare(session, bulk, limits).await?;
    let outcome = execute(&mut reader, bulk, flash, payload, image, target, callbacks).await;
    let restored = reader.restore().await;
    bulk::finish(outcome, restored)
}

async fn execute(
    reader: &mut Qualified<'_>,
    bulk: &FirmwareBulk,
    flash: &FirmwareFlash,
    payload: &Payload,
    image: &[u8],
    target: BulkRange,
    callbacks: &mut super::Callbacks<'_>,
) -> bulk::Result<super::Backup> {
    callback::check_code(reader, &payload.transport.checks).await?;
    callback::check_code(reader, &payload.checks).await?;
    for check in &flash.state_checks {
        require(
            word(
                &reader
                    .access
                    .raw(check.address, u64::from(check.size))
                    .await?,
            ) == check.value,
            format!("Flash driver state differs: {}", check.name),
        )?;
    }
    let mut reservation = Reservation {
        address: payload.address,
        capacity: payload.capacity,
        image,
        mutable: &payload.mutable,
        touched: false,
        uploaded: false,
    };
    let outcome = async {
        let original = payload.transport.original.to_le_bytes();
        require(
            reader.access.raw(payload.transport.address, 4).await? == original,
            "XPP service callback differs from its stock value",
        )?;
        reservation.upload(reader).await?;
        let mut lease = CallbackLease::new(
            payload.transport.address,
            u64::from(payload.transport.original),
            payload.entry,
            4,
        );
        let capture = async {
            lease.publish(reader).await?;
            let mut calls = u64::from(u32::from_le_bytes(
                reader
                    .access
                    .raw(payload.result + 8, 4)
                    .await?
                    .try_into()
                    .expect("four bytes"),
            ));
            let control = read_chunk(
                reader,
                bulk,
                flash,
                payload,
                flash.control.offset,
                flash.control.size,
                &mut calls,
                &lease,
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
                u64::from(payload.buffer_capacity),
                payload.update_size,
                flash.window.size,
            ) {
                let bytes = read_chunk(
                    reader, bulk, flash, payload, offset, size, &mut calls, &lease,
                )
                .await?;
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
        let restored = lease.restore(reader).await;
        bulk::finish(capture, restored)
    }
    .await;
    let restored = reservation
        .restore(
            reader,
            payload.transport.address,
            payload.transport.original,
        )
        .await;
    if restored.is_err() {
        reader.access.session.invalidate_completion();
    }
    bulk::finish(outcome, restored)
}

async fn read_chunk(
    reader: &mut Qualified<'_>,
    bulk: &FirmwareBulk,
    flash: &FirmwareFlash,
    payload: &Payload,
    offset: u64,
    size: u64,
    calls: &mut u64,
    lease: &CallbackLease,
) -> bulk::Result<Vec<u8>> {
    let alignment = READ_ALIGNMENT;
    let source = flash
        .window
        .address
        .checked_add(offset)
        .ok_or_else(|| BulkError::Check("XPP flash source overflow".into()))?;
    let destination = bulk.window.address;
    require(
        size != 0
            && size <= u64::from(payload.buffer_capacity)
            && source.is_multiple_of(alignment)
            && size.is_multiple_of(alignment)
            && destination.is_multiple_of(alignment),
        "XPP flash request is outside its aligned transfer buffer",
    )?;
    let request = [
        1u32,
        u32::try_from(source)
            .map_err(|_| BulkError::Check("XPP flash address exceeds A32".into()))?,
        u32::try_from(size)
            .map_err(|_| BulkError::Check("XPP flash request is too large".into()))?,
        u32::try_from(destination)
            .map_err(|_| BulkError::Check("XPP destination exceeds A32".into()))?,
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect::<Vec<_>>();
    callback::write(reader, payload.request, &request).await?;
    lease.check(reader).await?;
    execute_command(reader).await?;
    let result = reader.access.raw(payload.result, RESULT_BYTES).await?;
    let result_u32 =
        |at: usize| u32::from_le_bytes(result[at..at + 4].try_into().expect("four bytes"));
    require(
        result_u32(0) == 1
            && u64::from(result_u32(8))
                == calls
                    .checked_add(1)
                    .ok_or_else(|| BulkError::Check("XPP call counter overflow".into()))?
            && u64::from(result_u32(12)) == source
            && u64::from(result_u32(16)) == size
            && u64::from(result_u32(20)) == destination,
        "XPP reader result does not describe this request",
    )?;
    *calls += 1;
    let status = i32::from_le_bytes(result[4..8].try_into().expect("four bytes"));
    require(
        status == 0,
        format!("Stock XPP flash read returned {status}"),
    )?;
    reader
        .read(BulkRange {
            address: destination,
            size,
        })
        .await
}

async fn execute_command(reader: &mut Qualified<'_>) -> bulk::Result<()> {
    let text = b"CMD PROBE\r\n";
    let body: Vec<u8> = [5u32, 0, 0, 0, text.len() as u32]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .chain(text.iter().copied())
        .collect();
    let request = Request::new(CommandCode(0x200001), body, NonZeroU32::new(256).unwrap())?;
    let reply = reader.access.session.research_command(&request).await?;
    require(
        reply.body().len() >= 4 && word(&reply.body()[..4]) == 5,
        "XPP callback operation echo differs",
    )?;
    require(
        reply.status == CommandStatus::Success,
        "XPP stock callback returned an error",
    )
}

struct Reservation<'a> {
    address: u64,
    capacity: u64,
    image: &'a [u8],
    mutable: &'a [Range],
    touched: bool,
    uploaded: bool,
}

impl Reservation<'_> {
    async fn upload(&mut self, reader: &mut Qualified<'_>) -> bulk::Result<()> {
        let read_size = reader.access.limits.memory.read.get();
        let mut offset = 0u64;
        while offset < self.capacity {
            let size = u64::from(read_size).min(self.capacity - offset);
            require(
                reader
                    .access
                    .raw(self.address + offset, size)
                    .await?
                    .iter()
                    .all(|byte| *byte == 0),
                "XPP executable reservation is occupied",
            )?;
            offset += size;
        }
        self.touched = true;
        let write_size = reader.access.limits.memory.write.get() as usize;
        for (index, piece) in self.image.chunks(write_size).enumerate() {
            callback::write(reader, self.address + (index * write_size) as u64, piece).await?;
        }
        let mut offset = 0usize;
        while offset < self.image.len() {
            let size = (read_size as usize).min(self.image.len() - offset);
            require(
                reader
                    .access
                    .raw(self.address + offset as u64, size as u64)
                    .await?
                    == self.image[offset..offset + size],
                "XPP executable upload readback differs",
            )?;
            offset += size;
        }
        self.uploaded = true;
        Ok(())
    }

    async fn restore(
        &self,
        reader: &mut Qualified<'_>,
        callback_address: u64,
        original_callback: u32,
    ) -> bulk::Result<()> {
        if !self.touched {
            return Ok(());
        }
        require(
            reader.access.session.state() == SessionState::Ready,
            "Completion unknown; XPP executable reservation retained",
        )?;
        require(
            reader.access.raw(callback_address, 4).await? == original_callback.to_le_bytes(),
            "XPP callback remains published; executable reservation retained",
        )?;
        // The stock callback is restored by CallbackLease before this check.
        let observed = read_all(reader, self.address, self.capacity).await?;
        require(
            callback::owns_image(
                &observed,
                self.image,
                usize::try_from(self.capacity)
                    .map_err(|_| BulkError::Check("XPP reservation exceeds host size".into()))?,
                self.mutable,
                self.address,
                self.uploaded,
            ),
            "XPP executable reservation ownership changed; restoration withheld",
        )?;
        write_all(reader, self.address, self.image.len() as u64, 0).await?;
        require(
            read_all(reader, self.address, self.capacity)
                .await?
                .iter()
                .all(|byte| *byte == 0),
            "XPP executable reservation restoration differs",
        )
    }
}

async fn read_all(reader: &mut Qualified<'_>, address: u64, size: u64) -> bulk::Result<Vec<u8>> {
    let piece = u64::from(reader.access.limits.memory.read.get());
    let mut bytes = Vec::with_capacity(
        usize::try_from(size)
            .map_err(|_| BulkError::Check("XPP range exceeds host size".into()))?,
    );
    let mut offset = 0u64;
    while offset < size {
        let count = piece.min(size - offset);
        bytes.extend(reader.access.raw(address + offset, count).await?);
        offset += count;
    }
    Ok(bytes)
}

async fn write_all(
    reader: &mut Qualified<'_>,
    address: u64,
    size: u64,
    value: u8,
) -> bulk::Result<()> {
    let piece = reader.access.limits.memory.write.get() as usize;
    let zeros = vec![value; piece];
    let mut offset = 0u64;
    while offset < size {
        let count = (size - offset).min(piece as u64) as usize;
        callback::write(reader, address + offset, &zeros[..count]).await?;
        offset += count as u64;
    }
    Ok(())
}
