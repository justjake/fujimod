//! Stock diagnostic bulk (E1) memory reads from a firmware's catalog bulk facts.
//!
//! Every read qualifies the path first: camera identity, stock code, the stock
//! routine that points E1 at memory, and bulk/slow probe comparisons. A source
//! override is restored before the read returns. Nothing here writes settings,
//! retries a command or reads outside the table's window.
use crate::{Limits, access::range};
use fujimod_catalog_bulk::{BulkRange, BulkSource, FirmwareBulk};
use fujimod_usb::{
    ff80::{
        Session, SessionState,
        commands::{
            diagnostic::{CommandStatus, DataReadMode, DiagnosticTag},
            info::InfoSelector,
            memory::{Address, MemoryRange},
        },
    },
    operations::{
        checks,
        data::{DataAlignment, DataReader, DataWindow, OffsetMode},
        memory,
    },
};
use std::num::NonZeroU32;

#[derive(Debug, thiserror::Error)]
pub enum BulkError {
    #[error(transparent)]
    Usb(#[from] fujimod_usb::Error),
    #[error("{0}")]
    Check(String),
    #[error("{operation}; restoring the bulk source failed: {cleanup}")]
    Cleanup {
        operation: String,
        cleanup: Box<BulkError>,
    },
}
pub type Result<T> = std::result::Result<T, BulkError>;

fn address(value: u64) -> Result<u32> {
    u32::try_from(value)
        .map_err(|_| BulkError::Check(format!("{value:#x} is beyond 32-bit E1 addressing")))
}
pub(crate) fn memory_range(target: BulkRange) -> Result<MemoryRange> {
    let size =
        u32::try_from(target.size).map_err(|_| BulkError::Check("Range too large".into()))?;
    Ok(range(address(target.address)?, size)?)
}

fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(BulkError::Check(message.into()))
    }
}

/// Bytes read through the bulk path, and diagnostic scratch the stock setup
/// overwrote (`address`, `size`), which may lie inside the bytes read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BulkRead {
    pub data: Vec<u8>,
    pub overwritten: Option<(u64, u32)>,
}

/// Qualify the bulk path, read `target`, and restore any source override.
pub async fn read(
    session: &mut Session,
    bulk: &FirmwareBulk,
    limits: Limits,
    target: BulkRange,
) -> Result<BulkRead> {
    require(
        bulk.window.contains(target),
        "Range is outside the bulk window",
    )?;
    require(
        bulk.dma.is_none_or(|dma| dma.admits(target)),
        "Range is not aligned to the bulk DMA",
    )?;
    memory_range(target)?;
    let mut qualified = Qualified::prepare(session, bulk, limits).await?;
    let outcome = qualified.read(target).await;
    let overwritten = qualified.source.overwritten;
    let restored = qualified.restore().await;
    finish(outcome.map(|data| BulkRead { data, overwritten }), restored)
}

/// A qualified bulk mapping kept across calls without borrowing the session.
/// Pass the same session to every call, and `restore` it when finished.
pub struct BulkMapping {
    source: Source,
    window: BulkRange,
    limits: Limits,
}
impl BulkMapping {
    pub async fn prepare(
        session: &mut Session,
        bulk: &FirmwareBulk,
        limits: Limits,
    ) -> Result<Self> {
        let Qualified { source, .. } = Qualified::prepare(session, bulk, limits).await?;
        Ok(Self {
            source,
            window: bulk.window,
            limits,
        })
    }
    /// Diagnostic scratch the stock setup overwrote (`address`, `size`).
    pub fn overwritten(&self) -> Option<(u64, u32)> {
        self.source.overwritten
    }
    pub async fn read(&self, session: &mut Session, target: BulkRange) -> Result<Vec<u8>> {
        require(
            self.window.contains(target),
            "Range is outside the bulk window",
        )?;
        let limits = self.limits;
        self.source
            .bulk(&mut Access { session, limits }, target)
            .await
    }
    pub async fn restore(self, session: &mut Session) -> Result<()> {
        let limits = self.limits;
        let result = self.source.restore(&mut Access { session, limits }).await;
        if result.is_err() {
            session.invalidate_completion();
        }
        result
    }
}

pub(crate) fn finish<T>(outcome: Result<T>, restored: Result<()>) -> Result<T> {
    match (outcome, restored) {
        (Ok(data), Ok(())) => Ok(data),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(error), Err(cleanup)) => Err(BulkError::Cleanup {
            operation: error.to_string(),
            cleanup: Box::new(cleanup),
        }),
    }
}

/// One qualified mapping, kept for a multi-command workflow. Call `restore`
/// before releasing it; unknown completion withholds all cleanup traffic.
pub(crate) struct Qualified<'a> {
    pub(crate) access: Access<'a>,
    source: Source,
}
impl<'a> Qualified<'a> {
    pub(crate) async fn prepare(
        session: &'a mut Session,
        bulk: &FirmwareBulk,
        limits: Limits,
    ) -> Result<Self> {
        let alignment = bulk
            .dma
            .map(|dma| DataAlignment::new(dma.address_alignment, dma.length_granule))
            .transpose()?;
        check_identity(session, bulk).await?;
        let mut access = Access { session, limits };
        for check in &bulk.code_checks {
            let bytes = access
                .raw(check.address | bulk.code_alias, check.size)
                .await?;
            require(
                hex(&checks::sha256_bytes(&bytes)) == check.sha256,
                &format!("Stock code differs: {}", check.name),
            )?;
        }
        let mut source = Source::prepare(&mut access, bulk).await?;
        if let Some(alignment) = alignment {
            source.window = source.window.with_alignment(alignment);
        }
        if let Err(error) = source.qualify(&mut access, bulk).await {
            let restored = source.restore(&mut access).await;
            if restored.is_err() {
                access.session.invalidate_completion();
            }
            return finish(Err(error), restored);
        }
        Ok(Self { access, source })
    }
    pub(crate) async fn read(&mut self, target: BulkRange) -> Result<Vec<u8>> {
        self.source.bulk(&mut self.access, target).await
    }
    pub(crate) async fn read_progress(
        &mut self,
        target: BulkRange,
        progress: &mut dyn FnMut(u64) -> std::result::Result<(), String>,
    ) -> Result<Vec<u8>> {
        self.source
            .bulk_progress(&mut self.access, target, progress)
            .await
    }
    pub(crate) async fn restore(&mut self) -> Result<()> {
        let result = self.source.restore(&mut self.access).await;
        if result.is_err() {
            self.access.session.invalidate_completion();
        }
        result
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Slow memory reads within one qualified session.
pub(crate) struct Access<'a> {
    pub(crate) session: &'a mut Session,
    pub(crate) limits: Limits,
}
impl Access<'_> {
    pub(crate) async fn raw(&mut self, at: u64, size: u64) -> Result<Vec<u8>> {
        let target = memory_range(BulkRange { address: at, size })?;
        Ok(memory::read_range(self.session, target, self.limits.memory).await?)
    }
    pub(crate) async fn write(&mut self, at: u64, bytes: &[u8]) -> Result<()> {
        memory::write_range(
            self.session,
            Address(address(at)?),
            bytes,
            self.limits.memory,
        )
        .await?;
        Ok(())
    }
}

pub(crate) async fn check_identity(session: &mut Session, bulk: &FirmwareBulk) -> Result<()> {
    let product = session.device_info(InfoSelector::Product).await?.text;
    require(
        product.trim() == bulk.product,
        &format!("Connected camera {product:?} is not the selected camera"),
    )?;
    let version = session
        .device_info(InfoSelector::InternalVersion)
        .await?
        .text;
    require(
        version.trim() == bulk.internal_version,
        &format!("Internal version {version:?} is not the selected firmware"),
    )
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

/// The prepared E1 source: how offsets map to addresses, and what to restore.
struct Source {
    window: DataWindow,
    overwritten: Option<(u64, u32)>,
    /// Source-state address, its original bytes and the values it may hold
    /// before restoration.
    saved: Option<(u64, Vec<u8>, Vec<Vec<u8>>)>,
}
impl Source {
    async fn prepare(access: &mut Access<'_>, bulk: &FirmwareBulk) -> Result<Self> {
        let nonzero = |n: u32| NonZeroU32::new(n).ok_or(fujimod_usb::Error::InvalidLength);
        let aperture = memory_range(bulk.window)?;
        match bulk.source {
            BulkSource::Query {
                tag,
                callback_table,
                callback_offset,
                stock_callback,
                base,
                source_state,
                ref capacities,
                wrapping,
            } => {
                // The table pointer may hold the cached or the uncached alias.
                let table = u64::from(word(&access.raw(callback_table, 4).await?, 0));
                let alias = bulk.code_alias;
                let physical = if table >= alias { table - alias } else { table };
                let slot = alias + physical + callback_offset;
                require(
                    table % 4 == 0
                        && bulk.window.contains(BulkRange {
                            address: slot,
                            size: 4,
                        }),
                    "Bulk callback table is outside the RAM window",
                )?;
                require(
                    access.raw(slot, 4).await? == address(stock_callback)?.to_le_bytes(),
                    "Require the stock diagnostic query callback",
                )?;
                let reply = access
                    .session
                    .diagnostic_query(DiagnosticTag::Research(tag))
                    .await?;
                require(
                    reply.status == CommandStatus::Success,
                    "Bulk source query refused",
                )?;
                let state = access.raw(source_state, 8).await?;
                let capacity = word(&state, 4);
                require(
                    u64::from(word(&state, 0)) == base
                        && capacities.contains(&capacity)
                        && reply.fields[1] == capacity,
                    "Bulk source or capacity differs from the located facts",
                )?;
                let mode = if wrapping {
                    OffsetMode::Wrapping
                } else {
                    OffsetMode::Checked
                };
                Ok(Self {
                    window: DataWindow::new(
                        Address(address(base)?),
                        aperture,
                        nonzero(capacity)?,
                        mode,
                    )?,
                    overwritten: None,
                    saved: None,
                })
            }
            BulkSource::Pattern {
                registered_buffer,
                source_state,
                capacity,
            } => {
                let descriptor = access.raw(registered_buffer, 8).await?;
                let base = u64::from(word(&descriptor, 0));
                require(
                    base != 0 && word(&descriptor, 4) >= capacity,
                    "Registered scratch cannot hold the stock pattern",
                )?;
                protect(
                    bulk,
                    base,
                    capacity,
                    &[(registered_buffer, 8), (source_state, 8)],
                )?;
                pattern(access, capacity).await?;
                let mut expected = descriptor[..4].to_vec();
                expected.extend(capacity.to_le_bytes());
                require(
                    access.raw(source_state, 8).await? == expected,
                    "Prepared source differs from the registered scratch",
                )?;
                verify_pattern(access, capacity).await?;
                Ok(Self {
                    window: DataWindow::new(
                        Address(word(&descriptor, 0)),
                        aperture,
                        nonzero(capacity)?,
                        OffsetMode::Wrapping,
                    )?,
                    overwritten: Some((base, capacity)),
                    saved: None,
                })
            }
            BulkSource::PatternZeroBase {
                registered_buffer,
                source_state,
                capacity,
            } => {
                let descriptor = access.raw(registered_buffer, 12).await?;
                let base = u64::from_le_bytes(descriptor[..8].try_into().expect("eight bytes"));
                require(
                    base != 0
                        && word(&descriptor, 8) >= capacity
                        && base.checked_add(u64::from(capacity)).is_some(),
                    "Registered scratch cannot hold the stock pattern",
                )?;
                protect(
                    bulk,
                    base,
                    capacity,
                    &[(registered_buffer, 12), (source_state, 12)],
                )?;
                let original = access.raw(source_state, 12).await?;
                let mut prepared = descriptor[..8].to_vec();
                prepared.extend(capacity.to_le_bytes());
                let mut zeroed = vec![0; 8];
                zeroed.extend(capacity.to_le_bytes());
                let mut source = Self {
                    window: DataWindow::new(
                        Address(0),
                        aperture,
                        nonzero(capacity)?,
                        OffsetMode::Checked,
                    )?,
                    overwritten: Some((base, capacity)),
                    saved: Some((
                        source_state,
                        original.clone(),
                        vec![original, prepared.clone(), zeroed],
                    )),
                };
                let setup = async {
                    pattern(access, capacity).await?;
                    require(
                        access.raw(source_state, 12).await? == prepared,
                        "Prepared source differs from the registered scratch",
                    )?;
                    verify_pattern(access, capacity).await?;
                    access.write(source_state, &[0; 8]).await?;
                    require(
                        access.raw(source_state, 8).await? == [0; 8],
                        "Source base did not read back as zero",
                    )
                }
                .await;
                if let Err(error) = setup {
                    let cleanup = source.restore(access).await;
                    if cleanup.is_err() {
                        access.session.invalidate_completion();
                    }
                    source.saved = None;
                    return Err(match cleanup {
                        Ok(()) => error,
                        Err(cleanup) => BulkError::Cleanup {
                            operation: error.to_string(),
                            cleanup: Box::new(cleanup),
                        },
                    });
                }
                Ok(source)
            }
        }
    }

    async fn qualify(&self, access: &mut Access<'_>, bulk: &FirmwareBulk) -> Result<()> {
        for probe in &bulk.probes {
            let slow = access.raw(probe.address, probe.size).await?;
            require(
                self.bulk(access, *probe).await? == slow,
                "Bulk and slow reads disagree",
            )?;
        }
        if let Some(probe) = &bulk.mapped_probe {
            let aliased = BulkRange {
                address: probe.address | bulk.code_alias,
                size: probe.size,
            };
            let bytes = self.bulk(access, aliased).await?;
            require(
                hex(&checks::sha256_bytes(&bytes)) == probe.sha256,
                "Bulk read differs from the firmware's bytes",
            )?;
        }
        Ok(())
    }

    async fn bulk(&self, access: &mut Access<'_>, target: BulkRange) -> Result<Vec<u8>> {
        self.bulk_progress(access, target, &mut |_| Ok(())).await
    }
    async fn bulk_progress(
        &self,
        access: &mut Access<'_>,
        target: BulkRange,
        progress: &mut dyn FnMut(u64) -> std::result::Result<(), String>,
    ) -> Result<Vec<u8>> {
        let mut reader = DataReader::new(&mut *access.session, self.window, memory_range(target)?)?;
        let mut bytes = Vec::with_capacity(target.size as usize);
        while let Some(chunk) = reader.next_chunk().await? {
            progress(chunk.len() as u64).map_err(BulkError::Check)?;
            bytes.extend(chunk);
        }
        Ok(bytes)
    }

    async fn restore(&self, access: &mut Access<'_>) -> Result<()> {
        let Some((address, original, allowed)) = &self.saved else {
            return Ok(());
        };
        require(
            access.session.state() == SessionState::Ready,
            "Session completion unknown; the bulk source was not restored",
        )?;
        let size = original.len() as u64;
        let current = access.raw(*address, size).await?;
        require(
            allowed.contains(&current),
            "Bulk source changed owner; restoration withheld",
        )?;
        access.write(*address, original).await?;
        require(
            access.raw(*address, size).await? == *original,
            "Restored bulk source did not read back",
        )
    }
}

/// The stock pattern setup must not overwrite checked code or transfer state.
fn protect(bulk: &FirmwareBulk, base: u64, capacity: u32, state: &[(u64, u64)]) -> Result<()> {
    let end = base + u64::from(capacity);
    let overlaps = |address: u64, size: u64| base < address.saturating_add(size) && address < end;
    require(
        !bulk
            .code_checks
            .iter()
            .any(|check| overlaps(check.address, check.size))
            && !state.iter().any(|&(address, size)| overlaps(address, size)),
        "Diagnostic scratch overlaps checked code or transfer state",
    )
}

async fn pattern(access: &mut Access<'_>, capacity: u32) -> Result<()> {
    let reply = access
        .session
        .prepare_data_read(DataReadMode::Pattern)
        .await?;
    require(
        reply.status == CommandStatus::Success && reply.fields[1] == capacity,
        "Pattern setup refused or returned another capacity",
    )
}

async fn verify_pattern(access: &mut Access<'_>, capacity: u32) -> Result<()> {
    let step = access.session.max_reply_bytes().get();
    let mut offset = 0;
    while offset < capacity {
        let count = step.min(capacity - offset);
        let bytes = access
            .session
            .read_data(offset, NonZeroU32::new(count).expect("nonzero count"))
            .await?;
        require(
            bytes
                .iter()
                .enumerate()
                .all(|(index, &byte)| byte == (offset as usize + index) as u8),
            "Diagnostic scratch pattern mismatch",
        )?;
        offset += count;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
