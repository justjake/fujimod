//! Owned bulk-memory loans: one staged mapping kept across reads, with the
//! native session retained in the owner state so cancellation can still report
//! what the loan borrowed.
//!
//! A `Memory` handle owns no resource directly. The session's slot points at a
//! [`State`] that holds the native session and the staged mapping; operations
//! lease that pair for one await, exactly as the range reader does, and put it
//! back on an ordinary return or a dropped future. The state keeps a weak
//! back-reference to its owner slot, so the slot can hold the state strongly
//! without a cycle; the handle keeps the Python session alive instead.
use super::*;
use fujimod_catalog_bulk::{BulkDma, BulkRange, FirmwareBulk};
use fujimod_usb::ff80::commands::debug::DebugEvent;
use fujimod_workflows::{
    Limits,
    bulk::{BulkError, BulkLoan, BulkMapping, BulkMappingPhase},
};
use std::sync::{OnceLock, Weak};

fn bulk_error(failure: BulkError) -> PyErr {
    let message = failure.to_string();
    match failure {
        BulkError::Usb(source) => error(source),
        BulkError::Check(_) => PyValueError::new_err(message),
        BulkError::Cleanup { .. } => PyRuntimeError::new_err(message),
    }
}
fn abandoned_error() -> PyErr {
    binding_error(BindingError::Abandoned)
}
fn closed_error() -> PyErr {
    ClosedSessionError::new_err("memory handle is closed")
}
fn phase_name(phase: BulkMappingPhase) -> &'static str {
    match phase {
        BulkMappingPhase::Staged => "Staged",
        BulkMappingPhase::Preparing => "Preparing",
        BulkMappingPhase::Ready => "Ready",
        BulkMappingPhase::Restoring => "Restoring",
        BulkMappingPhase::Restored => "Restored",
        BulkMappingPhase::Unresolved => "Unresolved",
    }
}

/// Where the owned loan's resource is. `InFlight` holds the cancellation
/// sender of the one operation leasing the pair; `Released` marks a handle
/// whose successful close returned the native session to its slot.
enum Resource {
    Idle(Box<ff80::Session>, BulkMapping),
    InFlight { cancel: Option<oneshot::Sender<()>> },
    Released,
}

/// The owner state a session slot points at while a memory handle exists.
pub(super) struct State {
    /// The slot holding this state, so completion paths can verify that they
    /// still own the session before reopening it. Weak: no cycle.
    owner: Weak<Mutex<Slot>>,
    resource: Resource,
    abandoned: bool,
}

/// A detached report of one loan, built from the Rust state alone so no lock is
/// held while Python objects are created.
#[derive(Clone)]
pub(super) struct Snapshot {
    phase: &'static str,
    in_flight: bool,
    closed: bool,
    abandoned: bool,
    loan_available: bool,
    loan: Option<BulkLoan>,
}
impl Snapshot {
    fn into_python(self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let report = PyDict::new(py);
        report.set_item("phase", self.phase)?;
        report.set_item("in_flight", self.in_flight)?;
        report.set_item("closed", self.closed)?;
        report.set_item("abandoned", self.abandoned)?;
        report.set_item("loan_available", self.loan_available)?;
        match &self.loan {
            Some(loan) => {
                let entry = PyDict::new(py);
                entry.set_item("address", loan.address)?;
                entry.set_item("original", PyBytes::new(py, &loan.original))?;
                match &loan.selected {
                    Some(selected) => entry.set_item("selected", PyBytes::new(py, selected))?,
                    None => entry.set_item("selected", py.None())?,
                }
                report.set_item("loan", entry)?;
            }
            None => report.set_item("loan", py.None())?,
        }
        Ok(report.into_any().unbind())
    }
}
impl State {
    fn snapshot(&self) -> Snapshot {
        let abandoned = self.abandoned;
        match &self.resource {
            Resource::Idle(_, mapping) => Snapshot {
                phase: if abandoned {
                    "Abandoned"
                } else {
                    phase_name(mapping.phase())
                },
                in_flight: false,
                closed: false,
                abandoned,
                loan_available: true,
                loan: mapping.loan(),
            },
            Resource::InFlight { .. } => Snapshot {
                phase: if abandoned { "Abandoned" } else { "InFlight" },
                in_flight: true,
                closed: false,
                abandoned,
                loan_available: false,
                loan: None,
            },
            Resource::Released => Snapshot {
                phase: "Restored",
                in_flight: false,
                closed: true,
                abandoned,
                loan_available: false,
                loan: None,
            },
        }
    }
    /// Whether the retained session can no longer be used or restored.
    pub(super) fn fenced(&self) -> bool {
        match &self.resource {
            Resource::Idle(session, mapping) => {
                session.state() != ff80::SessionState::Ready
                    || mapping.phase() == BulkMappingPhase::Unresolved
            }
            _ => false,
        }
    }
}

/// The slot this state belongs to. Reading the weak back-reference takes the
/// state lock only for the upgrade; the caller then takes the slot first, as
/// the lock order requires.
fn owner_arc(state: &Arc<Mutex<State>>) -> PyResult<Arc<Mutex<Slot>>> {
    let owner = { lock(state)?.owner.upgrade() };
    owner.ok_or_else(closed_error)
}

/// Return an untouched loan to its slot when the first lease could not be
/// taken. No exchange has run, so the staged mapping carries nothing to
/// restore and the native session stays usable.
fn release_unprepared(owner: &Arc<Mutex<Slot>>, state: &Arc<Mutex<State>>) {
    let Ok(mut slot) = owner.lock() else {
        return;
    };
    if !matches!(&*slot, Slot::MemoryOwned(attached) if Arc::ptr_eq(attached, state)) {
        return;
    }
    let Ok(mut held) = state.lock() else {
        return;
    };
    let staged = matches!(
        &held.resource,
        Resource::Idle(_, mapping) if mapping.phase() == BulkMappingPhase::Staged
    );
    if !staged {
        return;
    }
    let Resource::Idle(session, _) = std::mem::replace(&mut held.resource, Resource::Released)
    else {
        unreachable!("staged idle resource checked above")
    };
    drop(held);
    *slot = Slot::Open(session);
}

/// The native session and staged mapping leased for one operation. Dropping it
/// returns both to the owner state without issuing any I/O, so a canceled
/// operation keeps its loan report and sends no cleanup traffic.
struct Lease {
    state: Arc<Mutex<State>>,
    session: Option<Box<ff80::Session>>,
    mapping: Option<BulkMapping>,
    cancel: Option<oneshot::Receiver<()>>,
    /// Set while a raw read or debug event is awaiting. A lease dropped before
    /// it settles fences the mapping and the native completion, because an
    /// exchange may have been acknowledged but not observed.
    fence: bool,
}
impl Lease {
    /// Take the idle pair for one operation. The owner slot is read before the
    /// state, and neither guard is held across an await.
    fn take(state: &Arc<Mutex<State>>) -> PyResult<Self> {
        let owner = owner_arc(state)?;
        // Convert errors only after both guards have been dropped. Python
        // error construction may invoke callbacks; it must not hold State.
        let parts = (|| {
            let slot = owner.lock().map_err(|_| BindingError::Poisoned)?;
            match &*slot {
                Slot::MemoryOwned(attached) if Arc::ptr_eq(attached, state) => {}
                Slot::MemoryAbandoned(_) => return Err(BindingError::Abandoned),
                _ => return Err(BindingError::Closed),
            }
            let mut held = state.lock().map_err(|_| BindingError::Poisoned)?;
            if held.abandoned {
                return Err(BindingError::Abandoned);
            }
            if held.fenced() {
                return Err(BindingError::CompletionUnknown);
            }
            match &held.resource {
                Resource::Idle(..) => {}
                Resource::InFlight { .. } => return Err(BindingError::Busy),
                Resource::Released => return Err(BindingError::Closed),
            }
            let (cancel, receiver) = oneshot::channel();
            let Resource::Idle(session, mapping) = std::mem::replace(
                &mut held.resource,
                Resource::InFlight {
                    cancel: Some(cancel),
                },
            ) else {
                unreachable!("idle resource checked above")
            };
            Ok((session, mapping, receiver))
        })();
        let (session, mapping, cancel) = parts.map_err(binding_error)?;
        Ok(Self {
            state: state.clone(),
            session: Some(session),
            mapping: Some(mapping),
            cancel: Some(cancel),
            fence: false,
        })
    }
    fn parts(&mut self) -> (&mut ff80::Session, &mut BulkMapping) {
        (
            self.session.as_mut().expect("leased session"),
            self.mapping.as_mut().expect("leased mapping"),
        )
    }
    async fn prepare(&mut self) -> PyResult<()> {
        let cancel = self.cancel.take().expect("one lease per operation");
        let (session, mapping) = self.parts();
        detached(futures_lite::future::or(
            async move {
                let _ = cancel.await;
                Err(abandoned_error())
            },
            async move {
                mapping
                    .prepare_preserving(session)
                    .await
                    .map_err(bulk_error)
            },
        ))
        .await
    }
    async fn read(&mut self, target: BulkRange) -> PyResult<Vec<u8>> {
        let cancel = self.cancel.take().expect("one lease per operation");
        let (session, mapping) = self.parts();
        detached(futures_lite::future::or(
            async move {
                let _ = cancel.await;
                Err(abandoned_error())
            },
            async move { mapping.read(session, target).await.map_err(bulk_error) },
        ))
        .await
    }
    async fn restore(&mut self) -> PyResult<()> {
        let cancel = self.cancel.take().expect("one lease per operation");
        let (session, mapping) = self.parts();
        detached(futures_lite::future::or(
            async move {
                let _ = cancel.await;
                Err(abandoned_error())
            },
            async move { mapping.restore(session).await.map_err(bulk_error) },
        ))
        .await
    }
    /// One bounded slow read, as the ordinary range reader performs it. The
    /// owner's limits bound every command; no bulk source traffic is sent.
    async fn raw(&mut self, address: u32, length: u32, limits: Limits) -> PyResult<Vec<u8>> {
        let cancel = self.cancel.take().expect("one lease per operation");
        let target = MemoryRange::new(
            Address(address),
            NonZeroU32::new(length)
                .ok_or_else(|| PyValueError::new_err("length must be nonzero"))?,
        )
        .map_err(error)?;
        // Armed before the first await: a dropped future may have sent an
        // exchange whose outcome this owner did not observe.
        self.fence = true;
        let (session, _) = self.parts();
        let outcome = detached(futures_lite::future::or(
            async move {
                let _ = cancel.await;
                RawOutcome::Abandoned
            },
            async move {
                RawOutcome::Returned(
                    fujimod_usb::operations::memory::read_range(session, target, limits.memory)
                        .await
                        .map_err(error),
                )
            },
        ))
        .await;
        self.finish(outcome)
    }
    /// One debug event command on the same retained session.
    async fn event(&mut self, event: DebugEvent, limit: NonZeroU32) -> PyResult<()> {
        let cancel = self.cancel.take().expect("one lease per operation");
        self.fence = true;
        let (session, _) = self.parts();
        let outcome = detached(futures_lite::future::or(
            async move {
                let _ = cancel.await;
                RawOutcome::Abandoned
            },
            async move {
                RawOutcome::Returned(
                    session
                        .send_event(event, limit)
                        .await
                        .map(|_| ())
                        .map_err(error),
                )
            },
        ))
        .await;
        self.finish(outcome)
    }
    /// Settle a raw exchange. A known return with the session still ready keeps
    /// the mapping usable; anything else fences it without further traffic. An
    /// explicit abandon keeps the fence armed for the lease's own drop.
    fn finish<T>(&mut self, outcome: RawOutcome<T>) -> PyResult<T> {
        match outcome {
            RawOutcome::Abandoned => Err(abandoned_error()),
            RawOutcome::Returned(result) => {
                {
                    let (session, mapping) = self.parts();
                    if session.state() != ff80::SessionState::Ready {
                        mapping.invalidate_completion(session);
                    }
                }
                self.fence = false;
                result
            }
        }
    }
}
enum RawOutcome<T> {
    Returned(PyResult<T>),
    Abandoned,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let (Some(mut session), Some(mut mapping)) = (self.session.take(), self.mapping.take())
        else {
            return;
        };
        let (mut held, poisoned) = match self.state.lock() {
            Ok(held) => (held, false),
            Err(poisoned) => (poisoned.into_inner(), true),
        };
        if poisoned {
            held.abandoned = true;
        }
        if held.abandoned {
            // Abandon can win before the shared future is first polled, so
            // its own cancellation guard may not have been armed yet.
            session.invalidate_completion();
        }
        if self.fence {
            // A raw read or event was dropped between exchanges: completion and
            // source ownership are uncertain, so no restoration may follow.
            mapping.invalidate_completion(&mut session);
        }
        if matches!(held.resource, Resource::InFlight { .. }) || poisoned {
            held.resource = Resource::Idle(session, mapping);
        }
    }
}

/// Fence an abandoned loan. Called after the owner slot already reports
/// `MemoryAbandoned` and no owner lock is held: an idle pair is fenced
/// synchronously and an active operation is signaled to stop.
pub(super) fn abandon(state: &Arc<Mutex<State>>) -> PyResult<()> {
    let cancel = {
        let mut held = lock(state)?;
        if held.abandoned {
            return Ok(());
        }
        held.abandoned = true;
        match &mut held.resource {
            Resource::Idle(session, _) => {
                session.invalidate_completion();
                None
            }
            Resource::InFlight { cancel } => cancel.take(),
            Resource::Released => None,
        }
    };
    if let Some(cancel) = cancel {
        let _ = cancel.send(());
    }
    Ok(())
}

/// The firmware's bundled bulk facts for an exact firmware ID.
fn bundled() -> PyResult<&'static fujimod_catalog::ReferenceCatalog> {
    static CATALOG: OnceLock<Result<fujimod_catalog::ReferenceCatalog, String>> = OnceLock::new();
    CATALOG
        .get_or_init(fujimod_catalog::ReferenceCatalog::bundled)
        .as_ref()
        .map_err(|error| PyRuntimeError::new_err(error.clone()))
}

/// A frozen bulk profile built from strict JSON, or the exact bundled ID.
#[pyclass(frozen, from_py_object, module = "fujimod.usb")]
#[derive(Clone)]
pub(super) struct BulkProfile {
    inner: FirmwareBulk,
}
#[pymethods]
impl BulkProfile {
    /// Parse one firmware's bulk facts, refusing unknown fields and invalid
    /// windows, sources or digests before any session is used.
    #[staticmethod]
    fn from_json(text: &str) -> PyResult<Self> {
        let inner: FirmwareBulk =
            serde_json::from_str(text).map_err(|error| PyValueError::new_err(error.to_string()))?;
        inner.validate().map_err(PyValueError::new_err)?;
        Ok(Self { inner })
    }
    #[getter]
    fn id(&self) -> &str {
        &self.inner.id
    }
    #[getter]
    fn firmware_id(&self) -> &str {
        &self.inner.firmware_id
    }
}
impl BulkProfile {
    pub(super) fn facts(&self) -> FirmwareBulk {
        self.inner.clone()
    }
}

/// A memory-handle profile argument: a frozen [`BulkProfile`] or one exact
/// bundled firmware ID. There is no other source.
#[derive(FromPyObject)]
pub(super) enum ProfileArg {
    Profile(BulkProfile),
    Id(String),
}
impl ProfileArg {
    /// Resolve and qualify before the session is touched. An ID must name a
    /// bundled bulk profile; nothing else is consulted.
    fn resolve(self) -> PyResult<FirmwareBulk> {
        match self {
            Self::Profile(profile) => Ok(profile.facts()),
            Self::Id(id) => bundled()?.bulk(&id).cloned().ok_or_else(|| {
                PyValueError::new_err(fujimod_catalog::generation::missing_table_reason(
                    &id, "bulk",
                ))
            }),
        }
    }
}

/// One owned bulk loan: read-only, explicitly closed, and never released by
/// dropping the handle.
#[pyclass(frozen, name = "Memory", module = "fujimod.usb")]
pub(super) struct Memory {
    /// Keeps the session's slot alive for as long as the handle exists; the
    /// state only refers back to it weakly.
    owner: Arc<Mutex<Slot>>,
    state: Arc<Mutex<State>>,
    closed: Mutex<Option<Snapshot>>,
    /// The exact transfer limits this loan was opened with; a raw read never
    /// substitutes wider bounds.
    limits: Limits,
}
impl Memory {
    /// The retained close report, when this handle already closed once.
    fn closed_report(&self) -> PyResult<Option<Snapshot>> {
        Ok(lock(&self.closed)?.clone())
    }
    fn snapshot(&self) -> PyResult<Snapshot> {
        if let Some(report) = self.closed_report()? {
            return Ok(report);
        }
        Ok(lock(&self.state)?.snapshot())
    }
    /// Return the restored native session to the slot this state came from.
    /// The resource leaves the state only when the slot still owns it.
    fn commit(&self, mut lease: Lease) -> PyResult<()> {
        let mut slot = lock(&self.owner)?;
        if !matches!(&*slot, Slot::MemoryOwned(attached) if Arc::ptr_eq(attached, &self.state)) {
            drop(slot);
            return Err(abandoned_error());
        }
        let report = {
            let mut held = lock(&self.state)?;
            let mapping = lease.mapping.as_ref().expect("leased mapping");
            let report = Snapshot {
                phase: phase_name(mapping.phase()),
                in_flight: false,
                closed: true,
                abandoned: false,
                loan_available: true,
                loan: mapping.loan(),
            };
            held.resource = Resource::Released;
            report
        };
        let session = lease.session.take().expect("leased session");
        *slot = Slot::Open(session);
        drop(slot);
        drop(lease.mapping.take());
        *lock(&self.closed)? = Some(report);
        Ok(())
    }
    /// Open a provider on this loan for a remote adapter. Everything that can
    /// refuse is checked here, before any traffic: a closed, abandoned, fenced
    /// or busy loan, and a profile naming another firmware.
    pub(super) fn provider(
        &self,
        firmware_id: &str,
        uncached_alias: u32,
    ) -> PyResult<Box<dyn fujimod_python_usb_base::MemorySource>> {
        if self.closed_report()?.is_some() {
            return Err(closed_error());
        }
        let profile = {
            // Reuse the owner-identity, fence and busy checks. Taking and
            // returning an unstarted lease sends no traffic, and error
            // conversion happens after the state guards have been dropped.
            let lease = Lease::take(&self.state)?;
            lease
                .mapping
                .as_ref()
                .expect("leased mapping")
                .profile()
                .clone()
        };
        if profile.firmware_id != firmware_id {
            return Err(PyValueError::new_err(format!(
                "memory loan profile is for firmware {}, not {firmware_id}",
                profile.firmware_id
            )));
        }
        Ok(Box::new(Provider {
            _owner: self.owner.clone(),
            state: self.state.clone(),
            limits: self.limits,
            window: profile.window,
            dma: profile.dma,
            uncached_alias,
        }))
    }
}

/// A provider over one open memory loan. It holds both the owner slot and the
/// loan state, so dropping it retains the borrowed source instead of restoring
/// it; only the memory handle's explicit close does that.
struct Provider {
    /// Retains the loan's owner slot for the provider's lifetime; the loan
    /// state only refers back to it weakly.
    _owner: Arc<Mutex<Slot>>,
    state: Arc<Mutex<State>>,
    limits: Limits,
    window: BulkRange,
    dma: Option<BulkDma>,
    /// The remote facts' declared alias, already applied to remote addresses.
    /// Only its canonical counterpart may select the existing bulk window.
    uncached_alias: u32,
}
impl fujimod_python_usb_base::MemorySource for Provider {
    fn limits(&self) -> DebugMemoryLimits {
        self.limits.memory
    }
    fn read<'a>(
        &'a self,
        address: u32,
        length: u32,
    ) -> Pin<Box<dyn std::future::Future<Output = PyResult<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move {
            if length == 0 {
                return Err(PyValueError::new_err("length must be nonzero"));
            }
            let mut lease = Lease::take(&self.state)?;
            let requested = BulkRange {
                address: u64::from(address),
                size: u64::from(length),
            };
            // Honor a profile that directly admits the supplied address.
            // Otherwise remove only the alias declared by these remote facts;
            // the resulting target must still lie in the unchanged profile.
            let boundary = 1u64 << self.uncached_alias.trailing_zeros();
            let last = requested.address + requested.size - 1;
            let continuous = requested.address / boundary == last / boundary;
            let target = if self.window.contains(requested) || !continuous {
                requested
            } else {
                BulkRange {
                    address: u64::from(address & !self.uncached_alias),
                    size: requested.size,
                }
            };
            // Only a declared, admitted range larger than the owner's slow-read
            // limit uses the shared E1 mapping. Every other range stays a
            // bounded ordinary read; an E1 failure is never retried as one.
            if length > self.limits.memory.read.get()
                && self.window.contains(target)
                && self.dma.is_none_or(|dma| dma.admits(target))
            {
                lease.read(target).await
            } else {
                lease.raw(address, length, self.limits).await
            }
        })
    }
    fn event<'a>(
        &'a self,
        event: DebugEvent,
        limit: NonZeroU32,
    ) -> Pin<Box<dyn std::future::Future<Output = PyResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut lease = Lease::take(&self.state)?;
            lease.event(event, limit).await
        })
    }
}
#[pymethods]
impl Memory {
    /// Read-only: one range through the borrowed source. The handle stays open.
    async fn read(&self, address: u32, length: u32) -> PyResult<Py<PyBytes>> {
        nz(length)?;
        if self.closed_report()?.is_some() {
            return Err(closed_error());
        }
        let target = BulkRange {
            address: u64::from(address),
            size: u64::from(length),
        };
        let mut lease = Lease::take(&self.state)?;
        let data = lease.read(target).await?;
        Ok(bytes(data))
    }
    /// Restore the borrowed source and return the session to its slot. Calling
    /// it again after a successful close does nothing.
    async fn aclose(&self) -> PyResult<()> {
        if self.closed_report()?.is_some() {
            return Ok(());
        }
        let mut lease = Lease::take(&self.state)?;
        match lease.restore().await {
            Ok(()) => self.commit(lease),
            Err(error) => {
                // The lease drop keeps the fenced pair attached, so the report
                // stays readable and every later request refuses.
                drop(lease);
                Err(error)
            }
        }
    }
    async fn __aenter__(slf: Py<Self>) -> Py<Self> {
        slf
    }
    /// Context-manager cleanup preserves a body exception: a failed restore
    /// adds a note to it instead of replacing it.
    async fn __aexit__(
        &self,
        _kind: Py<PyAny>,
        value: Py<PyAny>,
        _traceback: Py<PyAny>,
    ) -> PyResult<bool> {
        match self.aclose().await {
            Ok(()) => Ok(false),
            Err(error) => {
                let primary = Python::attach(|py| !_kind.is_none(py) && !value.is_none(py));
                if !primary {
                    return Err(error);
                }
                Python::attach(|py| {
                    let note = format!("memory cleanup failed: {}", error.value(py));
                    // Python 3.10 has no exception notes; never mask the body
                    // exception for want of one.
                    let _ = value.bind(py).call_method1("add_note", (note,));
                });
                Ok(false)
            }
        }
    }
    /// The loan report, plus this handle's closed state.
    #[getter]
    fn state(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.snapshot()?.into_python(py)
    }
}

#[pymethods]
impl AsyncSession {
    /// Qualify the selected firmware's stock bulk path once and keep the
    /// borrowed source until `memory.aclose()`. The profile is either a frozen
    /// `usb.BulkProfile` or one exact bundled firmware ID.
    async fn memory(&self, profile: ProfileArg) -> PyResult<Memory> {
        let bulk = profile.resolve()?;
        let limits = Limits {
            memory: self.limits.memory,
            configuration: self.limits.configuration,
        };
        // Malformed facts refuse here, before the slot is touched and before
        // any USB traffic.
        let mapping = BulkMapping::new(&bulk, limits).map_err(bulk_error)?;
        let state = {
            let mut slot = lock(&self.inner)?;
            slot.available()?;
            let Slot::Open(native) = std::mem::replace(&mut *slot, Slot::CompletionUnknown) else {
                unreachable!("an available slot holds an open session")
            };
            let state = Arc::new(Mutex::new(State {
                owner: Arc::downgrade(&self.inner),
                resource: Resource::Idle(native, mapping),
                abandoned: false,
            }));
            *slot = Slot::MemoryOwned(state.clone());
            state
        };
        let mut lease = match Lease::take(&state) {
            Ok(lease) => lease,
            Err(error) => {
                release_unprepared(&self.inner, &state);
                return Err(error);
            }
        };
        match lease.prepare().await {
            Ok(()) => {
                // An abandon during setup may have won the race; never
                // resurrect it with a returned handle.
                let attached = owner_arc(&state).is_ok_and(|owner| {
                    owner.lock().is_ok_and(|slot| {
                        matches!(&*slot, Slot::MemoryOwned(held) if Arc::ptr_eq(held, &state))
                    })
                });
                if !attached {
                    drop(lease);
                    return Err(abandoned_error());
                }
                drop(lease);
                Ok(Memory {
                    owner: self.inner.clone(),
                    state,
                    closed: Mutex::new(None),
                    limits,
                })
            }
            Err(error) => {
                let known = {
                    let session = lease.session.as_ref().expect("leased session");
                    let mapping = lease.mapping.as_ref().expect("leased mapping");
                    mapping.phase() == BulkMappingPhase::Restored
                        && session.state() == ff80::SessionState::Ready
                };
                // A known setup error that already restored the pair leaves a
                // usable session: return it to the slot, then report the error.
                if known
                    && let Ok(owner) = owner_arc(&state)
                    && let Ok(mut slot) = owner.lock()
                    && matches!(&*slot, Slot::MemoryOwned(attached) if Arc::ptr_eq(attached, &state))
                {
                    let session = lease.session.take().expect("leased session");
                    let mapping = lease.mapping.take().expect("leased mapping");
                    *slot = Slot::Open(session);
                    drop(slot);
                    drop(mapping);
                    return Err(error);
                }
                drop(lease);
                Err(error)
            }
        }
    }
    /// The attached loan's report, or `None` when no owner is attached.
    #[getter]
    fn memory_state(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let report = {
            let slot = lock(&self.inner)?;
            match &*slot {
                Slot::MemoryOwned(state) | Slot::MemoryAbandoned(state) => {
                    Some(lock(state)?.snapshot())
                }
                _ => None,
            }
        };
        match report {
            Some(report) => report.into_python(py),
            None => Ok(py.None()),
        }
    }
}

pub(super) fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<BulkProfile>()?;
    module.add_class::<Memory>()
}
