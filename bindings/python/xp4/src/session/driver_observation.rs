//! Python conversion of a paused A32 caller observation.
use fujimod_emulator::board::harness::arm32::Snapshot;
use pyo3::{
    prelude::*,
    types::{PyDict, PyList},
};

pub(super) fn state<'py>(py: Python<'py>, saved: Snapshot) -> PyResult<Bound<'py, PyDict>> {
    let out = PyDict::new(py);
    out.set_item("core", saved.core)?;
    out.set_item("executable", saved.executable)?;
    out.set_item("readable", saved.readable)?;
    out.set_item("writable", saved.writable)?;
    out.set_item("return_address", saved.return_address)?;
    out.set_item("store_limit", saved.store_limit)?;
    out.set_item("stores", saved.stores)?;
    out.set_item("model_readable", saved.model_readable)?;
    out.set_item("model_writable", saved.model_writable)?;
    out.set_item("model_writes", saved.model_writes)?;
    let mmio = PyList::empty(py);
    for entry in saved.mmio {
        let policy = PyDict::new(py);
        policy.set_item("start", entry.start)?;
        policy.set_item("end", entry.end)?;
        policy.set_item("width", entry.width)?;
        policy.set_item("read", entry.read)?;
        policy.set_item("write", entry.write)?;
        mmio.append(policy)?;
    }
    out.set_item("mmio", mmio)?;
    out.set_item("mmio_accesses", saved.mmio_accesses)?;
    out.set_item("returned", saved.returned)?;
    out.set_item("failure", saved.failure)?;
    out.set_item("accepted_instructions", saved.accepted_instructions)?;
    out.set_item("scope", saved.scope)?;
    Ok(out)
}
