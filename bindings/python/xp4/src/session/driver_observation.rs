//! Python conversion of a paused A32 caller observation.
use fujimod_emulator::board::harness::arm32::Snapshot;
use pyo3::{prelude::*, types::PyDict};

pub(super) fn state<'py>(py: Python<'py>, saved: Snapshot) -> PyResult<Bound<'py, PyDict>> {
    let out = PyDict::new(py);
    out.set_item("core", saved.core)?;
    out.set_item("executable", saved.executable)?;
    out.set_item("readable", saved.readable)?;
    out.set_item("writable", saved.writable)?;
    out.set_item("return_address", saved.return_address)?;
    out.set_item("store_limit", saved.store_limit)?;
    out.set_item("stores", saved.stores)?;
    out.set_item("returned", saved.returned)?;
    out.set_item("failure", saved.failure)?;
    out.set_item("accepted_instructions", saved.accepted_instructions)?;
    out.set_item("scope", saved.scope)?;
    Ok(out)
}
