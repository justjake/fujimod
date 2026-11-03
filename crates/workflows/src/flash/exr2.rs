//! Corrected main-area NAND reads through an owned allocation and stock query callback.
use super::{Backup, CodeCheck};
use crate::{
    Limits,
    bulk::{self, BulkError, Qualified},
    callback::{CallbackLease, check_code, require, word, write},
};
use fujimod_catalog::capture::FirmwareFlash;
use fujimod_catalog_bulk::{BulkRange, BulkSource, FirmwareBulk};
use fujimod_usb::{
    ff80::{
        Session, SessionState,
        commands::{
            configuration::ConfigurationView,
            diagnostic::{CommandStatus, DiagnosticTag},
        },
    },
    operations::{checks::sha256_bytes, configuration},
};
use serde::{Deserialize, Serialize};

const REQUEST: u64 = 0x1000;
const BUFFER: u64 = 0x2000;
const CAPACITY: u64 = super::CAPTURE_CHUNK_BYTES as u64;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "mechanism", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Configuration {
    Pointer { address: u32 },
    Fixed { address: u32 },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub schema: u32,
    pub firmware_id: String,
    pub firmware_sha256: String,
    pub sha256: String,
    pub bootstrap_size: u32,
    pub reader_offset: u32,
    pub observer_offset: u32,
    pub allocate: u32,
    pub release: u32,
    pub configuration: Configuration,
    pub cache_clean: u32,
    pub instruction_invalidate: u32,
    pub barrier: u32,
    pub instruction_barrier: u32,
    pub checks: Vec<CodeCheck>,
    pub control_sha256: String,
    pub update_size: u64,
}
impl Payload {
    pub fn validate(
        &self,
        flash: &FirmwareFlash,
        bulk: &FirmwareBulk,
        image: &[u8],
    ) -> bulk::Result<()> {
        flash.validate().map_err(BulkError::Check)?;
        require(
            self.schema == 6
                && self.firmware_id == flash.firmware_id
                && self.firmware_id == bulk.firmware_id,
            "Flash payload and catalog firmware differ",
        )?;
        require(
            flash.device.kind == "raw-nand"
                && flash.reader.nand.is_some()
                && flash.transport.as_deref() == Some("exr2-query-heap"),
            "Unsupported EXR II flash transport",
        )?;
        require(
            matches!(bulk.source, BulkSource::Query { wrapping: true, .. }) && bulk.code_alias != 0,
            "EXR II requires a qualified wrapping query source",
        )?;
        require(
            self.bootstrap_size > 0
                && self.bootstrap_size <= 32
                && self.reader_offset == 32
                && self.observer_offset > self.reader_offset
                && self.observer_offset.is_multiple_of(4)
                && u64::from(self.observer_offset) + 4 == image.len() as u64
                && image[self.observer_offset as usize..] == [0x1e, 0xff, 0x2f, 0xe1]
                && image.len() > 32
                && image.len() <= REQUEST as usize
                && self.sha256 == bulk::hex(&sha256_bytes(image)),
            "EXR II reader image or ABI differs",
        )?;
        require(
            flash.window.size <= u64::from(u32::MAX)
                && flash.control.size <= CAPACITY
                && flash.control.offset + flash.control.size <= self.update_size
                && self.update_size <= flash.window.size,
            "NAND control geometry differs",
        )?;
        for at in [
            self.allocate,
            self.release,
            self.cache_clean,
            self.instruction_invalidate,
            self.barrier,
            self.instruction_barrier,
        ] {
            require(
                at != 0
                    && at.is_multiple_of(4)
                    && self
                        .checks
                        .iter()
                        .any(|r| u64::from(at) >= r.address && u64::from(at) < r.address + r.size),
                "Unpinned execution primitive",
            )?;
        }
        for r in &flash.code_ranges {
            require(
                self.checks
                    .iter()
                    .any(|c| c.address == r.address && c.size == r.size),
                "NAND code check missing",
            )?;
        }
        require(
            self.control_sha256.len() == 64 && self.checks.iter().all(|c| c.sha256.len() == 64),
            "Invalid execution hash",
        )?;
        for c in &self.checks {
            bulk::memory_range(BulkRange {
                address: c.address | bulk.code_alias,
                size: c.size,
            })?;
        }
        let configuration = match self.configuration {
            Configuration::Pointer { address } | Configuration::Fixed { address } => address,
        };
        require(
            configuration != 0 && configuration.is_multiple_of(4),
            "Invalid configuration location",
        )?;
        bulk::memory_range(BulkRange {
            address: u64::from(configuration) | bulk.code_alias,
            size: 4,
        })?;
        for address in [
            flash.reader.nand.as_ref().expect("validated NAND ABI").lock,
            flash
                .reader
                .nand
                .as_ref()
                .expect("validated NAND ABI")
                .unlock,
        ] {
            require(
                address <= u64::from(u32::MAX),
                "NAND entry exceeds the query ABI",
            )?;
        }
        Ok(())
    }
}

struct Query {
    callback: u64,
    stock: u32,
    outputs: u64,
}
impl Query {
    async fn invoke(
        &self,
        reader: &mut Qualified<'_>,
        entry: u32,
        argument: u32,
        output: [u32; 2],
        void: bool,
    ) -> bulk::Result<Vec<u8>> {
        require(
            word(&reader.access.raw(self.callback, 4).await?) == u64::from(self.stock),
            "Query callback is occupied",
        )?;
        let saved = reader.access.raw(self.outputs, 8).await?;
        let outcome = async {
            write(reader, self.outputs, &words(&output)).await?;
            write(reader, self.callback, &entry.to_le_bytes()).await?;
            let reply = reader
                .access
                .session
                .diagnostic_query(DiagnosticTag::Research(argument))
                .await?;
            require(
                word(&reader.access.raw(self.callback, 4).await?) == u64::from(entry),
                "Query callback ownership changed",
            )?;
            let result = reader.access.raw(self.outputs, 8).await?;
            require(
                reply.status == CommandStatus::Success
                    || void
                        && reply.status
                            == CommandStatus::DeviceError {
                                primary: 1,
                                secondary: 0xffff,
                            },
                "Query execution refused",
            )?;
            Ok(result)
        }
        .await;
        let cleanup = async {
            require(
                reader.access.session.state() == SessionState::Ready,
                "Completion unknown; query cleanup withheld",
            )?;
            let current = word(&reader.access.raw(self.callback, 4).await?);
            require(
                current == u64::from(entry) || current == u64::from(self.stock),
                "Query callback ownership changed; cleanup withheld",
            )?;
            write(reader, self.callback, &self.stock.to_le_bytes()).await?;
            write(reader, self.outputs, &saved).await
        }
        .await;
        if cleanup.is_err() {
            reader.access.session.invalidate_completion();
        }
        bulk::finish(outcome, cleanup)
    }
    async fn sync(&self, reader: &mut Qualified<'_>, p: &Payload) -> bulk::Result<()> {
        self.invoke(reader, p.cache_clean, 0, [0, 0], true).await?;
        let barrier = [self.outputs as u32 + 4, 0];
        self.invoke(reader, p.barrier, self.outputs as u32, barrier, true)
            .await?;
        self.invoke(reader, p.instruction_invalidate, 0, [0, 0], false)
            .await?;
        self.invoke(reader, p.barrier, self.outputs as u32, barrier, true)
            .await?;
        self.invoke(reader, p.instruction_barrier, 0, [0, 0], false)
            .await?;
        Ok(())
    }
}
fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|n| n.to_le_bytes()).collect()
}

async fn configuration_flags(session: &mut Session, limits: Limits) -> bulk::Result<[u8; 2]> {
    let mut flags = [0; 2];
    for (value, offset) in flags.iter_mut().zip([0xa3, 0xf7]) {
        *value = configuration::read_byte(
            session,
            ConfigurationView::Ram,
            offset,
            limits.configuration,
        )
        .await?;
    }
    Ok(flags)
}

async fn require_debug_access(session: &mut Session, limits: Limits) -> bulk::Result<[u8; 2]> {
    let flags = configuration_flags(session, limits).await?;
    // The page reader needs debug transport, not service-mode teardown policy.
    // Service-session callers prepare their reset guard before this workflow.
    require(
        flags[1] == 1,
        "Existing debug access is required for NAND backup",
    )?;
    Ok(flags)
}

async fn verify_configuration_flags(
    session: &mut Session,
    limits: Limits,
    before: [u8; 2],
) -> bulk::Result<()> {
    require(
        configuration_flags(session, limits).await? == before,
        "Configuration flags changed during NAND backup",
    )
}

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
    let flags_before = require_debug_access(session, limits).await?;
    let BulkSource::Query {
        callback_table,
        callback_offset,
        stock_callback,
        source_state,
        ..
    } = bulk.source
    else {
        unreachable!()
    };
    let mut access = bulk::Access { session, limits };
    let initial_source = access.raw(source_state, 8).await?;
    let mut reader = if selection.preserves_source() {
        Qualified::prepare_preserving(access.session, bulk, limits).await?
    } else {
        Qualified::prepare(access.session, bulk, limits).await?
    };
    let prepared_source = reader.access.raw(source_state, 8).await?;
    let table = word(&reader.access.raw(callback_table, 4).await?) & !bulk.code_alias;
    require(
        bulk.window.contains(BulkRange {
            address: (table + callback_offset) | bulk.code_alias,
            size: 4,
        }),
        "Query callback slot is outside DRAM",
    )?;
    let query = Query {
        callback: table + callback_offset,
        stock: stock_callback as u32,
        outputs: source_state & !bulk.code_alias,
    };
    let outcome = execute(
        &mut reader,
        &query,
        bulk,
        flash,
        payload,
        image,
        target,
        selection.controller_mode(),
        selection.allows_uncorrectable(),
        callbacks,
    )
    .await;
    let cleanup = async {
        reader.restore().await?;
        require(
            reader.access.session.state() == SessionState::Ready,
            "Completion unknown; source cleanup withheld",
        )?;
        let current = reader.access.raw(source_state, 8).await?;
        require(
            current == prepared_source || current == initial_source,
            "Query source ownership changed",
        )?;
        write(
            &mut reader,
            source_state & !bulk.code_alias,
            &initial_source,
        )
        .await?;
        verify_configuration_flags(reader.access.session, limits, flags_before).await?;
        Ok(())
    }
    .await;
    if cleanup.is_err() {
        reader.access.session.invalidate_completion();
    }
    bulk::finish(outcome, cleanup)
}

async fn execute(
    reader: &mut Qualified<'_>,
    query: &Query,
    bulk: &FirmwareBulk,
    flash: &FirmwareFlash,
    p: &Payload,
    image: &[u8],
    target: BulkRange,
    controller_mode: u32,
    allow_uncorrectable: bool,
    callbacks: &mut super::Callbacks<'_>,
) -> bulk::Result<Backup> {
    // Stock code is read through its uncached view before publishing any callback.
    let checks: Vec<_> = p
        .checks
        .iter()
        .map(|c| CodeCheck {
            address: c.address | bulk.code_alias,
            ..c.clone()
        })
        .collect();
    check_code(reader, &checks).await?;
    let mut driver_state = Vec::new();
    for c in &flash.state_checks {
        let value = word(
            &reader
                .access
                .raw(c.address | bulk.code_alias, u64::from(c.size))
                .await?,
        );
        require(value == c.value, format!("NAND state differs: {}", c.name))?;
        driver_state.push(value);
    }
    // Invoke only the lower-level physical-page reader. No high-level driver
    // initializer, shutdown, mapping, journal, program or erase request runs.
    let configuration = match p.configuration {
        Configuration::Pointer { address } => word(
            &reader
                .access
                .raw(u64::from(address) | bulk.code_alias, 4)
                .await?,
        ),
        Configuration::Fixed { address } => u64::from(address),
    } & !bulk.code_alias;
    let bootstrap = configuration + 0x80;
    require(
        bulk.window.contains(BulkRange {
            address: configuration | bulk.code_alias,
            size: 0x100,
        }),
        "Configuration scratch is outside DRAM",
    )?;
    let original = reader.access.raw(bootstrap, 32).await?;
    require(
        reader.access.raw(bootstrap | bulk.code_alias, 32).await? == original,
        "Bootstrap aliases disagree",
    )?;
    let mut stub = image[..p.bootstrap_size as usize].to_vec();
    stub.resize(32, 0);
    let mut touched = false;
    let mut allocation = None;
    let mut uploaded = false;
    let mut base = 0;
    let minimum = flash.control.size.max(0x10000).next_power_of_two();
    let mut capacity = target.size.max(minimum).min(CAPACITY).next_power_of_two();
    let outcome = async {
        touched = true;
        write(reader, bootstrap, &stub).await?;
        query.sync(reader, p).await?;
        require(
            reader.access.raw(bootstrap | bulk.code_alias, 32).await? == stub,
            "Bootstrap uncached readback differs",
        )?;
        let raw = loop {
            let bytes = (capacity + BUFFER + 32) as u32;
            let reply = query
                .invoke(reader, bootstrap as u32, bytes, [0, p.allocate], false)
                .await?;
            let raw = word(&reply[..4]);
            // A confirmed NULL is allocation failure, not uncertain completion.
            // Try the next smaller owned buffer without changing read semantics.
            if raw == 0 && capacity > minimum {
                capacity /= 2;
            } else {
                break raw;
            }
        };
        let allocation_bytes = capacity + BUFFER + 32;
        require(
            raw != 0
                && raw.is_multiple_of(8)
                && raw < bulk.code_alias
                && bulk.window.contains(BulkRange {
                    address: raw | bulk.code_alias,
                    size: allocation_bytes,
                }),
            "Stock allocation is outside DRAM; retained",
        )?;
        base = (raw + 31) & !31;
        let configuration_word = match p.configuration {
            Configuration::Pointer { address } => (u64::from(address) & !bulk.code_alias, 4),
            Configuration::Fixed { .. } => (configuration, 0x100),
        };
        for (at, n) in [
            (bootstrap, 32),
            (query.callback, 4),
            (query.outputs, 8),
            (configuration, 0x100),
            configuration_word,
        ]
        .into_iter()
        .chain(p.checks.iter().map(|c| (c.address, c.size)))
        {
            require(
                !(raw < at + n && at < raw + allocation_bytes),
                "Stock allocation overlaps protected state; retained",
            )?;
        }
        allocation = Some(raw);
        let code = &image[p.reader_offset as usize..];
        // Write in the executing CPU's cacheable view, then clean and invalidate.
        write(reader, base, code).await?;
        require(
            reader.access.raw(base, code.len() as u64).await? == code,
            "Reader upload differs",
        )?;
        uploaded = true;
        query.sync(reader, p).await?;
        require(
            reader
                .read(BulkRange {
                    address: base | bulk.code_alias,
                    size: code.len() as u64,
                })
                .await?
                == code,
            "Reader DMA readback differs",
        )?;
        let lease = &mut CallbackLease::new(query.callback, u64::from(query.stock), base, 4);
        let capture = async {
            lease.publish(reader).await?;
            let mut calls = 0;
            let mut uncorrectable_page_offsets = Vec::new();
            let mut nand_read_stats = super::NandReadStats {
                buffer_bytes: capacity,
                ..Default::default()
            };
            let control = read_chunk(
                reader,
                bulk,
                flash,
                base,
                BulkRange {
                    address: flash.control.offset,
                    size: flash.control.size,
                },
                &mut calls,
                &mut uncorrectable_page_offsets,
                &mut nand_read_stats,
                base + u64::from(p.observer_offset - p.reader_offset),
                lease,
                0,
            )
            .await?;
            require(
                uncorrectable_page_offsets.is_empty(),
                "Control chunk has uncorrectable NAND pages",
            )?;
            require(
                bulk::hex(&sha256_bytes(&control)) == p.control_sha256,
                "Control chunk differs from update; no backup read",
            )?;
            let mut digest = super::xp5::OrderedDigest::default();
            let mut done = 0;
            for (offset, size) in
                super::read_pieces(target, capacity, p.update_size, flash.window.size)
            {
                let bytes = read_chunk(
                    reader,
                    bulk,
                    flash,
                    base,
                    BulkRange {
                        address: offset,
                        size,
                    },
                    &mut calls,
                    &mut uncorrectable_page_offsets,
                    &mut nand_read_stats,
                    base + u64::from(p.observer_offset - p.reader_offset),
                    lease,
                    controller_mode,
                )
                .await?;
                require(
                    uncorrectable_page_offsets.is_empty() || allow_uncorrectable,
                    "Requested flash range has uncorrectable NAND pages",
                )?;
                digest.push(offset - target.address, &bytes);
                (callbacks.chunk)(offset, bytes).map_err(BulkError::Check)?;
                done += size;
                (callbacks.progress)(done, target.size).map_err(BulkError::Check)?;
            }
            Ok(Backup {
                device: flash.device.clone(),
                size: target.size,
                sha256: digest.finish(target.size)?,
                uncorrectable_page_offsets,
                nand_read_stats: Some(nand_read_stats),
                uncorrectable_granularity_bytes: flash
                    .reader
                    .nand
                    .as_ref()
                    .map_or(0, |n| n.page_bytes * n.max_pages_per_call),
            })
        }
        .await;
        let restored = lease.restore(reader).await;
        bulk::finish(capture, restored)
    }
    .await;
    let cleanup = async {
        require(
            reader.access.session.state() == SessionState::Ready,
            "Completion unknown; allocation and bootstrap retained",
        )?;
        if touched {
            require(
                reader.access.raw(bootstrap, 32).await? == stub,
                "Bootstrap ownership changed; cleanup withheld",
            )?;
        }
        for (c, &before) in flash.state_checks.iter().zip(&driver_state) {
            require(
                word(
                    &reader
                        .access
                        .raw(c.address | bulk.code_alias, u64::from(c.size))
                        .await?,
                ) == before,
                "NAND driver state changed; cleanup withheld",
            )?;
        }
        if let Some(raw) = allocation {
            if uploaded {
                require(
                    reader
                        .access
                        .raw(base, (image.len() - p.reader_offset as usize) as u64)
                        .await?
                        == image[p.reader_offset as usize..],
                    "Reader ownership changed; allocation retained",
                )?;
            }
            // The allocation remains owned until this confirmed free returns.
            query
                .invoke(reader, bootstrap as u32, raw as u32, [0, p.release], false)
                .await?;
        }
        if touched {
            let observed = reader.access.raw(bootstrap, 32).await?;
            require(
                observed == stub || observed == original,
                "Bootstrap ownership changed; restoration withheld",
            )?;
            write(reader, bootstrap, &original).await?;
            query.sync(reader, p).await?;
            require(
                reader.access.raw(bootstrap | bulk.code_alias, 32).await? == original,
                "Bootstrap restoration differs",
            )?;
        }
        Ok(())
    }
    .await;
    if cleanup.is_err() {
        reader.access.session.invalidate_completion();
    }
    bulk::finish(outcome, cleanup)
}

async fn read_chunk(
    reader: &mut Qualified<'_>,
    bulk: &FirmwareBulk,
    flash: &FirmwareFlash,
    base: u64,
    target: BulkRange,
    calls: &mut u32,
    uncorrectable_page_offsets: &mut Vec<u64>,
    stats: &mut super::NandReadStats,
    observer: u64,
    lease: &CallbackLease,
    controller_mode: u32,
) -> bulk::Result<Vec<u8>> {
    let (offset, size) = (target.address, target.size);
    let nand = flash.reader.nand.as_ref().expect("validated NAND ABI");
    let stock = flash
        .code_ranges
        .iter()
        .find(|r| r.name == flash.reader.symbol)
        .expect("validated reader")
        .address;
    let request = (base + REQUEST) | bulk.code_alias;
    let buffer = (base + BUFFER) | bulk.code_alias;
    let values = [
        6,
        offset as u32,
        size as u32,
        buffer as u32,
        stock as u32,
        nand.lock as u32,
        nand.unlock as u32,
        nand.timeout_ms,
        flash.window.size as u32,
        nand.page_bytes.trailing_zeros(),
        u32::MAX,
        *calls,
        0,
        0,
        0,
        observer as u32,
        nand.max_pages_per_call,
        controller_mode,
        flash
            .code_ranges
            .iter()
            .find(|r| r.name == "NAND correction mode")
            .ok_or_else(|| BulkError::Check("NAND correction restoration entry is missing".into()))?
            .address as u32,
    ];
    if *calls == 0 {
        write(reader, request, &words(&values)).await?;
    } else {
        // Only source/length change during a capture; result echoes and the
        // monotonic call counter verify this write without a separate readback.
        reader
            .access
            .write(request + 4, &words(&values[1..3]))
            .await?;
        if *calls == 1 && controller_mode != 0 {
            write(reader, request + 68, &controller_mode.to_le_bytes()).await?;
        }
    }
    lease.check(reader).await?;
    let reply = reader
        .access
        .session
        .diagnostic_query(DiagnosticTag::Research(request as u32))
        .await?;
    require(
        reply.status == CommandStatus::Success,
        "NAND query execution refused",
    )?;
    let bitmap_bytes = (size / u64::from(nand.page_bytes)).div_ceil(32) * 4;
    let result = reader.access.raw(request + 40, 52 + bitmap_bytes).await?;
    *calls = calls
        .checked_add(1)
        .ok_or(BulkError::Check("Reader count overflow".into()))?;
    require(
        word(&result[4..8]) == u64::from(*calls)
            && word(&result[8..12]) == offset
            && word(&result[12..16]) == size
            && word(&result[28..32]) == u64::from(controller_mode),
        "NAND result does not describe this request",
    )?;
    require(
        word(&result[..4]) == 0 && word(&result[16..20]) == size,
        format!(
            "Stock NAND read returned {} after {} bytes",
            word(&result[..4]),
            word(&result[16..20])
        ),
    )?;
    stats.stock_calls += word(&result[36..40]);
    stats.burst_calls += word(&result[40..44]);
    stats.fallback_pages += word(&result[44..48]);
    stats.uncorrectable_burst_pages += word(&result[48..52]);
    uncorrectable_page_offsets.extend(decode_uncorrectable(
        &result[52..],
        offset,
        size,
        nand.page_bytes,
    )?);
    reader
        .read(BulkRange {
            address: buffer,
            size,
        })
        .await
}

fn decode_uncorrectable(
    bitmap: &[u8],
    offset: u64,
    size: u64,
    page_bytes: u32,
) -> bulk::Result<Vec<u64>> {
    let pages = size / u64::from(page_bytes);
    require(
        pages > 0
            && pages <= CAPACITY / u64::from(page_bytes)
            && bitmap.len() as u64 == pages.div_ceil(32) * 4,
        "NAND ECC bitmap exceeds request",
    )?;
    let words: Vec<u32> = bitmap
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    require(
        pages.is_multiple_of(32) || words.last().unwrap() >> (pages % 32) == 0,
        "NAND ECC bitmap exceeds request",
    )?;
    Ok((0..pages)
        .filter(|index| words[*index as usize / 32] & (1u32 << (index % 32)) != 0)
        .map(|index| offset + index * u64::from(page_bytes))
        .collect())
}

#[cfg(test)]
#[path = "exr2_tests.rs"]
mod tests;
