//! Whole-flash backup through a stock reader and a temporary RAM payload.
use crate::callback::{CallbackLease, CodeCheck, Range, Transport};
use crate::{
    Limits,
    bulk::{self, BulkError, Qualified},
    callback::{self, Arena, require, word},
};
use fujimod_catalog::capture::{FirmwareFlash, FlashDevice};
use fujimod_catalog_bulk::{BulkRange, BulkSource, FirmwareBulk};
use fujimod_usb::{
    ff80::{Session, SessionState, commands::configuration::ConfigurationView},
    operations::{checks::sha256_bytes, configuration},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Metadata shipped beside the compiled flash-reader image. Addresses and code
/// hashes are produced by the mod build; the catalog supplies flash policy.
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
    pub mutable: Vec<Range>,
    pub transport: Transport,
    pub checks: Vec<CodeCheck>,
    pub control_sha256: String,
    pub update_size: u64,
}
#[derive(Debug)]
pub struct Backup {
    pub device: FlashDevice,
    pub size: u64,
    pub sha256: String,
    /// Physical page offsets covered by an uncorrectable NAND read.
    pub uncorrectable_page_offsets: Vec<u64>,
    /// Largest ECC observation span; completed bursts may flag multiple pages.
    pub uncorrectable_granularity_bytes: u32,
    pub nand_read_stats: Option<super::NandReadStats>,
}

impl Payload {
    /// Validate every bound before the first USB command.
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
            "Flash payload and catalog firmware differ",
        )?;
        require(
            flash.transport.as_deref() == Some("xp5-cmd-callback"),
            format!("Unsupported flash transport {:?}", flash.transport),
        )?;
        require(
            self.sha256 == bulk::hex(&sha256_bytes(image)),
            "Flash payload hash differs",
        )?;
        require(
            !image.is_empty() && image.len() as u64 <= self.capacity && self.capacity <= 0x10000,
            "Flash payload exceeds the executable reservation",
        )?;
        let image_end = self
            .address
            .checked_add(image.len() as u64)
            .ok_or(BulkError::Check("Payload address overflow".into()))?;
        let in_image = |at: u64, n: u64| {
            n != 0 && at >= self.address && at.checked_add(n).is_some_and(|end| end <= image_end)
        };
        let mutable = |at: u64, n: u64| {
            self.mutable.iter().any(|r| {
                at >= r.address
                    && at
                        .checked_add(n)
                        .is_some_and(|end| end <= r.address + r.size)
            })
        };
        for r in &self.mutable {
            require(
                in_image(r.address, r.size),
                "Mutable object exceeds payload",
            )?;
        }
        require(
            in_image(self.entry, 4)
                && !mutable(self.entry, 4)
                && in_image(self.request, 24)
                && mutable(self.request, 24)
                && in_image(self.result, 40)
                && mutable(self.result, 40)
                && in_image(self.buffer, u64::from(self.buffer_capacity))
                && mutable(self.buffer, u64::from(self.buffer_capacity)),
            "Flash reader ABI exceeds payload or mutable objects",
        )?;
        let overlaps =
            |a: u64, n: u64, b: u64, m: u64| a < b.saturating_add(m) && b < a.saturating_add(n);
        require(
            !self
                .mutable
                .iter()
                .any(|r| overlaps(self.entry, 4, r.address, r.size)),
            "Reader entry overlaps mutable data",
        )?;
        require(
            !overlaps(self.request, 24, self.result, 40)
                && !overlaps(
                    self.request,
                    24,
                    self.buffer,
                    u64::from(self.buffer_capacity),
                )
                && !overlaps(
                    self.result,
                    40,
                    self.buffer,
                    u64::from(self.buffer_capacity),
                ),
            "Flash ABI objects overlap",
        )?;
        require(
            self.transport.guard_offset != self.transport.debug_offset
                && u64::from(self.transport.guard_offset) < self.transport.configuration_bytes
                && u64::from(self.transport.debug_offset) < self.transport.configuration_bytes,
            "Invalid service configuration offsets",
        )?;
        require(
            self.transport.checks.iter().any(|c| {
                self.transport.instruction_cache_sync >= c.address
                    && self
                        .transport
                        .instruction_cache_sync
                        .checked_add(16)
                        .is_some_and(|end| end <= c.address + c.size)
            }),
            "Instruction cache entry has no pinned code check",
        )?;
        for c in self.checks.iter().chain(&self.transport.checks) {
            require(
                !overlaps(self.address, self.capacity, c.address, c.size),
                "Executable reservation overlaps stock code",
            )?;
        }
        require(
            !overlaps(self.address, self.capacity, self.transport.callback, 8),
            "Executable reservation overlaps callback",
        )?;
        let reader = flash
            .reader
            .as_ref()
            .ok_or_else(|| BulkError::Check("XP5 stock flash reader is absent".into()))?;
        let alignment = u64::from(reader.alignment);
        require(
            self.buffer.is_multiple_of(alignment)
                && self.buffer_capacity != 0
                && self.buffer_capacity.is_multiple_of(4096)
                && u64::from(self.buffer_capacity).is_multiple_of(alignment)
                && flash.control.size <= u64::from(self.buffer_capacity)
                && flash.control.offset + flash.control.size <= self.update_size
                && self.update_size <= flash.window.size,
            "Flash buffer or update control geometry differs",
        )?;
        require(
            self.stock_reader_symbol == reader.symbol
                && flash
                    .code_ranges
                    .iter()
                    .any(|c| c.name == reader.symbol && c.address == self.stock_reader),
            "Payload does not call the catalog stock reader",
        )?;
        for c in &flash.code_ranges {
            require(
                self.checks
                    .iter()
                    .any(|p| p.address == c.address && p.size == c.size),
                "Flash driver code check is missing",
            )?;
        }
        require(
            valid_hash(&self.control_sha256)
                && self.control_sha256 == flash.control.sha256
                && self
                    .checks
                    .iter()
                    .chain(&self.transport.checks)
                    .all(|c| valid_hash(&c.sha256)),
            "Invalid payload code or control hash",
        )?;
        for r in [
            Range {
                address: self.address,
                size: self.capacity,
            },
            self.transport.registered_scratch.clone(),
        ] {
            bulk::memory_range(BulkRange {
                address: r.address,
                size: r.size,
            })?;
        }
        for c in self.checks.iter().chain(&self.transport.checks) {
            bulk::memory_range(BulkRange {
                address: c.address,
                size: c.size,
            })?;
        }
        for c in &self.transport.words {
            require(
                matches!(c.size, 1 | 2 | 4 | 8),
                "Invalid transport state width",
            )?;
            bulk::memory_range(BulkRange {
                address: c.address,
                size: c.size,
            })?;
        }
        require(
            self.transport.callback.is_multiple_of(8)
                && self.transport.instruction_cache_sync.is_multiple_of(4),
            "Unaligned callback or cache entry",
        )?;
        bulk::memory_range(BulkRange {
            address: self.transport.callback,
            size: 8,
        })?;
        require(
            matches!(bulk.source,BulkSource::PatternZeroBase{capacity,configuration_pointer,configuration_bytes,..}
                if u64::from(capacity) == self.transport.registered_scratch.size
                    && configuration_pointer == self.transport.configuration_pointer
                    && configuration_bytes == self.transport.configuration_bytes),
            "Flash transport requires the catalog zero-base bulk source",
        )?;
        Ok(())
    }
}
fn valid_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Deliver chunks in camera-only-first order. The final SHA-256 covers the
/// complete image in flash-offset order. A sink failure still runs cleanup.
pub(crate) async fn read(
    session: &mut Session,
    bulk: &FirmwareBulk,
    flash: &FirmwareFlash,
    payload: &Payload,
    image: &[u8],
    limits: Limits,
    selection: super::Selection,
    callbacks: &mut super::Callbacks<'_>,
) -> bulk::Result<Backup> {
    payload.validate(flash, bulk, image)?;
    let target = selection.range(flash);
    (callbacks.progress)(0, target.size).map_err(BulkError::Check)?;
    // The built callback profile explicitly authorizes its empty arena and
    // registered diagnostic scratch, even when the bulk catalog only has a
    // bounded scouting window. No other addresses are inferred from capacity.
    let mut extended = bulk.clone();
    let mut start = bulk
        .window
        .address
        .min(payload.address)
        .min(payload.transport.registered_scratch.address);
    let mut end = (bulk.window.address + bulk.window.size)
        .max(payload.address + payload.capacity)
        .max(
            payload.transport.registered_scratch.address
                + payload.transport.registered_scratch.size,
        );
    if let Some(window) = &flash.e1_window {
        start = start.min(window.address);
        end = end.max(window.address + window.size);
    }
    extended.window = BulkRange {
        address: start,
        size: end - start,
    };
    bulk::memory_range(extended.window)?;
    let bulk = &extended;
    bulk::check_identity(session, bulk).await?;
    let transport = &payload.transport;
    require(
        configuration::read_byte(
            session,
            ConfigurationView::Ram,
            transport.guard_offset,
            limits.configuration,
        )
        .await?
            == 1,
        "Existing reset guard must be 1",
    )?;
    let original_gate = configuration::read_byte(
        session,
        ConfigurationView::Ram,
        transport.debug_offset,
        limits.configuration,
    )
    .await?;
    require(original_gate <= 1, "Unexpected debug byte")?;
    let outcome = async {
        if original_gate == 0 {
            configuration::write_byte_verified(
                session,
                ConfigurationView::Ram,
                transport.debug_offset,
                1,
                limits.configuration,
            )
            .await?;
        }
        let mut access = bulk::Access { session, limits };
        let BulkSource::PatternZeroBase {
            registered_buffer,
            source_state,
            ..
        } = bulk.source
        else {
            unreachable!("validated source")
        };
        let descriptor = access.raw(registered_buffer, 12).await?;
        require(
            word(&descriptor[..8]) == transport.registered_scratch.address
                && word(&descriptor[8..]) >= transport.registered_scratch.size,
            "Live diagnostic scratch differs from payload placement",
        )?;
        let configuration = word(&access.raw(transport.configuration_pointer, 8).await?);
        require(configuration != 0, "Invalid active configuration pointer")?;
        bulk::memory_range(BulkRange {
            address: configuration,
            size: transport.configuration_bytes,
        })?;
        let scratch_range = &transport.registered_scratch;
        let overlaps = |at: u64, n: u64| {
            scratch_range.address < at.saturating_add(n)
                && at < scratch_range.address + scratch_range.size
        };
        require(
            !overlaps(payload.address, payload.capacity)
                && !(payload.address < configuration + transport.configuration_bytes
                    && configuration < payload.address + payload.capacity)
                && !(payload.address < source_state + 12
                    && source_state < payload.address + payload.capacity)
                && !(payload.address < registered_buffer + 12
                    && registered_buffer < payload.address + payload.capacity)
                && !overlaps(transport.callback, 8)
                && !overlaps(configuration, transport.configuration_bytes)
                && !overlaps(registered_buffer, 12)
                && !overlaps(source_state, 12)
                && !overlaps(transport.configuration_pointer, 8)
                && !transport.words.iter().any(|c| overlaps(c.address, c.size))
                && !transport
                    .checks
                    .iter()
                    .chain(&payload.checks)
                    .any(|c| overlaps(c.address, c.size))
                && !bulk.code_checks.iter().any(|c| overlaps(c.address, c.size)),
            "Diagnostic scratch overlaps protected state or code",
        )?;
        let mut reader = Qualified::prepare_preserving(access.session, bulk, limits).await?;
        let outcome = if flash.e1_window.is_some() {
            if mapped_control_matches(&mut reader, flash, payload).await? {
                read_mapped(&mut reader, flash, target, callbacks).await
            } else {
                execute(
                    &mut reader,
                    flash,
                    payload,
                    image,
                    limits,
                    target,
                    callbacks,
                )
                .await
            }
        } else {
            execute(
                &mut reader,
                flash,
                payload,
                image,
                limits,
                target,
                callbacks,
            )
            .await
        };
        let restored = reader.restore().await;
        bulk::finish(outcome, restored)
    }
    .await;
    let cleanup = async {
        require(
            session.state() == SessionState::Ready,
            "Completion unknown; flash cleanup withheld",
        )?;
        require(
            configuration::read_byte(
                session,
                ConfigurationView::Ram,
                transport.guard_offset,
                limits.configuration,
            )
            .await?
                == 1,
            "Reset guard changed during flash backup",
        )?;
        let current = configuration::read_byte(
            session,
            ConfigurationView::Ram,
            transport.debug_offset,
            limits.configuration,
        )
        .await?;
        require(
            current == 1 || current == original_gate,
            "Debug gate ownership changed; restoration withheld",
        )?;
        if current != original_gate {
            configuration::write_byte_verified(
                session,
                ConfigurationView::Ram,
                transport.debug_offset,
                original_gate,
                limits.configuration,
            )
            .await?;
        }
        Ok(())
    }
    .await;
    if cleanup.is_err() {
        session.invalidate_completion();
    }
    bulk::finish(outcome, cleanup)
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
    require(
        size != 0 && address >= aperture.address && end <= aperture_end,
        "Flash range is outside the direct E1 window",
    )?;
    let range = BulkRange { address, size };
    bulk::memory_range(range)?;
    Ok(range)
}

/// Try the update-backed control extent first. A completed E1 mismatch or
/// refusal selects the stock-reader path; unknown USB completion stops here.
async fn mapped_control_matches(
    reader: &mut Qualified<'_>,
    flash: &FirmwareFlash,
    payload: &Payload,
) -> bulk::Result<bool> {
    let range = mapped_range(flash, flash.control.offset, flash.control.size)?;
    match reader.read(range).await {
        Ok(bytes) => Ok(bulk::hex(&sha256_bytes(&bytes)) == payload.control_sha256),
        Err(_error) if reader.access.session.state() == SessionState::Ready => Ok(false),
        Err(error) => Err(error),
    }
}

/// Capture mapped NOR through E1 after the control bytes match the update.
/// Each caller-owned chunk is bounded by the live registration and aligned to
/// the stock reader's source alignment.
async fn read_mapped(
    reader: &mut Qualified<'_>,
    flash: &FirmwareFlash,
    target: BulkRange,
    callbacks: &mut super::Callbacks<'_>,
) -> bulk::Result<Backup> {
    let alignment = u64::from(flash.reader.as_ref().map_or(1, |reader| reader.alignment));
    let capacity = reader
        .capacity()
        .min(reader.access.session.max_reply_bytes().get());
    let piece = u64::from(capacity) / alignment * alignment;
    require(
        piece != 0,
        "Direct E1 flash capacity is below reader alignment",
    )?;
    let end = target
        .address
        .checked_add(target.size)
        .ok_or_else(|| BulkError::Check("Flash range overflow".into()))?;
    let mut digest = OrderedDigest::default();
    let mut done = 0;
    let mut offset = target.address;
    while offset < end {
        let size = piece.min(end - offset);
        let range = mapped_range(flash, offset, size)?;
        let bytes = reader.read(range).await?;
        require(
            bytes.len() as u64 == size,
            "Direct E1 flash read returned another length",
        )?;
        digest.push(offset - target.address, &bytes);
        (callbacks.chunk)(offset, bytes).map_err(BulkError::Check)?;
        done += size;
        (callbacks.progress)(done, target.size).map_err(BulkError::Check)?;
        offset += size;
    }
    let sha256 = digest.finish(target.size)?;
    Ok(Backup {
        device: flash.device.clone(),
        size: target.size,
        sha256,
        uncorrectable_page_offsets: Vec::new(),
        uncorrectable_granularity_bytes: 0,
        nand_read_stats: None,
    })
}

async fn execute(
    reader: &mut Qualified<'_>,
    flash: &FirmwareFlash,
    payload: &Payload,
    image: &[u8],
    limits: Limits,
    target: BulkRange,
    callbacks: &mut super::Callbacks<'_>,
) -> bulk::Result<Backup> {
    callback::check_code(reader, &payload.transport.checks).await?;
    callback::check_code(reader, &payload.checks).await?;
    for c in &payload.transport.words {
        require(
            word(&reader.access.raw(c.address, c.size).await?) == c.value,
            "Callback transport state differs",
        )?;
    }
    for c in &flash.state_checks {
        require(
            word(&reader.access.raw(c.address, u64::from(c.size)).await?) == c.value,
            format!("Flash driver state differs: {}", c.name),
        )?;
    }
    let mut arena = Arena {
        address: payload.address,
        capacity: payload.capacity,
        image,
        mutable: &payload.mutable,
        uploaded: false,
        touched: false,
    };
    let outcome = async {
        require(
            reader.access.raw(payload.transport.callback, 8).await? == [0; 8],
            "Service callback is occupied",
        )?;
        arena.upload(reader, limits).await?;
        callback::invoke(
            reader,
            payload.transport.callback,
            payload.transport.instruction_cache_sync,
            false,
        )
        .await?;
        let mut lease = CallbackLease::new(payload.transport.callback, 0, payload.entry, 8);
        let capture = async {
            lease.publish(reader).await?;
            let mut calls = word(&reader.access.raw(payload.result + 16, 8).await?);
            let control = read_chunk(
                reader,
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
                let bytes =
                    read_chunk(reader, flash, payload, offset, size, &mut calls, &lease).await?;
                digest.push(offset - target.address, &bytes);
                (callbacks.chunk)(offset, bytes).map_err(BulkError::Check)?;
                done += size;
                (callbacks.progress)(done, target.size).map_err(BulkError::Check)?;
            }
            require(
                digest.next == target.size && digest.pending.is_empty(),
                "Flash coverage is incomplete",
            )?;
            Ok(Backup {
                device: flash.device.clone(),
                size: target.size,
                sha256: bulk::hex(&digest.hash.finalize()),
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
    let restored = arena.restore(reader, payload.transport.callback).await;
    if restored.is_err() {
        reader.access.session.invalidate_completion();
    }
    bulk::finish(outcome, restored)
}
async fn read_chunk(
    reader: &mut Qualified<'_>,
    flash: &FirmwareFlash,
    payload: &Payload,
    offset: u64,
    size: u64,
    calls: &mut u64,
    lease: &CallbackLease,
) -> bulk::Result<Vec<u8>> {
    let source = flash.window.address + offset;
    let request: Vec<u8> = [1, source, size]
        .into_iter()
        .flat_map(u64::to_le_bytes)
        .collect();
    callback::write(reader, payload.request, &request).await?;
    lease.check(reader).await?;
    callback::execute(reader, true).await?;
    let result = reader.access.raw(payload.result, 40).await?;
    require(
        word(&result[..8]) == 1
            && calls.checked_add(1) == Some(word(&result[16..24]))
            && word(&result[24..32]) == source
            && word(&result[32..40]) == size,
        "Flash reader result does not describe this request",
    )?;
    *calls += 1;
    require(
        word(&result[8..16]) == 0,
        format!("Stock flash read returned {}", word(&result[8..16]) as i64),
    )?;
    reader
        .read(BulkRange {
            address: payload.buffer,
            size,
        })
        .await
}
pub(crate) fn pieces(size: u64, capacity: u64, update_size: u64) -> Vec<(u64, u64)> {
    let chunks: Vec<_> = (0..size)
        .step_by(capacity as usize)
        .map(|offset| (offset, capacity.min(size - offset)))
        .collect();
    chunks
        .iter()
        .copied()
        .filter(|&(o, n)| o + n > update_size)
        .chain(
            chunks
                .iter()
                .copied()
                .filter(|&(o, n)| o + n <= update_size),
        )
        .collect()
}
#[derive(Default)]
pub(super) struct OrderedDigest {
    hash: Sha256,
    next: u64,
    pending: BTreeMap<u64, Vec<u8>>,
}
impl OrderedDigest {
    pub(super) fn push(&mut self, offset: u64, bytes: &[u8]) {
        if offset == self.next {
            self.hash.update(bytes);
            self.next += bytes.len() as u64;
        } else {
            self.pending.insert(offset, bytes.to_vec());
        }
        while let Some(bytes) = self.pending.remove(&self.next) {
            self.hash.update(&bytes);
            self.next += bytes.len() as u64;
        }
    }
    pub(super) fn finish(self, size: u64) -> bulk::Result<String> {
        require(
            self.next == size && self.pending.is_empty(),
            "Flash coverage is incomplete",
        )?;
        Ok(bulk::hex(&self.hash.finalize()))
    }
}

#[cfg(test)]
#[path = "xp5_tests.rs"]
mod tests;
