//! Thin Python handles for the shared Rust session keyframes.
use super::*;

#[pyclass(unsendable, module = "fujimod._native.emulator")]
pub struct Checkpoint {
    pub(super) saved: Option<fujimod_emulator_session::checkpoint::Snapshot>,
    pub(super) bytes: Option<Vec<u8>>,
    pub(super) ticks: u64,
    pub(super) instruction_counts: Vec<u64>,
}
#[pymethods]
impl Checkpoint {
    #[getter]
    fn ticks(&self) -> u64 {
        self.ticks
    }
    #[getter]
    fn instruction_counts(&self) -> Vec<u64> {
        self.instruction_counts.clone()
    }
    fn to_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        if let Some(bytes) = &self.bytes {
            return Ok(PyBytes::new(py, bytes));
        }
        let bytes = self
            .saved
            .as_ref()
            .ok_or_else(|| PyValueError::new_err("checkpoint has no saved state"))?
            .export()
            .map_err(error)?;
        Ok(PyBytes::new(py, &bytes))
    }
    #[staticmethod]
    fn from_bytes(bytes: &Bound<'_, PyBytes>) -> PyResult<Self> {
        let bytes = bytes.as_bytes().to_vec();
        let (ticks, instruction_counts) =
            fujimod_emulator_session::checkpoint::Snapshot::inspect(&bytes).map_err(error)?;
        Ok(Self {
            saved: None,
            bytes: Some(bytes),
            ticks,
            instruction_counts,
        })
    }
}
