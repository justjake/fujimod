//! Stock diagnostic bulk (E1) memory reads from a firmware's catalog bulk facts.
//!
//! Qualification checks camera identity, stock code, source setup and
//! bulk/slow probe comparisons. The single-read helper restores its source
//! before returning; an owner-held mapping keeps it until explicit restoration.
//! Nothing here writes settings, retries a command or reads outside the table's
//! window.
use crate::{Limits, access::range};
use fujimod_catalog_bulk::{BulkRange, BulkSource, FirmwareBulk};
use fujimod_usb::{
    ff80::{
        Session, SessionState,
        commands::{
            diagnostic::{CommandStatus, DiagnosticTag},
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

/// Bytes read through the bulk path, and source setup's changed RAM extent
/// (`address`, `size`). Pattern scratch can remain changed. A Publication pair
/// is restored on known completion, but reads covering it contain the selected
/// source words: this is live RAM, not a snapshot of the pre-setup state.
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
    let overwritten = qualified.mapping.overwritten();
    let restored = qualified.restore().await;
    finish(outcome.map(|data| BulkRead { data, overwritten }), restored)
}

mod mapping;

pub use mapping::{BulkLoan, BulkMapping, BulkMappingPhase};

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
    mapping: BulkMapping,
}
impl<'a> Qualified<'a> {
    pub(crate) async fn prepare_preserving(
        session: &'a mut Session,
        bulk: &FirmwareBulk,
        limits: Limits,
    ) -> Result<Self> {
        let mut mapping = BulkMapping::new(bulk, limits)?;
        mapping.prepare_preserving(session).await?;
        Ok(Self {
            access: Access { session, limits },
            mapping,
        })
    }

    pub(crate) async fn prepare(
        session: &'a mut Session,
        bulk: &FirmwareBulk,
        limits: Limits,
    ) -> Result<Self> {
        let mapping = BulkMapping::prepare(session, bulk, limits).await?;
        Ok(Self {
            access: Access { session, limits },
            mapping,
        })
    }
    pub(crate) async fn read(&mut self, target: BulkRange) -> Result<Vec<u8>> {
        self.mapping.read(&mut *self.access.session, target).await
    }
    pub(crate) fn capacity(&self) -> u32 {
        self.mapping.capacity()
    }
    pub(crate) async fn read_progress(
        &mut self,
        target: BulkRange,
        progress: &mut dyn FnMut(u64) -> std::result::Result<(), String>,
    ) -> Result<Vec<u8>> {
        self.mapping
            .read_progress(&mut *self.access.session, target, progress)
            .await
    }
    pub(crate) async fn restore(&mut self) -> Result<()> {
        self.mapping.restore(&mut *self.access.session).await
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
    bulk.validate().map_err(BulkError::Check)?;
    check_camera(session, &bulk.product, &bulk.internal_version).await
}

/// Require the camera's reported product name and internal version.
pub(crate) async fn check_camera(
    session: &mut Session,
    expected_product: &str,
    expected_version: &str,
) -> Result<()> {
    let product = session.device_info(InfoSelector::Product).await?.text;
    require(
        product.trim() == expected_product,
        &format!("Connected camera {product:?} is not the selected camera"),
    )?;
    let version = session
        .device_info(InfoSelector::InternalVersion)
        .await?
        .text;
    require(
        version.trim() == expected_version,
        &format!("Internal version {version:?} is not the selected firmware"),
    )
}

async fn check_code(access: &mut Access<'_>, bulk: &FirmwareBulk) -> Result<()> {
    let ranges = bulk
        .code_checks
        .iter()
        .map(|check| {
            memory_range(BulkRange {
                address: check.address | bulk.code_alias,
                size: check.size,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let bytes = memory::read_ranges(access.session, &ranges, access.limits.memory).await?;
    for (check, bytes) in bulk.code_checks.iter().zip(bytes) {
        require(
            hex(&checks::sha256_bytes(&bytes)) == check.sha256,
            &format!("Stock code differs: {}", check.name),
        )?;
    }
    Ok(())
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

// The generated counting pattern is 64 KiB, but that is not an E1 transport
// limit. Its checked echo arm can publish up to the registered data capacity.
// Borrow that same bound without generating a pattern or touching its buffer.
fn transfer_capacity(access: &Access<'_>, registered: u32, pattern: u32) -> Result<u32> {
    require(
        registered >= pattern && pattern > 0,
        "Registered E1 capacity is unavailable",
    )?;
    Ok(registered.min(access.session.max_reply_bytes().get()))
}

/// The prepared E1 source: how offsets map to addresses, and what to restore.
/// It starts empty and only gains a window once the source is qualified; the
/// loan is installed before anything may write the borrowed cell. `restored`
/// records that no borrowed cell is left changed, so a setup error can report
/// a closed loan instead of an unresolved one.
struct Source {
    window: Option<DataWindow>,
    overwritten: Option<(u64, u32)>,
    loan: Option<Loan>,
    restored: bool,
}

/// One borrowed source-state cell. `selected` is the value this mapping chose,
/// known before the write for publication and pattern borrows and only after a
/// readback for a query.
#[derive(Clone, Debug)]
struct Loan {
    address: u64,
    original: Vec<u8>,
    selected: Option<Vec<u8>>,
}
impl Loan {
    /// Whether the cell currently holds a value this mapping may overwrite with
    /// the original bytes.
    fn allows(&self, current: &[u8]) -> bool {
        current == self.original || self.selected.as_deref() == Some(current)
    }
    /// The value the mapping itself selected: what an owner check must find in
    /// the cell before more data is read through it.
    fn expected(&self) -> Option<&[u8]> {
        self.selected.as_deref()
    }
    fn report(&self) -> BulkLoan {
        BulkLoan {
            address: self.address,
            original: self.original.clone(),
            selected: self.selected.clone(),
        }
    }
}
impl Source {
    fn staged() -> Self {
        Self {
            window: None,
            overwritten: None,
            loan: None,
            restored: true,
        }
    }

    fn set_alignment(&mut self, alignment: DataAlignment) {
        if let Some(window) = self.window {
            self.window = Some(window.with_alignment(alignment));
        }
    }

    fn capacity(&self) -> Option<u32> {
        self.window.map(|window| window.capacity().get())
    }

    fn report(&self) -> Option<BulkLoan> {
        self.loan.as_ref().map(Loan::report)
    }

    /// Borrow the selected source, storing its original cell bytes in this
    /// stage before anything may write them. The preserving path never fills
    /// diagnostic scratch: Pattern and PatternZeroBase borrow their state pair
    /// instead of running the pattern setup.
    async fn borrow(&mut self, access: &mut Access<'_>, bulk: &FirmwareBulk) -> Result<()> {
        let aperture = memory_range(bulk.window)?;
        let (state, original, selected, capacity, overwritten) = match &bulk.source {
            BulkSource::DirectQuery { .. } => return self.prepare(access, bulk).await,
            BulkSource::Query { source_state, .. } => {
                // The callback and diagnostic dispatcher use the cacheable
                // publication cell. Borrow and restore that same view; writing
                // its uncached alias can be overwritten by a dirty cache line.
                let state = *source_state & !bulk.code_alias;
                let original = access.raw(state, 8).await?;
                self.loan = Some(Loan {
                    address: state,
                    original: original.clone(),
                    selected: None,
                });
                self.restored = false;
                if let Err(error) = self.prepare(access, bulk).await {
                    if access.session.state() == SessionState::Ready {
                        match access.raw(state, 8).await {
                            Ok(current) if current == original => self.restored = true,
                            Ok(_) => access.session.invalidate_completion(),
                            Err(cleanup) => {
                                access.session.invalidate_completion();
                                return Err(BulkError::Cleanup {
                                    operation: error.to_string(),
                                    cleanup: Box::new(cleanup),
                                });
                            }
                        }
                    }
                    return Err(error);
                }
                // Preparation checked the query's pair against the profile.
                // A later readback must still match it; do not adopt a foreign
                // pair as ours or make it eligible for restoration.
                let selected = access.raw(state, 8).await?;
                if self.loan.as_ref().and_then(|loan| loan.selected.as_ref()) != Some(&selected) {
                    access.session.invalidate_completion();
                    return Err(BulkError::Check(
                        "Bulk source changed owner during query setup".into(),
                    ));
                }
                return Ok(());
            }
            BulkSource::Publication {
                source_state,
                capacity,
            } => {
                // Borrow the same cacheable cell the publisher writes, and use
                // a zero base so E1 offsets are CPU addresses.
                let state = *source_state & !bulk.code_alias;
                let original = access.raw(state, 8).await?;
                let capacity = (*capacity).min(access.session.max_reply_bytes().get());
                let mut selected = vec![0; 4];
                selected.extend(capacity.to_le_bytes());
                require(
                    original != selected,
                    "Borrowed E1 publication already holds the selected source; ownership is unresolved",
                )?;
                (state, original, selected, capacity, Some((state, 8)))
            }
            BulkSource::Pattern {
                source_state,
                registered_buffer,
                capacity,
            } => {
                let descriptor = access.raw(*registered_buffer, 8).await?;
                let capacity = transfer_capacity(access, word(&descriptor, 4), *capacity)?;
                let original = access.raw(*source_state, 8).await?;
                let mut selected = vec![0; 4];
                selected.extend(capacity.to_le_bytes());
                (*source_state, original, selected, capacity, None)
            }
            BulkSource::PatternZeroBase {
                source_state,
                registered_buffer,
                capacity,
                ..
            } => {
                let descriptor = access.raw(*registered_buffer, 12).await?;
                let capacity = transfer_capacity(access, word(&descriptor, 8), *capacity)?;
                let original = access.raw(*source_state, 12).await?;
                let mut selected = vec![0; 8];
                selected.extend(capacity.to_le_bytes());
                (*source_state, original, selected, capacity, None)
            }
        };
        // Journal the pair before the write: from here on the cell may hold the
        // original or the selected bytes, and restoration may accept either.
        self.overwritten = overwritten;
        self.loan = Some(Loan {
            address: state,
            original,
            selected: Some(selected.clone()),
        });
        self.restored = false;
        let window = DataWindow::new(
            Address(0),
            aperture,
            NonZeroU32::new(capacity).ok_or(fujimod_usb::Error::InvalidLength)?,
            OffsetMode::Checked,
        )?;
        let outcome = async {
            access.write(state, &selected).await?;
            require(
                access.raw(state, selected.len() as u64).await? == selected,
                "Borrowed E1 publication did not read back",
            )
        }
        .await;
        if let Err(error) = outcome {
            let restored = self.restore(access).await;
            if restored.is_err() {
                access.session.invalidate_completion();
            }
            return finish(Err(error), restored);
        }
        self.window = Some(window);
        Ok(())
    }

    async fn prepare(&mut self, access: &mut Access<'_>, bulk: &FirmwareBulk) -> Result<()> {
        let nonzero = |n: u32| NonZeroU32::new(n).ok_or(fujimod_usb::Error::InvalidLength);
        let aperture = memory_range(bulk.window)?;
        match bulk.source {
            BulkSource::Publication { .. } => Err(BulkError::Check(
                "Publication sources are borrowed, never prepared".into(),
            )),
            BulkSource::DirectQuery {
                tag,
                query_base,
                base,
                data_offset,
                ref capacities,
                wrapping,
            } => {
                let reply = access
                    .session
                    .diagnostic_query(DiagnosticTag::Research(tag))
                    .await?;
                require(
                    reply.status == CommandStatus::Success,
                    "Bulk source query refused",
                )?;
                let capacity = reply.fields[1];
                require(
                    u64::from(reply.fields[0]) == query_base
                        && capacities.contains(&capacity)
                        && data_offset < capacity,
                    "Bulk source or capacity differs from the located facts",
                )?;
                let mode = if wrapping {
                    OffsetMode::Wrapping
                } else {
                    OffsetMode::Checked
                };
                self.window = Some(DataWindow::new(
                    Address(address(base)?),
                    aperture,
                    nonzero(capacity)?,
                    mode,
                )?);
                self.overwritten = None;
                Ok(())
            }
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
                let state = access.raw(source_state & !alias, 8).await?;
                let capacity = word(&state, 4);
                require(
                    u64::from(word(&state, 0)) == base
                        && capacities.contains(&capacity)
                        && reply.fields[1] == capacity,
                    "Bulk source or capacity differs from the located facts",
                )?;
                if let Some(loan) = self.loan.as_mut() {
                    loan.selected = Some(state);
                }
                let mode = if wrapping {
                    OffsetMode::Wrapping
                } else {
                    OffsetMode::Checked
                };
                self.window = Some(DataWindow::new(
                    Address(address(base)?),
                    aperture,
                    nonzero(capacity)?,
                    mode,
                )?);
                self.overwritten = None;
                Ok(())
            }
            BulkSource::Pattern { .. } | BulkSource::PatternZeroBase { .. } => Err(
                BulkError::Check("Pattern sources must borrow their publication pair".into()),
            ),
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
    async fn bulk_progress<F: FnMut(u64) -> std::result::Result<(), String> + ?Sized>(
        &self,
        access: &mut Access<'_>,
        target: BulkRange,
        progress: &mut F,
    ) -> Result<Vec<u8>> {
        let window = self
            .window
            .ok_or_else(|| BulkError::Check("Bulk source is not prepared".into()))?;
        window.validate(memory_range(target)?)?;
        let mut bytes = Vec::with_capacity(target.size as usize);
        while (bytes.len() as u64) < target.size {
            // Another owner may have republished the source since the last
            // chunk; never send an E1 request over a pair this read did not
            // select.
            self.check_owner(access).await?;
            let done = bytes.len() as u64;
            let remaining = BulkRange {
                address: target.address + done,
                size: target.size - done,
            };
            // Borrow the session for one chunk only, so the ownership check
            // above and below can use it; DataReader keeps the reply-limit,
            // aperture and alignment bounds of this same window.
            let chunk = {
                let mut reader =
                    DataReader::new(&mut *access.session, window, memory_range(remaining)?)?;
                reader.next_chunk().await?
            };
            let chunk = chunk.ok_or_else(|| BulkError::Check("Incomplete bulk range".into()))?;
            self.check_owner(access).await?;
            progress(chunk.len() as u64).map_err(BulkError::Check)?;
            bytes.extend(chunk);
        }
        Ok(bytes)
    }

    /// Require the borrowed publication to still hold the bytes this read
    /// selected, so a foreign owner is never read through or overwritten.
    /// Non-borrowed sources have nothing to check.
    async fn check_owner(&self, access: &mut Access<'_>) -> Result<()> {
        let Some(loan) = &self.loan else {
            return Ok(());
        };
        require(
            access.session.state() == SessionState::Ready,
            "Session completion unknown; the borrowed bulk source is not read further",
        )?;
        let size = loan.original.len() as u64;
        let current = match access.raw(loan.address, size).await {
            Ok(current) => current,
            Err(error) => {
                access.session.invalidate_completion();
                return Err(error);
            }
        };
        if loan.expected() != Some(current.as_slice()) {
            access.session.invalidate_completion();
            return Err(BulkError::Check(
                "Bulk source changed owner; no further bulk data is read".into(),
            ));
        }
        Ok(())
    }

    async fn restore(&mut self, access: &mut Access<'_>) -> Result<()> {
        let Some(loan) = &self.loan else {
            self.restored = true;
            return Ok(());
        };
        require(
            access.session.state() == SessionState::Ready,
            "Session completion unknown; the bulk source was not restored",
        )?;
        let size = loan.original.len() as u64;
        let current = access.raw(loan.address, size).await?;
        require(
            loan.allows(&current),
            "Bulk source changed owner; restoration withheld",
        )?;
        access.write(loan.address, &loan.original).await?;
        require(
            access.raw(loan.address, size).await? == loan.original,
            "Restored bulk source did not read back",
        )?;
        self.restored = true;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests;
