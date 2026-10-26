//! Python errors, values and hooks that the emulator binding crates share.
pub mod board_session;
pub mod boot;
pub mod discovery;
pub mod observation;
pub mod values;
pub mod xp4_backend;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

pyo3::create_exception!(fujimod._native.emulator, NativeError, PyRuntimeError);

pub fn error(message: String) -> PyErr {
    NativeError::new_err(message)
}
