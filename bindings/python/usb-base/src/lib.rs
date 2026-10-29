//! What the USB bindings share with other binding crates: the `fujimod.usb`
//! exception types, the conversion of USB errors into them, and the hooks
//! through which a binding crate leases or runs an operation on a Python
//! `fujimod.usb.Session` without depending on the USB bindings, which install
//! the hooks when they register.
use fujimod_usb::{self as usb, ff80::Session};
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
    types::PyBytes,
};
use std::{
    any::Any,
    borrow::{Borrow, BorrowMut},
    future::Future,
    num::NonZeroU32,
    pin::Pin,
    sync::OnceLock,
};

pyo3::create_exception!(fujimod.usb, UsbError, PyRuntimeError);
pyo3::create_exception!(fujimod.usb, TransferError, UsbError);
pyo3::create_exception!(fujimod.usb, ProtocolError, UsbError);
pyo3::create_exception!(fujimod.usb, CompletionUnknownError, UsbError);
pyo3::create_exception!(fujimod.usb, DeviceCommandError, UsbError);
pyo3::create_exception!(fujimod.usb, ClosedSessionError, UsbError);
pyo3::create_exception!(fujimod.usb, SessionBusyError, UsbError);

pub fn nz(n: u32) -> PyResult<NonZeroU32> {
    NonZeroU32::new(n).ok_or_else(|| PyValueError::new_err("length/limit must be nonzero"))
}
pub fn convert_error(py: Python<'_>, error: usb::Error) -> PyErr {
    let error = match error {
        usb::Error::TestPeer(source) => match source.downcast::<PyErr>() {
            Ok(error) => return *error,
            Err(source) => usb::Error::TestPeer(source),
        },
        other => other,
    };
    let message = error.to_string();
    let exception = match &error {
        usb::Error::CompletionUnknown => CompletionUnknownError::new_err(message),
        usb::Error::DeviceStatus { .. } => DeviceCommandError::new_err(message),
        usb::Error::Protocol(_) => ProtocolError::new_err(message),
        usb::Error::Proxy(_)
        | usb::Error::Transfer { .. }
        | usb::Error::Timeout(_)
        | usb::Error::ShortTransfer { .. }
        | usb::Error::PartialRead { .. } => TransferError::new_err(message),
        usb::Error::LengthLimit { .. }
        | usb::Error::AddressOverflow
        | usb::Error::OutsideDataWindow
        | usb::Error::MisalignedDataRange { .. }
        | usb::Error::InvalidLength
        | usb::Error::InvalidToken
        | usb::Error::InvalidTimeout => PyValueError::new_err(message),
        _ => UsbError::new_err(message),
    };
    let object = exception.value(py);
    // Transfers that fail before a data completion still expose the same fields.
    if matches!(
        &error,
        usb::Error::Proxy(_)
            | usb::Error::Transfer { .. }
            | usb::Error::Timeout(_)
            | usb::Error::ShortTransfer { .. }
            | usb::Error::PartialRead { .. }
    ) {
        let _ = object.setattr("partial", PyBytes::new(py, &[]));
        let _ = object.setattr("transferred", py.None());
    }
    match error {
        usb::Error::CallbackInvocation {
            source,
            restoration,
        } => {
            let _ = object.setattr("invocation_error", convert_error(py, *source));
            let _ = object.setattr(
                "restoration_error",
                restoration.map(|e| convert_error(py, *e)),
            );
        }
        usb::Error::CallbackRestoration(source) => {
            let _ = object.setattr("restoration_error", convert_error(py, *source));
        }
        usb::Error::DeviceStatus {
            command,
            primary,
            secondary,
        } => {
            let _ = object.setattr("command", command);
            let _ = object.setattr("primary", primary);
            let _ = object.setattr("secondary", secondary);
        }
        usb::Error::Transfer {
            transferred,
            partial,
            ..
        } => {
            let _ = object.setattr("transferred", transferred);
            let _ = object.setattr("partial", PyBytes::new(py, &partial));
        }
        usb::Error::ShortTransfer {
            expected, actual, ..
        } => {
            let _ = object.setattr("expected", expected);
            let _ = object.setattr("transferred", actual);
        }
        usb::Error::PartialRead { data, source, .. } => {
            let _ = object.setattr("partial", PyBytes::new(py, &data));
            let _ = object.setattr("transfer_error", convert_error(py, *source));
        }
        _ => (),
    }
    exception
}

/// A native ff80 operation borrowing the session for its lifetime.
pub type Operation<'a, T> = Pin<Box<dyn Future<Output = usb::Result<T>> + Send + 'a>>;
/// The type-erased result of an operation run through the hooks.
pub type Output = Box<dyn Any + Send>;
/// An operation passed to the USB bindings with its result type erased.
pub type Job = Box<dyn for<'a> FnOnce(&'a mut Session) -> Operation<'a, Output> + Send>;
/// A job the USB bindings are running on a session.
pub type Running = Pin<Box<dyn Future<Output = PyResult<Output>> + Send>>;

/// An ff80 session leased from a Python USB session; dropping it ends the lease.
pub struct Lease(Box<dyn BorrowMut<Session>>);
impl Lease {
    pub fn new(lease: impl BorrowMut<Session> + 'static) -> Self {
        Self(Box::new(lease))
    }
}
impl Borrow<Session> for Lease {
    fn borrow(&self) -> &Session {
        (*self.0).borrow()
    }
}
impl BorrowMut<Session> for Lease {
    fn borrow_mut(&mut self) -> &mut Session {
        (*self.0).borrow_mut()
    }
}

/// The session hooks the USB bindings install when they register.
pub struct Sessions {
    /// Lease the open ff80 session of a Python USB session object.
    pub lease: fn(&Bound<'_, PyAny>) -> PyResult<Lease>,
    /// Run a job on a Python USB session object, as its own operations run.
    pub run: fn(&Bound<'_, PyAny>, Job) -> PyResult<Running>,
    /// The firmware's configuration command limit of a Python USB session object, in bytes.
    pub configuration_limit: fn(&Bound<'_, PyAny>) -> PyResult<u32>,
}
static SESSIONS: OnceLock<Sessions> = OnceLock::new();

/// Install the USB bindings' session hooks; later installations are ignored.
pub fn install(sessions: Sessions) {
    let _ = SESSIONS.set(sessions);
}

fn sessions() -> PyResult<&'static Sessions> {
    SESSIONS
        .get()
        .ok_or_else(|| PyRuntimeError::new_err("USB bindings are not registered"))
}

/// Lease the open ff80 session of a Python USB session object.
pub fn lease(session: &Bound<'_, PyAny>) -> PyResult<Lease> {
    (sessions()?.lease)(session)
}

/// The firmware's configuration command limit of a Python USB session object, in bytes.
pub fn configuration_limit(session: &Bound<'_, PyAny>) -> PyResult<u32> {
    (sessions()?.configuration_limit)(session)
}

/// Run a native ff80 operation on a Python USB session object.
pub async fn with<T: Send + 'static>(
    session: &Py<PyAny>,
    operation: impl for<'a> FnOnce(&'a mut Session) -> Operation<'a, T> + Send + 'static,
) -> PyResult<T> {
    let job: Job = Box::new(move |session| {
        Box::pin(async move {
            operation(session)
                .await
                .map(|value| Box::new(value) as Output)
        })
    });
    let running = Python::attach(|py| (sessions()?.run)(session.bind(py), job))?;
    let output = running.await?;
    Ok(*output
        .downcast::<T>()
        .expect("the USB bindings return the job's own result"))
}
