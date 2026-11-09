//! One staged E1 mapping: its phases, its guarded operation and its loan.
//!
//! The source-specific setup lives in [`super::Source`]; this module only owns
//! the persistent stage, the ownership of a loan across calls, and the fence
//! that a dropped operation leaves behind.
use super::{
    Access, BulkError, Result, Source, check_code, check_identity, finish, memory_range, require,
};
use crate::Limits;
use fujimod_catalog_bulk::{BulkRange, BulkSource, FirmwareBulk};
use fujimod_usb::{
    ff80::{Session, SessionState},
    operations::data::DataAlignment,
};

/// Where a staged mapping is in its loan.
///
/// `Staged` before preparation, `Preparing` while the source is set up,
/// `Ready` for reads, `Restoring`, and `Restored` when no source loan remains
/// (setup changed no source, or restoration was verified). `Unresolved` means
/// completion or source ownership is uncertain; such a mapping sends no more
/// USB traffic. Its original source bytes, when captured, remain held.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BulkMappingPhase {
    Staged,
    Preparing,
    Ready,
    Restoring,
    Restored,
    Unresolved,
}

/// The source-state cell a mapping borrowed, as read from the camera.
/// `selected` contains the planned bytes for a host write, or the observed
/// bytes for a stock query. It stays `None` until the query's pair is read back.
/// The phase determines whether publication or restoration completed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BulkLoan {
    pub address: u64,
    pub original: Vec<u8>,
    pub selected: Option<Vec<u8>>,
}

/// A staged bulk mapping: the immutable firmware profile, its limits and the
/// borrowed source, kept by its owner across calls without borrowing the
/// session. Construct it with [`BulkMapping::new`] before the first await,
/// prepare it with [`BulkMapping::prepare_preserving`], and close it with
/// [`BulkMapping::restore`] by mutable reference.
///
/// The stage keeps the original source bytes after errors, cancellation and
/// successful restoration so the owner can report them; the owner must keep
/// the mapping for the whole loan, since a discarded mapping can neither report
/// nor restore what it held.
pub struct BulkMapping {
    pub(super) bulk: FirmwareBulk,
    pub(super) limits: Limits,
    pub(super) alignment: Option<DataAlignment>,
    pub(super) phase: BulkMappingPhase,
    pub(super) source: Source,
}
impl BulkMapping {
    /// Validate the firmware's bulk metadata and bounds without touching USB,
    /// so malformed facts are refused before a session is used.
    pub fn new(bulk: &FirmwareBulk, limits: Limits) -> Result<Self> {
        bulk.validate().map_err(BulkError::Check)?;
        memory_range(bulk.window)?;
        let alignment = bulk
            .dma
            .map(|dma| DataAlignment::new(dma.address_alignment, dma.length_granule))
            .transpose()?;
        Ok(Self {
            bulk: bulk.clone(),
            limits,
            alignment,
            phase: BulkMappingPhase::Staged,
            source: Source::staged(),
        })
    }

    /// Where the staged loan stands. Reads need `Ready`; `Restored` holds no
    /// live loan, and `Unresolved` sends no traffic.
    pub fn phase(&self) -> BulkMappingPhase {
        self.phase
    }

    /// The source-state cell this mapping borrowed, with its original bytes and
    /// the planned host-write bytes or observed query bytes. Reported
    /// independently of `overwritten()` and kept after errors, cancellation and
    /// restoration. `None` for a source that borrows no cell.
    pub fn loan(&self) -> Option<BulkLoan> {
        self.source.report()
    }

    /// Qualify the path without the preserving contract and keep the result.
    /// Queries may leave their selected publication installed. Publication and
    /// pattern sources still borrow their pair. Use the staged preserving API
    /// when an owner must retain a loan across cancellation during setup.
    pub async fn prepare(
        session: &mut Session,
        bulk: &FirmwareBulk,
        limits: Limits,
    ) -> Result<Self> {
        let mut mapping = Self::new(bulk, limits)?;
        mapping.prepare_with(session, false).await?;
        Ok(mapping)
    }

    /// Borrow only the located E1 publication words. Unlike pattern setup this
    /// does not fill diagnostic scratch. The publication words themselves are
    /// temporarily changed; Publication reports that extent as overwritten.
    /// The original bytes are stored in this mapping before any potentially
    /// mutating exchange, and kept for the owner's report on every outcome.
    pub async fn prepare_preserving(&mut self, session: &mut Session) -> Result<()> {
        self.prepare_with(session, true).await
    }

    async fn prepare_with(&mut self, session: &mut Session, preserving: bool) -> Result<()> {
        require(
            self.phase == BulkMappingPhase::Staged,
            "Bulk mapping was already prepared",
        )?;
        let mut operation = Operation::new(self, session);
        operation.mapping.phase = BulkMappingPhase::Preparing;
        let outcome = operation.stage(preserving).await;
        operation.mapping.phase = match &outcome {
            Ok(()) => BulkMappingPhase::Ready,
            Err(_) if operation.session.state() != SessionState::Ready => {
                BulkMappingPhase::Unresolved
            }
            // A known setup error that verified the cell back to its original
            // bytes left a closed loan; anything else is unresolved.
            Err(_) if operation.mapping.source.restored => BulkMappingPhase::Restored,
            Err(_) => BulkMappingPhase::Unresolved,
        };
        if operation.mapping.phase == BulkMappingPhase::Unresolved {
            operation.session.invalidate_completion();
        }
        operation.disarm();
        outcome
    }

    /// Source setup's changed RAM extent (`address`, `size`). Publication
    /// reports its temporarily replaced pair, restored only when restoration
    /// succeeds. Pattern scratch is not restored by closing the mapping.
    pub fn overwritten(&self) -> Option<(u64, u32)> {
        self.source.overwritten
    }
    /// Read a range through the borrowed source. The mapping must be `Ready`;
    /// a malformed target is refused before any USB traffic. Dropping this
    /// future before it returns fences the session without sending anything.
    pub async fn read(&mut self, session: &mut Session, target: BulkRange) -> Result<Vec<u8>> {
        self.accept_read(target)?;
        let mut operation = Operation::new(self, session);
        let outcome = operation.read(target).await;
        operation.finish_read();
        outcome
    }
    pub(crate) async fn read_progress(
        &mut self,
        session: &mut Session,
        target: BulkRange,
        progress: &mut dyn FnMut(u64) -> std::result::Result<(), String>,
    ) -> Result<Vec<u8>> {
        self.accept_read(target)?;
        let mut operation = Operation::new(self, session);
        let outcome = operation.read_progress(target, progress).await;
        operation.finish_read();
        outcome
    }
    fn accept_read(&self, target: BulkRange) -> Result<()> {
        require(
            self.phase == BulkMappingPhase::Ready,
            "Bulk mapping is not ready for reads",
        )?;
        require(
            self.bulk.window.contains(target),
            "Range is outside the bulk window",
        )?;
        self.source
            .window
            .ok_or_else(|| BulkError::Check("Bulk source is not prepared".into()))?
            .validate(memory_range(target)?)?;
        Ok(())
    }
    /// Restore the borrowed source by mutable reference and keep the stage with
    /// its original bytes. Idempotent only once a restoration succeeded; a
    /// failed restoration fences the session and leaves the mapping unresolved,
    /// which refuses further traffic instead of retrying cleanup.
    pub async fn restore(&mut self, session: &mut Session) -> Result<()> {
        match self.phase {
            BulkMappingPhase::Restored => return Ok(()),
            BulkMappingPhase::Ready => {}
            BulkMappingPhase::Unresolved => {
                return Err(BulkError::Check(
                    "Bulk mapping is unresolved (completion unknown); no restoration traffic is sent".into(),
                ));
            }
            BulkMappingPhase::Staged
            | BulkMappingPhase::Preparing
            | BulkMappingPhase::Restoring => {
                return Err(BulkError::Check(
                    "Bulk mapping is not ready for restoration".into(),
                ));
            }
        }
        if session.state() != SessionState::Ready {
            self.phase = BulkMappingPhase::Unresolved;
            return Err(BulkError::Check(
                "Session completion unknown; no restoration traffic is sent".into(),
            ));
        }
        let mut operation = Operation::new(self, session);
        operation.mapping.phase = BulkMappingPhase::Restoring;
        let outcome = operation.restore().await;
        match &outcome {
            Ok(()) => operation.mapping.phase = BulkMappingPhase::Restored,
            Err(_) => {
                operation.mapping.phase = BulkMappingPhase::Unresolved;
                operation.session.invalidate_completion();
            }
        }
        operation.disarm();
        outcome
    }
    pub(super) fn capacity(&self) -> u32 {
        self.source
            .capacity()
            .expect("qualified bulk source has a data window")
    }
}

/// Fence one mapping operation. If the operation future is dropped before it
/// returns, the mapping becomes `Unresolved` and the session's completion is
/// invalidated synchronously, without any USB traffic — even when the last
/// exchange was acknowledged and the session is still `Ready`. Only an
/// ordinary return disarms it.
pub(super) struct Operation<'a> {
    mapping: &'a mut BulkMapping,
    session: &'a mut Session,
    armed: bool,
}
impl<'a> Operation<'a> {
    pub(super) fn new(mapping: &'a mut BulkMapping, session: &'a mut Session) -> Self {
        Self {
            mapping,
            session,
            armed: true,
        }
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
    fn finish_read(&mut self) {
        if self.session.state() != SessionState::Ready {
            self.mapping.phase = BulkMappingPhase::Unresolved;
        }
        self.disarm();
    }
    async fn stage(&mut self, preserving: bool) -> Result<()> {
        let Operation {
            mapping, session, ..
        } = self;
        let limits = mapping.limits;
        let alignment = mapping.alignment;
        let BulkMapping { bulk, source, .. } = &mut **mapping;
        check_identity(session, bulk).await?;
        let mut access = Access { session, limits };
        check_code(&mut access, bulk).await?;
        if preserving {
            source.borrow(&mut access, bulk).await?;
        } else {
            match bulk.source {
                BulkSource::Query { .. } | BulkSource::DirectQuery { .. } => {
                    source.prepare(&mut access, bulk).await?
                }
                // Publication always borrows the located pair, in both prepare
                // paths; it is never initialized from a pattern or a query.
                BulkSource::Publication { .. }
                | BulkSource::Pattern { .. }
                | BulkSource::PatternZeroBase { .. } => source.borrow(&mut access, bulk).await?,
            }
        }
        if let Some(alignment) = alignment {
            source.set_alignment(alignment);
        }
        if let Err(error) = source.qualify(&mut access, bulk).await {
            let restored = source.restore(&mut access).await;
            if restored.is_err() {
                access.session.invalidate_completion();
            }
            return finish(Err(error), restored);
        }
        Ok(())
    }
    async fn read(&mut self, target: BulkRange) -> Result<Vec<u8>> {
        let Operation {
            mapping, session, ..
        } = self;
        let limits = mapping.limits;
        let mut access = Access { session, limits };
        mapping.source.bulk(&mut access, target).await
    }
    async fn read_progress(
        &mut self,
        target: BulkRange,
        progress: &mut dyn FnMut(u64) -> std::result::Result<(), String>,
    ) -> Result<Vec<u8>> {
        let Operation {
            mapping, session, ..
        } = self;
        let limits = mapping.limits;
        let mut access = Access { session, limits };
        mapping
            .source
            .bulk_progress(&mut access, target, progress)
            .await
    }
    async fn restore(&mut self) -> Result<()> {
        let Operation {
            mapping, session, ..
        } = self;
        let limits = mapping.limits;
        let mut access = Access { session, limits };
        mapping.source.restore(&mut access).await
    }
}
impl Drop for Operation<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.mapping.phase = BulkMappingPhase::Unresolved;
            self.session.invalidate_completion();
        }
    }
}

impl fujimod_usb::operations::card::BufferIo for BulkMapping {
    fn admits(&self, address: u32, size: u32) -> bool {
        self.enclosing(address, size).is_some()
    }

    fn read<'a>(
        &'a mut self,
        session: &'a mut Session,
        address: u32,
        size: u32,
    ) -> fujimod_usb::emulator::EndpointFuture<'a, fujimod_usb::Result<Vec<u8>>> {
        Box::pin(async move {
            let target = self
                .enclosing(address, size)
                .ok_or(fujimod_usb::Error::OutsideDataWindow)?;
            let raw =
                BulkMapping::read(self, session, target)
                    .await
                    .map_err(|error| match error {
                        BulkError::Usb(error) => error,
                        error => fujimod_usb::Error::Card(error.to_string()),
                    })?;
            let skip = (u64::from(address) - target.address) as usize;
            raw.get(skip..skip + size as usize)
                .map(<[u8]>::to_vec)
                .ok_or(fujimod_usb::Error::InvalidLength)
        })
    }
}

impl BulkMapping {
    fn enclosing(&self, address: u32, size: u32) -> Option<BulkRange> {
        if size == 0 {
            return None;
        }
        let (alignment, granule) = self.bulk.dma.map_or((1, 1), |dma| {
            (
                u64::from(dma.address_alignment),
                u64::from(dma.length_granule),
            )
        });
        let start = u64::from(address) / alignment * alignment;
        let size = (u64::from(address) - start + u64::from(size)).div_ceil(granule) * granule;
        let range = BulkRange {
            address: start,
            size,
        };
        (start.checked_add(size)? <= 1u64 << 32 && self.bulk.window.contains(range))
            .then_some(range)
    }
}
