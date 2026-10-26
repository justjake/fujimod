//! Python transport for typed, observational call-recorder snapshots.
use fujimod_emulator::board::observation::calls::Snapshot;
use pyo3::{
    prelude::*,
    types::{PyDict, PyList},
};

pub fn snapshot<'py>(py: Python<'py>, saved: &Snapshot) -> PyResult<Bound<'py, PyDict>> {
    let result = PyDict::new(py);
    result.set_item("edges", &saved.edges)?;
    result.set_item("hits", &saved.hits)?;
    let states = PyList::empty(py);
    for s in &saved.states {
        let row = PyDict::new(py);
        row.set_item("callee", s.callee)?;
        row.set_item("core", s.core)?;
        row.set_item("instructions", s.instructions)?;
        row.set_item("registers", s.registers.to_vec())?;
        row.set_item("returned", s.returned)?;
        row.set_item("windows", PyList::empty(py))?;
        states.append(row)?;
    }
    result.set_item("states", states)?;
    result.set_item("dropped_edges", saved.dropped_edges)?;
    result.set_item("dropped_states", saved.dropped_states)?;
    Ok(result)
}
