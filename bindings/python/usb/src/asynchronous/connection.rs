//! Explicit pre-session connection ownership, including stale-session reclaim.
use super::*;

#[pyclass(name = "Connection", frozen, module = "fujimod.usb")]
pub(crate) struct AsyncConnection {
    inner: Mutex<Option<Connection>>,
}
impl AsyncConnection {
    pub(crate) fn new(inner: Connection) -> Self {
        Self {
            inner: Mutex::new(Some(inner)),
        }
    }
    fn take(&self) -> PyResult<Connection> {
        lock(&self.inner)?
            .take()
            .ok_or_else(|| binding_error(BindingError::Closed))
    }
}
#[pymethods]
impl AsyncConnection {
    #[staticmethod]
    #[pyo3(signature=(device,interface,*,timeout_ms=5000,max_reply_bytes=8388608))]
    async fn open(
        device: Device,
        interface: InterfaceSelection,
        timeout_ms: u64,
        max_reply_bytes: u32,
    ) -> PyResult<Self> {
        let options = TransferOptions {
            timeout: Duration::from_millis(timeout_ms),
            max_reply_bytes: nz(max_reply_bytes)?,
        };
        detached(Connection::open(&device.inner, interface.inner, options))
            .await
            .map(Self::new)
            .map_err(error)
    }
    async fn reclaim_session(&self) -> PyResult<bool> {
        let mut native = self.take()?;
        let result = detached(native.reclaim_session()).await;
        *lock(&self.inner)? = Some(native);
        result.map_err(error)
    }
    #[pyo3(signature=(limits,*,trace=None))]
    async fn begin_session(
        &self,
        limits: FirmwareLimits,
        trace: Option<tracing::Trace>,
    ) -> PyResult<AsyncSession> {
        let native = self.take()?;
        detached(native.begin_session(trace.map(|t| t.inner)))
            .await
            .map(|native| AsyncSession::new(native, limits))
            .map_err(error)
    }
}
pub(super) fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<AsyncConnection>()
}
