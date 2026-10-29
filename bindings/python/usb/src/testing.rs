//! Optional synthetic exchanges; never exposes a production connection path.
use super::*;

#[pyclass(frozen, from_py_object, module = "fujimod.usb.testing")]
#[derive(Clone)]
struct Exchange {
    inner: usb::testing::Exchange,
}
#[pymethods]
impl Exchange {
    #[staticmethod]
    fn control(bytes: Vec<u8>) -> PyResult<Self> {
        let bytes = bytes
            .try_into()
            .map_err(|_| PyValueError::new_err("control envelope must have 12 bytes"))?;
        Ok(Self {
            inner: usb::testing::Exchange::Control(bytes),
        })
    }
    #[staticmethod]
    fn control_disconnected(bytes: Vec<u8>) -> PyResult<Self> {
        let bytes = bytes
            .try_into()
            .map_err(|_| PyValueError::new_err("control envelope must have 12 bytes"))?;
        Ok(Self {
            inner: usb::testing::Exchange::ControlFailure {
                bytes,
                error: nusb::transfer::TransferError::Disconnected,
            },
        })
    }
    #[staticmethod]
    fn write(bytes: Vec<u8>) -> Self {
        Self {
            inner: usb::testing::Exchange::Write(bytes),
        }
    }
    #[staticmethod]
    fn read(maximum: u32, data: Vec<u8>) -> Self {
        Self {
            inner: usb::testing::Exchange::Read {
                maximum: maximum as usize,
                data,
            },
        }
    }
    #[staticmethod]
    fn pending_read(maximum: u32) -> Self {
        Self {
            inner: usb::testing::Exchange::PendingRead {
                maximum: maximum as usize,
            },
        }
    }
    #[staticmethod]
    fn delayed_read(maximum: u32, data: Vec<u8>, delay_ms: u64) -> Self {
        Self {
            inner: usb::testing::Exchange::DelayedRead {
                maximum: maximum as usize,
                data,
                delay: Duration::from_millis(delay_ms),
            },
        }
    }
    #[staticmethod]
    fn disconnected(maximum: u32, partial: Vec<u8>) -> Self {
        Self {
            inner: usb::testing::Exchange::ReadFailure {
                maximum: maximum as usize,
                error: nusb::transfer::TransferError::Disconnected,
                partial,
            },
        }
    }
}
#[pyclass(frozen, module = "fujimod.usb.testing")]
struct Script {
    inner: usb::testing::Script,
}
#[pymethods]
impl Script {
    #[getter]
    fn remaining(&self) -> usize {
        self.inner.remaining()
    }
}
#[pyfunction]
#[pyo3(signature=(exchanges,*,max_reply_bytes=8388608))]
fn connection(
    exchanges: Vec<Exchange>,
    max_reply_bytes: u32,
) -> PyResult<(asynchronous::connection::AsyncConnection, Script)> {
    let script = usb::testing::Script::new(exchanges.into_iter().map(|e| e.inner));
    let native = script
        .connection(max_reply_bytes)
        .map_err(asynchronous::error)?;
    Ok((
        asynchronous::connection::AsyncConnection::new(native),
        Script { inner: script },
    ))
}
#[pyfunction]
#[pyo3(signature=(exchanges,limits,*,max_reply_bytes=8388608,trace=None))]
async fn session(
    exchanges: Vec<Exchange>,
    limits: FirmwareLimits,
    max_reply_bytes: u32,
    trace: Option<tracing::Trace>,
) -> PyResult<(asynchronous::AsyncSession, Script)> {
    let script = usb::testing::Script::new(exchanges.into_iter().map(|e| e.inner));
    let native =
        asynchronous::detached(script.begin_session(max_reply_bytes, trace.map(|t| t.inner)))
            .await
            .map_err(asynchronous::error)?;
    Ok((
        asynchronous::AsyncSession::new(native, limits),
        Script { inner: script },
    ))
}
/// Test-only callback owner. No Python peer can be installed into a production
/// connection; this backend is absent without the testing feature.
struct PythonPeer {
    inner: Py<PyAny>,
    ep_in: u8,
    ep_out: u8,
}
impl PythonPeer {
    fn no_result(&self, method: &str, data: &[u8]) -> usb::Result<()> {
        Python::attach(|py| {
            let result = self
                .inner
                .call_method1(py, method, (PyBytes::new(py, data),))?;
            if !result.is_none(py) {
                return Err(pyo3::exceptions::PyTypeError::new_err(
                    "virtual USB control/write must return None",
                ));
            }
            Ok(())
        })
        .map_err(|error: PyErr| usb::Error::TestPeer(Box::new(error)))
    }
}
impl usb::testing::Peer for PythonPeer {
    fn control(
        &mut self,
        _kind: usb::ff80::framing::RequestKind,
        envelope: [u8; 12],
    ) -> usb::Result<()> {
        self.no_result("control", &envelope)
    }
    fn write(&mut self, data: &[u8]) -> usb::Result<()> {
        self.no_result("write", data)
    }
    fn read(&mut self, maximum: usize) -> usb::Result<Vec<u8>> {
        Python::attach(|py| {
            let result = self.inner.call_method1(py, "read", (maximum,))?;
            Ok(result.bind(py).cast::<PyBytes>()?.as_bytes().to_vec())
        })
        .map_err(|error: PyErr| usb::Error::TestPeer(Box::new(error)))
    }
    fn endpoint(&self, stage: usb::error::TransferStage) -> Option<u8> {
        Some(match stage {
            usb::error::TransferStage::Control(_) => 0,
            usb::error::TransferStage::BulkIn => self.ep_in,
            usb::error::TransferStage::BulkOut => self.ep_out,
        })
    }
}

/// Callbacks synchronously advance a virtual camera, never a physical device.
/// control(envelope: bytes)/write(data: bytes) return None; read(maximum: int)
/// returns bytes. They must not call back into this session or its readers.
#[pyfunction]
#[pyo3(signature=(peer,limits,*,ep_in,ep_out,max_reply_bytes=8388608,trace=None))]
async fn peer_session(
    peer: Py<PyAny>,
    limits: FirmwareLimits,
    ep_in: u8,
    ep_out: u8,
    max_reply_bytes: u32,
    trace: Option<tracing::Trace>,
) -> PyResult<asynchronous::AsyncSession> {
    nz(max_reply_bytes)?;
    if !(0x81..=0x8f).contains(&ep_in) || !(1..=0x0f).contains(&ep_out) {
        return Err(PyValueError::new_err(
            "require explicit bulk IN/OUT endpoint addresses",
        ));
    }
    Python::attach(|py| -> PyResult<()> {
        for method in ["control", "write", "read"] {
            if !peer.bind(py).getattr(method)?.is_callable() {
                return Err(pyo3::exceptions::PyTypeError::new_err(
                    "virtual USB methods must be callable",
                ));
            }
        }
        Ok(())
    })?;
    let native = asynchronous::detached(usb::testing::peer_session(
        PythonPeer {
            inner: peer,
            ep_in,
            ep_out,
        },
        max_reply_bytes,
        trace.map(|t| t.inner),
    ))
    .await
    .map_err(asynchronous::error)?;
    Ok(asynchronous::AsyncSession::new(native, limits))
}
pub(crate) fn module(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    let module = PyModule::new(py, "fujimod.usb.testing")?;
    module.add_class::<Exchange>()?;
    module.add_class::<Script>()?;
    module.add_function(wrap_pyfunction!(session, &module)?)?;
    module.add_function(wrap_pyfunction!(connection, &module)?)?;
    module.add_function(wrap_pyfunction!(peer_session, &module)?)?;
    Ok(module)
}
