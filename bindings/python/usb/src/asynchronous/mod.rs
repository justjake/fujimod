//! Native asyncio adapters. Own the core session across await, never a mutex guard.
use super::*;
use futures_channel::oneshot;
use std::{
    future::Future,
    pin::Pin,
    sync::MutexGuard,
    task::{Context, Poll},
};

mod callback;
mod commands;
pub(super) mod connection;
mod install;
mod operations;
mod reader;
mod runtime;
mod sequence;
mod workflows;

/// Poll native USB without holding Python's interpreter lock. PyO3 supplies the
/// asyncio coroutine/waker; nusb supplies native I/O wakeups. No worker runtime.
struct Detached<F>(Pin<Box<F>>);
impl<F: Future + Send> Future for Detached<F>
where
    F::Output: Send,
{
    type Output = F::Output;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let waker = cx.waker();
        Python::attach(|py| py.detach(|| self.0.as_mut().poll(&mut Context::from_waker(waker))))
    }
}
pub(super) async fn detached<F: Future + Send>(future: F) -> F::Output
where
    F::Output: Send,
{
    Detached(Box::pin(future)).await
}
pub(super) fn error(error: usb::Error) -> PyErr {
    Python::attach(|py| convert_error(py, error))
}
fn binding_error(error: BindingError) -> PyErr {
    Python::attach(|py| error.into_python(py))
}
fn lock<T>(mutex: &Mutex<T>) -> PyResult<MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(|_| binding_error(BindingError::Poisoned))
}
/// Python bytes of a native buffer.
pub fn bytes(data: impl AsRef<[u8]>) -> Py<PyBytes> {
    Python::attach(|py| PyBytes::new(py, data.as_ref()).unbind())
}
use fujimod_python_usb_base::{Operation, Running, Sessions};

enum Slot {
    Open(Box<ff80::Session>),
    InFlight(oneshot::Sender<()>),
    ReadingRange(std::sync::Weak<Mutex<reader::State>>),
    Closed,
    CompletionUnknown,
    Abandoned,
}
impl Slot {
    fn available(&self) -> PyResult<()> {
        match self {
            Self::Open(s) if s.state() == ff80::SessionState::Ready => Ok(()),
            Self::Open(_) | Self::CompletionUnknown => {
                Err(binding_error(BindingError::CompletionUnknown))
            }
            Self::InFlight(_) | Self::ReadingRange(_) => Err(binding_error(BindingError::Busy)),
            Self::Closed => Err(binding_error(BindingError::Closed)),
            Self::Abandoned => Err(binding_error(BindingError::Abandoned)),
        }
    }
}
#[pyclass(name = "Session", frozen, from_py_object, module = "fujimod.usb")]
#[derive(Clone)]
pub struct AsyncSession {
    inner: Arc<Mutex<Slot>>,
    limits: FirmwareLimits,
    token: u16,
    packet_bytes: u32,
}
impl AsyncSession {
    /// The firmware's configuration command limit in bytes.
    pub fn configuration_limit(&self) -> u32 {
        self.limits.configuration.get()
    }
    pub(super) fn new(native: ff80::Session, limits: FirmwareLimits) -> Self {
        Self {
            token: native.info().id,
            packet_bytes: native.info().advertised_packet_bytes.get(),
            inner: Arc::new(Mutex::new(Slot::Open(Box::new(native)))),
            limits,
        }
    }
    /// Run a native ff80 operation on the open session.
    pub async fn with<T: Send>(
        &self,
        operation: impl for<'a> FnOnce(&'a mut ff80::Session) -> Operation<'a, T> + Send,
    ) -> PyResult<T> {
        let (mut lease, cancel) = Lease::new(self)?;
        detached(async {
            let Resource::Open(native) = &mut lease.resource else {
                unreachable!()
            };
            futures_lite::future::or(
                async {
                    let _ = cancel.await;
                    Err(binding_error(BindingError::Abandoned))
                },
                async { operation(native).await.map_err(error) },
            )
            .await
        })
        .await
    }
}
/// Closing consumes the native session. A dropped close can only restore an
/// unknown terminal state; an ordinary operation restores the fenced core.
enum Resource {
    Open(Box<ff80::Session>),
    Closing,
    Closed,
}
pub struct Lease {
    owner: Arc<Mutex<Slot>>,
    resource: Resource,
}
impl Lease {
    fn new(session: &AsyncSession) -> PyResult<(Self, oneshot::Receiver<()>)> {
        let mut slot = lock(&session.inner)?;
        slot.available()?;
        let (tx, rx) = oneshot::channel();
        let Slot::Open(native) = std::mem::replace(&mut *slot, Slot::InFlight(tx)) else {
            unreachable!()
        };
        Ok((
            Self {
                owner: session.inner.clone(),
                resource: Resource::Open(native),
            },
            rx,
        ))
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.owner.lock()
            && matches!(*slot, Slot::InFlight(_))
        {
            *slot = match std::mem::replace(&mut self.resource, Resource::Closing) {
                Resource::Open(native) => Slot::Open(native),
                Resource::Closing => Slot::CompletionUnknown,
                Resource::Closed => Slot::Closed,
            };
        }
    }
}
#[pyfunction]
async fn discover(filter: DeviceFilter) -> PyResult<Vec<Device>> {
    detached(usb::connection::discover(filter.into()))
        .await
        .map(|items| items.into_iter().map(|inner| Device { inner }).collect())
        .map_err(error)
}
#[pyfunction]
async fn discover_proxy(url: String, token: String, filter: DeviceFilter) -> PyResult<Vec<Device>> {
    detached(usb::connection::discover_proxy(&url, &token, filter.into()))
        .await
        .map(|items| items.into_iter().map(|inner| Device { inner }).collect())
        .map_err(error)
}
#[pyfunction]
async fn interfaces(device: Device) -> PyResult<Vec<InterfaceSelection>> {
    detached(usb::connection::interfaces(&device.inner))
        .await
        .map(|items| {
            items
                .into_iter()
                .map(|inner| InterfaceSelection { inner })
                .collect()
        })
        .map_err(error)
}
pub(super) fn register(py: Python<'_>, module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<AsyncSession>()?;
    callback::register(module)?;
    runtime::register(module)?;
    connection::register(module)?;
    module.add_class::<sequence::CommandSequence>()?;
    install::register(module)?;
    module.add_class::<reader::Reader>()?;
    module.add_function(wrap_pyfunction!(self::discover, module)?)?;
    module.add_function(wrap_pyfunction!(self::discover_proxy, module)?)?;
    module.add_function(wrap_pyfunction!(self::interfaces, module)?)?;
    operations::register(py, module)
}

/// Lend Python `fujimod.usb.Session` objects to the binding crates built on
/// `fujimod-python-usb-base`.
pub(super) fn install() {
    fujimod_python_usb_base::install(Sessions {
        lease: |session| {
            let session = session.cast::<AsyncSession>()?.get();
            Lease::new(session).map(|(lease, _)| fujimod_python_usb_base::Lease::new(lease))
        },
        run: |session, operation| {
            let session = session.cast::<AsyncSession>()?.get().clone();
            Ok(Box::pin(async move { session.with(operation).await }) as Running)
        },
        configuration_limit: |session| {
            Ok(session.cast::<AsyncSession>()?.get().configuration_limit())
        },
    });
}
impl std::borrow::Borrow<ff80::Session> for Lease {
    fn borrow(&self) -> &ff80::Session {
        match &self.resource {
            Resource::Open(session) => session,
            _ => unreachable!("Remote lease retains an open session"),
        }
    }
}
impl std::borrow::BorrowMut<ff80::Session> for Lease {
    fn borrow_mut(&mut self) -> &mut ff80::Session {
        let available = self
            .owner
            .lock()
            .is_ok_and(|slot| matches!(*slot, Slot::InFlight(_)));
        match &mut self.resource {
            Resource::Open(session) => {
                if !available {
                    session.invalidate_completion();
                }
                session
            }
            _ => unreachable!("Remote lease retains an open session"),
        }
    }
}
