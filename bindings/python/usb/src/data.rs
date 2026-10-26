//! Composed E1 range operations with an explicit firmware mapping.
use super::*;

#[pyclass(module = "fujimod.usb", frozen, from_py_object)]
#[derive(Clone)]
pub(super) struct DataWindow {
    pub(super) inner: usb::operations::data::DataWindow,
}
#[pymethods]
impl DataWindow {
    #[new]
    fn new(
        py: Python<'_>,
        base: u32,
        address: u32,
        length: u32,
        capacity: u32,
        offset_mode: OffsetMode,
    ) -> PyResult<Self> {
        let aperture =
            MemoryRange::new(Address(address), nz(length)?).map_err(|e| convert_error(py, e))?;
        let inner = usb::operations::data::DataWindow::new(
            Address(base),
            aperture,
            nz(capacity)?,
            offset_mode.into(),
        )
        .map_err(|e| convert_error(py, e))?;
        Ok(Self { inner })
    }
    #[getter]
    fn capacity(&self) -> u32 {
        self.inner.capacity().get()
    }
}

#[pyclass(frozen, module = "fujimod.usb.data")]
pub(super) struct UploadReply {
    pub(super) inner: usb::operations::data::UploadReply,
}
#[pymethods]
impl UploadReply {
    #[getter]
    fn setup(&self) -> DiagnosticReply {
        DiagnosticReply {
            inner: self.inner.setup.clone(),
        }
    }
    #[getter]
    fn acknowledgment<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.inner.acknowledgment)
    }
    #[getter]
    fn bytes_written(&self) -> u32 {
        self.inner.bytes_written
    }
}
