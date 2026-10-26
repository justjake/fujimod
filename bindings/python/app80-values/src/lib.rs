//! Python value types for firmware-qualified commands in `fujimod.usb.app80`.
use pyo3::{
    exceptions::PyValueError,
    prelude::*,
    types::{PyBytes, PyModule},
};

#[pyclass(frozen, eq, hash, from_py_object, module = "fujimod.usb.app80")]
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct CommandCode {
    pub inner: fujimod_usb_app80_types::CommandCode,
}
#[pymethods]
impl CommandCode {
    #[new]
    fn new(value: u8) -> Self {
        Self {
            inner: fujimod_usb_app80_types::CommandCode(value),
        }
    }
    #[getter]
    fn value(&self) -> u8 {
        self.inner.0
    }
}

pub mod catalog;

pub fn register(py: Python<'_>, module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<CommandCode>()?;
    catalog::register(py, module)
}
