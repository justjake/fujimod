//! Python drives these local Rust futures without moving board owners to threads.
use super::*;
pub(super) enum Output {
    Unit,
    RemoteReadResult(RemoteReadResult),
    Readiness(Readiness),
    MemoryReads(Vec<MemoryRead>),
    Capture(Capture),
}
#[pyclass(unsendable, module = "fujimod._native.remote")]
pub(super) struct Task {
    future: Option<LocalFuture<'static, Output>>,
    result: Option<Result<Output>>,
}
struct PythonWake(Py<PyAny>);
impl Wake for PythonWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        Python::attach(|py| {
            if let Err(error) = self.0.bind(py).call0() {
                error.write_unraisable(py, Some(self.0.bind(py)));
            }
        });
    }
}
impl Task {
    pub(super) fn new(future: LocalFuture<'static, Output>) -> Self {
        Self {
            future: Some(future),
            result: None,
        }
    }
}
#[pymethods]
impl Task {
    fn poll(&mut self, wake: Py<PyAny>) -> bool {
        let Some(future) = &mut self.future else {
            return true;
        };
        let waker = Waker::from(Arc::new(PythonWake(wake)));
        match future.as_mut().poll(&mut Context::from_waker(&waker)) {
            std::task::Poll::Pending => false,
            std::task::Poll::Ready(value) => {
                self.result = Some(value);
                self.future = None;
                true
            }
        }
    }
    fn blocking(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        if let Some(future) = self.future.take() {
            self.result = Some(futures_lite::future::block_on(future));
        }
        self.result(py)
    }
    fn cancel(&mut self) {
        self.future = None;
    }
    fn result(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let result = self
            .result
            .take()
            .ok_or_else(|| error("Task has no unconsumed result".into()))?
            .map_err(error)?;
        match result {
            Output::Unit => Ok(py.None()),
            Output::RemoteReadResult(read_result) => Ok(pyo3::types::PyString::new(
                py,
                &serde_json::to_string(&read_result).map_err(|e| error(e.to_string()))?,
            )
            .into_any()
            .unbind()),
            Output::Readiness(readiness) => Ok(pyo3::types::PyString::new(
                py,
                &serde_json::to_string(&readiness).map_err(|e| error(e.to_string()))?,
            )
            .into_any()
            .unbind()),
            Output::MemoryReads(reads) => {
                let values = pyo3::types::PyList::empty(py);
                for read in reads {
                    values.append((read.address, PyBytes::new(py, &read.bytes)))?;
                }
                Ok(values.into_any().unbind())
            }
            Output::Capture(capture) => Ok(PyTuple::new(
                py,
                [
                    PyBytes::new(py, capture.frame.png().map_err(error)?)
                        .into_any()
                        .unbind(),
                    capture.time.into_pyobject(py)?.into_any().unbind(),
                    match capture.clock {
                        Clock::GuestTicks => "guest_ticks",
                        Clock::WallNanoseconds => "wall_nanoseconds",
                    }
                    .into_pyobject(py)?
                    .into_any()
                    .unbind(),
                    capture.output.into_pyobject(py)?.into_any().unbind(),
                    capture.sequence.into_pyobject(py)?.into_any().unbind(),
                ],
            )?
            .into_any()
            .unbind()),
        }
    }
}
