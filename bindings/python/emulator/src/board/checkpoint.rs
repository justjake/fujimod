//! Thin Python handles for the shared Rust session keyframes.
use super::*;

#[pyclass(unsendable, module = "fujimod._native.emulator")]
pub struct Checkpoint {
    pub(super) saved: fujimod_emulator_session::checkpoint::Snapshot,
}
#[pymethods]
impl Checkpoint {
    #[getter]
    fn ticks(&self) -> u64 {
        self.saved.now()
    }
    #[getter]
    fn instruction_counts(&self) -> Vec<u64> {
        self.saved.instruction_counts().to_vec()
    }
}
