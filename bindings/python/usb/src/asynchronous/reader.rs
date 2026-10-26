//! Owned async range iterators. Dropping a pending next closes the reader and
//! returns the core session with its uncertain-completion fence intact.
use super::*;
pub(super) enum Operation {
    Memory(MemoryRange),
    Data(usb::operations::data::DataWindow, MemoryRange),
}
pub(super) enum NativeReader {
    Memory(usb::operations::memory::MemoryReader<ff80::Session>),
    Data(usb::operations::data::DataReader<ff80::Session>),
}
impl NativeReader {
    pub(super) fn bytes_completed(&self) -> u32 {
        match self {
            Self::Memory(r) => r.bytes_completed(),
            Self::Data(r) => r.bytes_completed(),
        }
    }
    pub(super) fn into_inner(self) -> ff80::Session {
        match self {
            Self::Memory(r) => r.into_inner(),
            Self::Data(r) => r.into_inner(),
        }
    }
    pub(super) async fn next_chunk(&mut self) -> usb::Result<Option<Vec<u8>>> {
        match self {
            Self::Memory(r) => r.next_chunk().await,
            Self::Data(r) => r.next_chunk().await,
        }
    }
}

pub(super) struct State {
    slot: ReaderSlot,
    completed: u32,
}
enum ReaderSlot {
    Idle(Box<NativeReader>),
    InFlight {
        cancel: oneshot::Sender<()>,
        waiters: Vec<oneshot::Sender<()>>,
    },
    Canceling {
        waiters: Vec<oneshot::Sender<()>>,
    },
    Closed,
}
#[pyclass(frozen, module = "fujimod.usb")]
pub(super) struct Reader {
    state: Arc<Mutex<State>>,
    owner: Arc<Mutex<Slot>>,
}
impl Reader {
    pub(super) fn new(session: &AsyncSession, operation: Operation) -> PyResult<Self> {
        let mut slot = lock(&session.inner)?;
        slot.available()?;
        let Slot::Open(native) = std::mem::replace(&mut *slot, Slot::CompletionUnknown) else {
            unreachable!()
        };
        let reader = match operation {
            Operation::Memory(range) => NativeReader::Memory(
                usb::operations::memory::MemoryReader::new(*native, range, session.limits.memory),
            ),
            Operation::Data(window, range) => NativeReader::Data(
                usb::operations::data::DataReader::new(*native, window, range)
                    .expect("window validated before taking ownership"),
            ),
        };
        let state = Arc::new(Mutex::new(State {
            slot: ReaderSlot::Idle(Box::new(reader)),
            completed: 0,
        }));
        *slot = Slot::ReadingRange(Arc::downgrade(&state));
        Ok(Self {
            state,
            owner: session.inner.clone(),
        })
    }
}
fn restore(owner: &Mutex<Slot>, reader: Box<NativeReader>) {
    if let Ok(mut slot) = owner.lock()
        && matches!(*slot, Slot::ReadingRange(_))
    {
        *slot = Slot::Open(Box::new(reader.into_inner()));
    }
}
enum Close {
    Ready(Box<NativeReader>),
    Waiting {
        cancel: Option<oneshot::Sender<()>>,
        done: oneshot::Receiver<()>,
    },
    Done,
}
fn begin_close(state: &Mutex<State>) -> PyResult<Close> {
    let mut state = lock(state)?;
    Ok(
        match std::mem::replace(&mut state.slot, ReaderSlot::Closed) {
            ReaderSlot::Idle(reader) => Close::Ready(reader),
            ReaderSlot::InFlight {
                cancel,
                mut waiters,
            } => {
                let (tx, done) = oneshot::channel();
                waiters.push(tx);
                state.slot = ReaderSlot::Canceling { waiters };
                Close::Waiting {
                    cancel: Some(cancel),
                    done,
                }
            }
            ReaderSlot::Canceling { mut waiters } => {
                let (tx, done) = oneshot::channel();
                waiters.push(tx);
                state.slot = ReaderSlot::Canceling { waiters };
                Close::Waiting { cancel: None, done }
            }
            ReaderSlot::Closed => Close::Done,
        },
    )
}
/// The session is already Abandoned before this is called. No owner lock is
/// held while requesting cancellation of a range read.
pub(super) fn abandon(state: &Mutex<State>) -> PyResult<()> {
    if let Close::Waiting {
        cancel: Some(cancel),
        ..
    } = begin_close(state)?
    {
        let _ = cancel.send(());
    }
    Ok(())
}
impl Drop for Reader {
    fn drop(&mut self) {
        match begin_close(&self.state) {
            Ok(Close::Ready(reader)) => restore(&self.owner, reader),
            Ok(Close::Waiting {
                cancel: Some(cancel),
                ..
            }) => {
                let _ = cancel.send(());
            }
            _ => (),
        }
    }
}
enum Disposition {
    Close,
    Continue,
}
struct ChunkLease {
    reader: Option<Box<NativeReader>>,
    state: Arc<Mutex<State>>,
    owner: Arc<Mutex<Slot>>,
    disposition: Disposition,
}
impl Drop for ChunkLease {
    fn drop(&mut self) {
        let Some(reader) = self.reader.take() else {
            return;
        };
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return,
        };
        state.completed = reader.bytes_completed();
        let previous = std::mem::replace(&mut state.slot, ReaderSlot::Closed);
        match previous {
            ReaderSlot::InFlight { .. } if matches!(self.disposition, Disposition::Continue) => {
                state.slot = ReaderSlot::Idle(reader);
            }
            previous => {
                let waiters = match previous {
                    ReaderSlot::InFlight { waiters, .. } | ReaderSlot::Canceling { waiters } => {
                        waiters
                    }
                    _ => vec![],
                };
                drop(state);
                restore(&self.owner, reader);
                // Wake aclose only after session ownership has been restored.
                for waiter in waiters {
                    let _ = waiter.send(());
                }
            }
        }
    }
}
#[pymethods]
impl Reader {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }
    // PyO3's async-iterator slot needs a synchronous awaitable factory.
    fn __anext__(slf: PyRef<'_, Self>) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        Ok(slf.into_pyobject(py)?.call_method0("_next_chunk")?.unbind())
    }
    async fn _next_chunk(&self) -> PyResult<Py<PyBytes>> {
        let (mut lease, cancel) = {
            let mut state = lock(&self.state)?;
            match &state.slot {
                ReaderSlot::Closed => {
                    return Err(pyo3::exceptions::PyStopAsyncIteration::new_err(()));
                }
                ReaderSlot::InFlight { .. } | ReaderSlot::Canceling { .. } => {
                    return Err(binding_error(BindingError::Busy));
                }
                ReaderSlot::Idle(_) => (),
            }
            let (tx, rx) = oneshot::channel();
            let ReaderSlot::Idle(reader) = std::mem::replace(
                &mut state.slot,
                ReaderSlot::InFlight {
                    cancel: tx,
                    waiters: vec![],
                },
            ) else {
                unreachable!()
            };
            (
                ChunkLease {
                    reader: Some(reader),
                    state: self.state.clone(),
                    owner: self.owner.clone(),
                    disposition: Disposition::Close,
                },
                rx,
            )
        };
        let reader = lease.reader.as_mut().expect("leased reader");
        let result = detached(futures_lite::future::or(
            async {
                let _ = cancel.await;
                Err(ClosedSessionError::new_err(
                    "range reader was closed or abandoned",
                ))
            },
            async { reader.next_chunk().await.map_err(error) },
        ))
        .await?;
        match result {
            Some(chunk) => {
                lease.disposition = Disposition::Continue;
                Ok(bytes(chunk))
            }
            None => Err(pyo3::exceptions::PyStopAsyncIteration::new_err(())),
        }
    }
    #[getter]
    fn bytes_completed(&self) -> PyResult<u32> {
        Ok(lock(&self.state)?.completed)
    }
    async fn aclose(&self) -> PyResult<()> {
        match begin_close(&self.state)? {
            Close::Ready(reader) => restore(&self.owner, reader),
            Close::Waiting { cancel, done } => {
                if let Some(cancel) = cancel {
                    let _ = cancel.send(());
                }
                let _ = done.await;
            }
            Close::Done => (),
        }
        Ok(())
    }
    async fn __aenter__(slf: Py<Self>) -> Py<Self> {
        slf
    }
    async fn __aexit__(
        &self,
        _kind: Py<PyAny>,
        _value: Py<PyAny>,
        _traceback: Py<PyAny>,
    ) -> PyResult<bool> {
        self.aclose().await?;
        Ok(false)
    }
}
