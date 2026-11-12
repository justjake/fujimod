//! Owned bulk-memory loans: one staged mapping kept across reads, with the
//! native session retained in the owner state so cancellation can still report
//! what the loan borrowed.
//!
//! A `Memory` handle owns no resource directly. The session's slot points at a
//! [`State`] that holds the native session and the staged mapping; operations
//! lease that pair for one await, exactly as the range reader does, and put it
//! back on an ordinary return or a dropped future. The state keeps a weak
//! back-reference to its owner slot, so the slot can hold the state strongly
//! without a cycle; the handle keeps the session alive instead.
//!
//! Retention ends with the session owner. Dropping the returned handle keeps
//! the loan (and its original source bytes) only while the session handle that
//! opened it is alive; the final session drop and the camera-mount ownership
//! above it are separate work.
use super::*;
use error::{bytes, nz};
use fujimod_catalog_bulk::{BulkRange, FirmwareBulk};
use fujimod_workflows::{
    Limits,
    bulk::{BulkError, BulkLoan, BulkMapping, BulkMappingPhase},
};
use futures_channel::oneshot;
use session::{Cancellation, Session, Slot, take_native};
use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};
use usb::ff80::{
    self,
    commands::memory::{Address, MemoryRange, ReadMemory, WriteMemory},
};

fn bulk_error(source: BulkError) -> JsValue {
    match source {
        BulkError::Usb(source) => error(source),
        other => crate::workflows::workflow_error(other),
    }
}
fn closed() -> JsValue {
    fail(ErrorKind::Closed, "memory handle is closed")
}
fn abandoned_error() -> JsValue {
    fail(ErrorKind::Abandoned, "memory loan was abandoned")
}
fn unresolved() -> JsValue {
    fail(
        ErrorKind::CompletionUnknown,
        "memory completion is unknown; no further transfers are sent",
    )
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
    Idle(ff80::Session, BulkMapping),
    InFlight {
        cancel: Option<oneshot::Sender<Cancellation>>,
    },
    Released,
}

/// The owner state a session slot points at while a memory loan exists.
pub(crate) struct State {
    /// The slot holding this state, so completion paths can verify that they
    /// still own the session before reopening it. Weak: no cycle.
    owner: Weak<RefCell<Slot>>,
    resource: Resource,
    abandoned: bool,
}

/// A detached report of one loan, cloned from the Rust state so no cell borrow
/// is held while the JavaScript objects are created.
#[derive(Clone)]
struct Snapshot {
    phase: &'static str,
    in_flight: bool,
    closed: bool,
    abandoned: bool,
    loan_available: bool,
    loan: Option<BulkLoan>,
}
impl State {
    /// Whether the owner abandoned this loan; its report and borrowed bytes
    /// stay reachable, but no operation may run and nothing is restored.
    pub(crate) fn abandoned(&self) -> bool {
        self.abandoned
    }
    /// The session-level state this loan reports.
    pub(crate) fn session_state(&self) -> crate::types::SessionState {
        use crate::types::SessionState;
        match &self.resource {
            Resource::InFlight { .. } => SessionState::InFlight,
            Resource::Idle(session, mapping) => {
                if session.state() != ff80::SessionState::Ready
                    || mapping.phase() == BulkMappingPhase::Unresolved
                {
                    SessionState::CompletionUnknown
                } else {
                    SessionState::MemoryOwned
                }
            }
            Resource::Released => SessionState::MemoryOwned,
        }
    }
    /// The current lease's cancellation sender, when one is pending.
    pub(crate) fn take_cancel(&mut self) -> Option<oneshot::Sender<Cancellation>> {
        match &mut self.resource {
            Resource::InFlight { cancel } => cancel.take(),
            _ => None,
        }
    }
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
    pub(crate) fn fenced(&self) -> bool {
        match &self.resource {
            Resource::Idle(session, mapping) => {
                session.state() != ff80::SessionState::Ready
                    || mapping.phase() == BulkMappingPhase::Unresolved
            }
            _ => false,
        }
    }
}

/// The slot this state belongs to. The weak back-reference is upgraded before
/// the slot is borrowed, so the two cells are never taken in the other order.
fn owner_of(state: &Rc<RefCell<State>>) -> Result<Rc<RefCell<Slot>>, JsValue> {
    let owner = {
        let held = state.borrow();
        held.owner.upgrade()
    };
    // The state guard is released before the error is built.
    match owner {
        Some(owner) => Ok(owner),
        None => Err(closed()),
    }
}

/// Return an untouched loan to its slot when the first lease could not be
/// taken. No exchange has run, so the staged mapping carries nothing to
/// restore and the native session stays usable.
fn release_unprepared(owner: &Rc<RefCell<Slot>>, state: &Rc<RefCell<State>>) {
    let mut slot = owner.borrow_mut();
    if !matches!(&*slot, Slot::MemoryOwned(attached) if Rc::ptr_eq(attached, state)) {
        return;
    }
    let mut held = state.borrow_mut();
    let staged = matches!(
        &held.resource,
        Resource::Idle(_, mapping) if mapping.phase() == BulkMappingPhase::Staged
    );
    if !staged {
        return;
    }
    let previous = std::mem::replace(&mut held.resource, Resource::Released);
    let Resource::Idle(session, _) = previous else {
        unreachable!("staged idle resource checked above")
    };
    drop(held);
    *slot = Slot::Open(session);
}

/// The native session and staged mapping leased for one operation. Dropping it
/// returns both to the owner state without issuing any I/O, so a canceled
/// operation keeps its loan report and sends no cleanup traffic.
struct Lease {
    state: Rc<RefCell<State>>,
    session: Option<ff80::Session>,
    mapping: Option<BulkMapping>,
    cancel: Option<oneshot::Receiver<Cancellation>>,
    /// Set while a raw read or write is awaiting. A lease dropped before it
    /// settles fences the mapping and the native completion, because an
    /// exchange may have been acknowledged but not observed.
    fence: bool,
}
/// A pure reason a loan operation cannot start or settle. JavaScript errors are
/// constructed only after every state or slot cell guard is released.
enum LoanError {
    Closed,
    Abandoned,
    Unresolved,
    Busy,
}
impl LoanError {
    fn into_error(self) -> JsValue {
        match self {
            Self::Closed => closed(),
            Self::Abandoned => abandoned_error(),
            Self::Unresolved => unresolved(),
            Self::Busy => fail(ErrorKind::Busy, "memory loan has a pending operation"),
        }
    }
}

impl Lease {
    /// Take the idle pair for one operation. The owner slot is read before the
    /// state, and neither cell stays borrowed across an await or an error
    /// construction.
    fn take(state: &Rc<RefCell<State>>) -> Result<Self, JsValue> {
        let owner = owner_of(state)?;
        let attached = {
            let slot = owner.borrow();
            matches!(&*slot, Slot::MemoryOwned(held) if Rc::ptr_eq(held, state))
        };
        let mut failure = if attached {
            None
        } else {
            Some(LoanError::Closed)
        };
        let mut parts = None;
        if failure.is_none() {
            let mut held = state.borrow_mut();
            if held.abandoned {
                failure = Some(LoanError::Abandoned);
            } else if held.fenced() {
                failure = Some(LoanError::Unresolved);
            } else {
                let (cancel, receiver) = oneshot::channel();
                let previous = std::mem::replace(
                    &mut held.resource,
                    Resource::InFlight {
                        cancel: Some(cancel),
                    },
                );
                match previous {
                    Resource::Idle(session, mapping) => {
                        parts = Some((session, mapping, receiver));
                    }
                    other => {
                        failure = Some(match &other {
                            Resource::InFlight { .. } => LoanError::Busy,
                            _ => LoanError::Closed,
                        });
                        held.resource = other;
                    }
                }
            }
        }
        match (failure, parts) {
            (Some(failure), _) => Err(failure.into_error()),
            (None, Some((session, mapping, cancel))) => Ok(Self {
                state: state.clone(),
                session: Some(session),
                mapping: Some(mapping),
                cancel: Some(cancel),
                fence: false,
            }),
            (None, None) => unreachable!("an attached idle loan yields a pair"),
        }
    }
    fn parts(&mut self) -> (&mut ff80::Session, &mut BulkMapping) {
        (
            self.session.as_mut().expect("leased session"),
            self.mapping.as_mut().expect("leased mapping"),
        )
    }
    async fn prepare(&mut self) -> Result<(), JsValue> {
        let cancel = self.cancel.take().expect("one lease per operation");
        let (session, mapping) = self.parts();
        futures_lite::future::or(async move { Err(cancel_error(cancel).await) }, async move {
            mapping
                .prepare_preserving(session)
                .await
                .map_err(bulk_error)
        })
        .await
    }
    async fn read(&mut self, target: BulkRange) -> Result<Vec<u8>, JsValue> {
        let cancel = self.cancel.take().expect("one lease per operation");
        let (session, mapping) = self.parts();
        futures_lite::future::or(async move { Err(cancel_error(cancel).await) }, async move {
            mapping.read(session, target).await.map_err(bulk_error)
        })
        .await
    }
    async fn restore(&mut self) -> Result<(), JsValue> {
        let cancel = self.cancel.take().expect("one lease per operation");
        let (session, mapping) = self.parts();
        futures_lite::future::or(async move { Err(cancel_error(cancel).await) }, async move {
            mapping.restore(session).await.map_err(bulk_error)
        })
        .await
    }
    /// One bounded slow read, as the ordinary single-packet path performs it.
    /// The fence is armed before the first await: a dropped future may have
    /// sent an exchange whose outcome this owner did not observe.
    async fn raw_read(&mut self, command: ReadMemory) -> Result<Vec<u8>, JsValue> {
        let cancel = self.cancel.take().expect("one lease per operation");
        self.fence = true;
        let outcome = {
            let (session, _) = self.parts();
            futures_lite::future::or(
                async move { Raw::Canceled(cancel_error(cancel).await) },
                async move { Raw::Returned(session.read_memory(command).await.map_err(error)) },
            )
            .await
        };
        self.finish(outcome)
    }
    /// One bounded single-packet write on the retained session.
    async fn raw_write_command(&mut self, command: WriteMemory) -> Result<(), JsValue> {
        let cancel = self.cancel.take().expect("one lease per operation");
        self.fence = true;
        let outcome = {
            let (session, _) = self.parts();
            futures_lite::future::or(
                async move { Raw::Canceled(cancel_error(cancel).await) },
                async move {
                    Raw::Returned(
                        session
                            .write_memory(command)
                            .await
                            .map(|_| ())
                            .map_err(error),
                    )
                },
            )
            .await
        };
        self.finish(outcome)
    }
    /// One larger write split into packet-sized debug commands.
    async fn raw_write_range(
        &mut self,
        address: Address,
        data: Vec<u8>,
        limits: usb::ff80::commands::memory::DebugMemoryLimits,
    ) -> Result<(), JsValue> {
        let cancel = self.cancel.take().expect("one lease per operation");
        self.fence = true;
        let outcome = {
            let (session, _) = self.parts();
            futures_lite::future::or(
                async move { Raw::Canceled(cancel_error(cancel).await) },
                async move {
                    Raw::Returned(
                        usb::operations::memory::write_range(session, address, &data, limits)
                            .await
                            .map_err(error),
                    )
                },
            )
            .await
        };
        self.finish(outcome)
    }
    /// Settle a raw exchange. A known return with the session still ready keeps
    /// the mapping usable; anything else fences it without further traffic. An
    /// explicit cancel keeps the fence armed for this lease's own drop, even
    /// before dispatch, because no completion was observed.
    fn finish<T>(&mut self, outcome: Raw<T>) -> Result<T, JsValue> {
        match outcome {
            Raw::Canceled(error) => Err(error),
            Raw::Returned(result) => {
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
enum Raw<T> {
    Returned(Result<T, JsValue>),
    Canceled(JsValue),
}
async fn cancel_error(mut cancel: oneshot::Receiver<Cancellation>) -> JsValue {
    (&mut cancel)
        .await
        .unwrap_or(Cancellation::Abandoned)
        .error()
}
impl Drop for Lease {
    fn drop(&mut self) {
        let (Some(mut session), Some(mut mapping)) = (self.session.take(), self.mapping.take())
        else {
            return;
        };
        let mut held = self.state.borrow_mut();
        if held.abandoned {
            // Abandon can win before the shared future is first polled, so its
            // own cancellation guard may not have been armed yet.
            session.invalidate_completion();
        }
        if self.fence {
            // A raw read or write was dropped between exchanges: completion and
            // source ownership are uncertain, so no restoration may follow.
            mapping.invalidate_completion(&mut session);
        }
        if matches!(held.resource, Resource::InFlight { .. }) {
            held.resource = Resource::Idle(session, mapping);
        }
    }
}

/// Fence an abandoned loan. The owner slot already holds this state, so the
/// loan stays reportable; nothing is restored, freed or closed here.
pub(crate) fn abandon(state: &Rc<RefCell<State>>) {
    let cancel = {
        let mut held = state.borrow_mut();
        if held.abandoned {
            return;
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
        let _ = cancel.send(Cancellation::Abandoned);
    }
}

/// Parse and validate one bulk profile without touching the session. Unknown
/// fields, invalid windows, sources and digests are refused here.
fn parse_profile(text: &str) -> Result<FirmwareBulk, JsValue> {
    let facts: FirmwareBulk = serde_json::from_str(text).map_err(|error| {
        fail(
            ErrorKind::InvalidArgument,
            &format!("bulk profile: {error}"),
        )
    })?;
    facts
        .validate()
        .map_err(|message| fail(ErrorKind::InvalidArgument, &message))?;
    Ok(facts)
}

/// One frozen bulk profile built from strict JSON.
#[wasm_bindgen]
pub struct BulkProfile {
    inner: FirmwareBulk,
    json: String,
}
#[wasm_bindgen]
impl BulkProfile {
    /// Parse one firmware's bulk facts from strict JSON, refusing unknown
    /// fields and invalid windows, sources or digests before any session use.
    #[wasm_bindgen(js_name = fromJson)]
    pub fn from_json(text: &str) -> Result<Self, JsValue> {
        let inner = parse_profile(text)?;
        let json = serde_json::to_string(&inner)
            .map_err(|error| fail(ErrorKind::InvalidArgument, &error.to_string()))?;
        Ok(Self { inner, json })
    }
    #[wasm_bindgen(getter)]
    pub fn id(&self) -> String {
        self.inner.id.clone()
    }
    #[wasm_bindgen(getter, js_name = firmwareId)]
    pub fn firmware_id(&self) -> String {
        self.inner.firmware_id.clone()
    }
    /// The canonical form of the parsed facts; the descriptor a memory call
    /// reads back is validated exactly as this text was.
    #[wasm_bindgen(getter)]
    pub fn json(&self) -> String {
        self.json.clone()
    }
}
/// A memory-handle profile argument: one exact bundled firmware ID or a
/// descriptor with a strict JSON getter. There is no other source.
fn resolve_profile(value: JsValue) -> Result<FirmwareBulk, JsValue> {
    if let Some(id) = value.as_string() {
        return crate::reference::bundled()?
            .bulk(&id)
            .cloned()
            .ok_or_else(|| {
                crate::workflows::workflow_error(fujimod_catalog::generation::missing_table_reason(
                    &id, "bulk",
                ))
            });
    }
    if value.is_null() || value.is_undefined() {
        return Err(fail(
            ErrorKind::InvalidArgument,
            "memory profile must be a firmware id or usb.BulkProfile",
        ));
    }
    let json = js_sys::Reflect::get(&value, &JsValue::from_str("json")).map_err(|_| {
        fail(
            ErrorKind::InvalidArgument,
            "memory profile descriptor has no readable json",
        )
    })?;
    let text = json.as_string().ok_or_else(|| {
        fail(
            ErrorKind::InvalidArgument,
            "memory profile descriptor json must be a string",
        )
    })?;
    parse_profile(&text)
}

/// An immutable report of one loan's actual state.
#[wasm_bindgen]
#[derive(Clone)]
pub struct MemoryLoan {
    address: u64,
    original: Vec<u8>,
    selected: Option<Vec<u8>>,
}
#[wasm_bindgen]
impl MemoryLoan {
    /// The source-state cell this loan borrowed.
    #[wasm_bindgen(getter)]
    pub fn address(&self) -> u64 {
        self.address
    }
    /// The cell's bytes before setup, as read from the camera.
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn original(&self) -> JsValue {
        bytes(&self.original)
    }
    /// The bytes this loan installed, or undefined when nothing was written.
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array | undefined")]
    pub fn selected(&self) -> JsValue {
        match &self.selected {
            Some(selected) => bytes(selected),
            None => JsValue::UNDEFINED,
        }
    }
}
impl MemoryLoan {
    fn new(loan: BulkLoan) -> Self {
        Self {
            address: loan.address,
            original: loan.original,
            selected: loan.selected,
        }
    }
}

/// An immutable diagnostic snapshot of the memory loan.
#[wasm_bindgen]
#[derive(Clone)]
pub struct MemoryReport {
    phase: String,
    in_flight: bool,
    closed: bool,
    abandoned: bool,
    loan_available: bool,
    loan: Option<MemoryLoan>,
}
#[wasm_bindgen]
impl MemoryReport {
    /// Staged, Preparing, Ready, Restoring, Restored, Unresolved, InFlight or
    /// Abandoned.
    #[wasm_bindgen(getter)]
    pub fn phase(&self) -> String {
        self.phase.clone()
    }
    #[wasm_bindgen(getter, js_name = inFlight)]
    pub fn in_flight(&self) -> bool {
        self.in_flight
    }
    #[wasm_bindgen(getter)]
    pub fn closed(&self) -> bool {
        self.closed
    }
    #[wasm_bindgen(getter)]
    pub fn abandoned(&self) -> bool {
        self.abandoned
    }
    /// False while one operation has the pair leased.
    #[wasm_bindgen(getter, js_name = loanAvailable)]
    pub fn loan_available(&self) -> bool {
        self.loan_available
    }
    /// The borrowed source bytes, when they are not leased out.
    #[wasm_bindgen(getter, unchecked_return_type = "MemoryLoan | undefined")]
    pub fn loan(&self) -> Option<MemoryLoan> {
        self.loan.clone()
    }
}
impl MemoryReport {
    fn new(snapshot: Snapshot) -> Self {
        Self {
            phase: snapshot.phase.to_owned(),
            in_flight: snapshot.in_flight,
            closed: snapshot.closed,
            abandoned: snapshot.abandoned,
            loan_available: snapshot.loan_available,
            loan: snapshot.loan.map(MemoryLoan::new),
        }
    }
}

fn limits(session: &Session) -> Limits {
    Limits {
        memory: session.limits.memory,
        configuration: session.limits.configuration,
    }
}
/// Bind a restored pair back to its slot. Only a slot that still owns this
/// state, a restored mapping and a ready session are released.
fn commit(
    owner: &Rc<RefCell<Slot>>,
    state: &Rc<RefCell<State>>,
    closed: &Rc<RefCell<Option<Snapshot>>>,
    lease: &mut Lease,
) -> Result<(), JsValue> {
    let mut slot = owner.borrow_mut();
    let failure = if !matches!(&*slot, Slot::MemoryOwned(attached) if Rc::ptr_eq(attached, state)) {
        Some(LoanError::Abandoned)
    } else {
        let (session, mapping) = lease.parts();
        if mapping.phase() == BulkMappingPhase::Restored
            && session.state() == ff80::SessionState::Ready
        {
            None
        } else {
            Some(LoanError::Unresolved)
        }
    };
    if let Some(failure) = failure {
        // Convert only after this slot guard is released.
        drop(slot);
        return Err(failure.into_error());
    }
    let report = {
        let (_, mapping) = lease.parts();
        Snapshot {
            phase: phase_name(mapping.phase()),
            in_flight: false,
            closed: true,
            abandoned: false,
            loan_available: true,
            loan: mapping.loan(),
        }
    };
    state.borrow_mut().resource = Resource::Released;
    let session = lease.session.take().expect("leased session");
    let mapping = lease.mapping.take().expect("leased mapping");
    *slot = Slot::Open(session);
    drop(slot);
    drop(mapping);
    *closed.borrow_mut() = Some(report);
    Ok(())
}

/// One owned bulk loan: explicitly closed, and never restored by dropping the
/// handle. Retention is bounded by the session owner (see the module comment).
#[wasm_bindgen]
pub struct Memory {
    session: Session,
    state: Rc<RefCell<State>>,
    closed: Rc<RefCell<Option<Snapshot>>>,
}
impl Memory {
    fn closed_report(&self) -> Option<Snapshot> {
        self.closed.borrow().clone()
    }
    fn snapshot(&self) -> Snapshot {
        if let Some(report) = self.closed_report() {
            return report;
        }
        self.state.borrow().snapshot()
    }
    fn take_lease(&self) -> Result<Lease, JsValue> {
        if self.closed_report().is_some() {
            return Err(closed());
        }
        Lease::take(&self.state)
    }
}

#[wasm_bindgen]
impl Session {
    /// Qualify the firmware's stock bulk path once and keep it until
    /// `memory.close()`. The profile is either one exact bundled firmware ID or
    /// a `usb.BulkProfile`; a descriptor's JSON is validated before the session
    /// is touched. Ordinary session commands, readers and a second memory loan
    /// refuse until the loan is closed or the session is abandoned.
    ///
    /// Retention requires this session handle (or the returned handle) to stay
    /// alive; the final session drop is a separate ownership question.
    ///
    /// A cancel or abandon before the first exchange can leave the slot owned
    /// by a staged loan that never returned a handle: `memoryState` reports
    /// `Staged` with no loan, ordinary work refuses, and this change has no
    /// resume or close API for that owner. `abandon()` ends it. Nothing was
    /// sent and no automatic release is implied.
    #[wasm_bindgen(unchecked_return_type = "Promise<Memory>")]
    pub fn memory(
        &self,
        #[wasm_bindgen(unchecked_param_type = "string | BulkProfile")] profile: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let facts = resolve_profile(profile)?;
        let mapping = BulkMapping::new(&facts, limits(self)).map_err(bulk_error)?;
        let native = take_native(&self.owner, Slot::CompletionUnknown, false)?;
        let state = Rc::new(RefCell::new(State {
            owner: Rc::downgrade(&self.owner),
            resource: Resource::Idle(native, mapping),
            abandoned: false,
        }));
        *self.owner.borrow_mut() = Slot::MemoryOwned(state.clone());
        let mut lease = match Lease::take(&state) {
            Ok(lease) => lease,
            Err(error) => {
                release_unprepared(&self.owner, &state);
                return Err(error);
            }
        };
        let share = self.share();
        let owner = self.owner.clone();
        let closed = Rc::new(RefCell::new(None));
        let caller = js_sys::Error::new("operation started");
        Ok(wasm_bindgen_futures::future_to_promise(async move {
            let result: Result<JsValue, JsValue> = async {
                match lease.prepare().await {
                    Ok(()) => {
                        // An abandon during setup may have won the race; never
                        // resurrect it with a returned handle.
                        let attached = {
                            let slot = owner.borrow();
                            matches!(&*slot, Slot::MemoryOwned(held) if Rc::ptr_eq(held, &state))
                        };
                        let abandoned = state.borrow().abandoned;
                        if !attached || abandoned {
                            drop(lease);
                            return Err(abandoned_error());
                        }
                        drop(lease);
                        Ok(Memory {
                            session: share,
                            state,
                            closed,
                        }
                        .into())
                    }
                    Err(error) => {
                        let known = {
                            let (session, mapping) = lease.parts();
                            mapping.phase() == BulkMappingPhase::Restored
                                && session.state() == ff80::SessionState::Ready
                        };
                        // A known setup error that already restored the pair
                        // leaves a usable session: return it to the slot, then
                        // report the error.
                        if known {
                            let mut slot = owner.borrow_mut();
                            if matches!(&*slot, Slot::MemoryOwned(held) if Rc::ptr_eq(held, &state))
                            {
                                let session = lease.session.take().expect("leased session");
                                let mapping = lease.mapping.take().expect("leased mapping");
                                state.borrow_mut().resource = Resource::Released;
                                *slot = Slot::Open(session);
                                drop(slot);
                                drop(mapping);
                                return Err(error);
                            }
                        }
                        drop(lease);
                        Err(error)
                    }
                }
            }
            .await;
            result.map_err(|failure| error::with_caller(failure, &caller))
        }))
    }

    /// The attached loan's report, or undefined when no loan owns the session.
    #[wasm_bindgen(
        getter,
        js_name = memoryState,
        unchecked_return_type = "MemoryReport | undefined"
    )]
    pub fn memory_state(&self) -> Option<MemoryReport> {
        let snapshot = {
            let slot = self.owner.borrow();
            match &*slot {
                Slot::MemoryOwned(state) => Some(state.borrow().snapshot()),
                _ => None,
            }
        };
        snapshot.map(MemoryReport::new)
    }
}

#[wasm_bindgen]
impl Memory {
    /// Read through the loan: a range larger than the single-packet path uses
    /// the qualified bulk mapping; a small one is a bounded slow read on the
    /// same retained session.
    #[wasm_bindgen(unchecked_return_type = "Promise<Uint8Array>")]
    pub fn read(
        &self,
        #[wasm_bindgen(unchecked_param_type = "number")] address: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] length: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let address = error::integer(address, u32::MAX)?;
        let length = error::integer(length, u32::MAX)?;
        let range = MemoryRange::new(Address(address), nz(length)?).map_err(error)?;
        let mut lease = self.take_lease()?;
        let limits = limits(&self.session);
        let packet = self.session.packet_bytes();
        let budget = self.session.max_reply_bytes();
        let caller = js_sys::Error::new("operation started");
        Ok(wasm_bindgen_futures::future_to_promise(async move {
            let result = async {
                if range.length().get()
                    <= packet
                        .min(limits.memory.read.get())
                        .min(budget.saturating_sub(36))
                {
                    let command = ReadMemory::new(range, limits.memory).map_err(error)?;
                    lease.raw_read(command).await
                } else {
                    let target = BulkRange {
                        address: u64::from(range.address().0),
                        size: u64::from(range.length().get()),
                    };
                    lease.read(target).await
                }
            }
            .await;
            result
                .map(bytes)
                .map_err(|failure| error::with_caller(failure, &caller))
        }))
    }

    /// Existing bounded RAM write, serialized through the loan and fenced when
    /// its completion is uncertain.
    #[wasm_bindgen(unchecked_return_type = "Promise<void>")]
    pub fn write(
        &self,
        #[wasm_bindgen(unchecked_param_type = "number")] address: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] data: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let data = error::buffer(data)?;
        let address = error::integer(address, u32::MAX)?;
        MemoryRange::new(
            Address(address),
            nz(u32::try_from(data.len()).map_err(|_| error(usb::Error::InvalidLength))?)?,
        )
        .map_err(error)?;
        let mut lease = self.take_lease()?;
        let limits = limits(&self.session);
        let packet = self.session.packet_bytes();
        let caller = js_sys::Error::new("operation started");
        Ok(wasm_bindgen_futures::future_to_promise(async move {
            let result = async {
                if data.len() as u64 <= u64::from(packet.min(limits.memory.write.get())) {
                    let command =
                        WriteMemory::new(Address(address), data, limits.memory).map_err(error)?;
                    lease.raw_write_command(command).await
                } else {
                    lease
                        .raw_write_range(Address(address), data, limits.memory)
                        .await
                }
            }
            .await;
            result
                .map(|()| JsValue::UNDEFINED)
                .map_err(|failure| error::with_caller(failure, &caller))
        }))
    }

    /// Diagnostic scratch the stock bulk setup overwrote, which may lie inside
    /// bytes this loan reads; null when the setup overwrote nothing, while an
    /// operation holds the pair, or after a successful close.
    #[wasm_bindgen(
        getter,
        unchecked_return_type = "{ address: number; size: number } | null"
    )]
    pub fn overwritten(&self) -> JsValue {
        let overwritten = if self.closed_report().is_some() {
            None
        } else {
            let state = self.state.borrow();
            match &state.resource {
                Resource::Idle(_, mapping) => mapping.overwritten(),
                _ => None,
            }
        };
        let Some((address, size)) = overwritten else {
            return JsValue::NULL;
        };
        let range = js_sys::Object::new();
        crate::error::property(&range, "address", address as f64);
        crate::error::property(&range, "size", size);
        range.into()
    }

    /// Restore the borrowed source and return the session to its slot. Calling
    /// it again after a successful close does nothing and sends no transfer.
    #[wasm_bindgen(unchecked_return_type = "Promise<void>")]
    pub fn close(&self) -> Result<js_sys::Promise, JsValue> {
        if self.closed_report().is_some() {
            return Ok(js_sys::Promise::resolve(&JsValue::UNDEFINED));
        }
        let mut lease = Lease::take(&self.state)?;
        let owner = self.session.owner.clone();
        let state = self.state.clone();
        let closed = self.closed.clone();
        let caller = js_sys::Error::new("operation started");
        Ok(wasm_bindgen_futures::future_to_promise(async move {
            match lease.restore().await {
                Ok(()) => commit(&owner, &state, &closed, &mut lease)
                    .map(|()| JsValue::UNDEFINED)
                    .map_err(|failure| error::with_caller(failure, &caller)),
                Err(failure) => {
                    // The lease drop keeps the fenced pair attached, so the
                    // report stays readable and every later request refuses.
                    drop(lease);
                    Err(error::with_caller(failure, &caller))
                }
            }
        }))
    }

    /// The loan report, plus this handle's closed state.
    #[wasm_bindgen(getter, unchecked_return_type = "MemoryReport")]
    pub fn state(&self) -> MemoryReport {
        MemoryReport::new(self.snapshot())
    }
}
