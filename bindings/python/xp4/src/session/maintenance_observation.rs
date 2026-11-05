//! Python conversion of a stopped session's translation-maintenance snapshot.
use fujimod_emulator_xp4_board::maintenance::{Delivery, Event, Snapshot, Status};
use pyo3::{
    prelude::*,
    types::{PyDict, PyList},
};

pub(super) fn state<'py>(py: Python<'py>, saved: Snapshot) -> PyResult<Bound<'py, PyDict>> {
    let out = PyDict::new(py);
    out.set_item(
        "identities",
        saved.identities.into_iter().map(|id| id.get()).collect::<Vec<_>>(),
    )?;
    let native = PyList::empty(py);
    for item in saved.native {
        native.append(status(py, item)?)?;
    }
    out.set_item("native", native)?;
    out.set_item("completed", saved.completed)?;
    out.set_item("delivered_peers", saved.delivered_peers)?;
    out.set_item("dropped", saved.dropped)?;
    let records = PyList::empty(py);
    for item in saved.records {
        records.append(delivery(py, item)?)?;
    }
    out.set_item("records", records)?;
    out.set_item(
        "pending",
        saved.pending.map(|item| delivery(py, item)).transpose()?,
    )?;
    out.set_item("failure", saved.failure)?;
    out.set_item("last_raw_error", saved.last_raw_error)?;
    Ok(out)
}

fn status<'py>(py: Python<'py>, saved: Status) -> PyResult<Bound<'py, PyDict>> {
    let out = PyDict::new(py);
    out.set_item("owner", saved.owner)?;
    out.set_item("next_sequence", saved.next_sequence)?;
    out.set_item("pending", saved.pending)?;
    out.set_item("poison", saved.poison)?;
    out.set_item("front", event(py, saved.front)?)?;
    out.set_item("failed", event(py, saved.failed)?)?;
    Ok(out)
}

fn event<'py>(py: Python<'py>, saved: Event) -> PyResult<Bound<'py, PyDict>> {
    let out = PyDict::new(py);
    out.set_item("owner", saved.owner)?;
    out.set_item("sequence", saved.sequence)?;
    out.set_item("vttbr", saved.vttbr)?;
    out.set_item("hcr", saved.hcr)?;
    out.set_item("cpsr", saved.cpsr)?;
    out.set_item("scr", saved.scr)?;
    out.set_item("nsacr", saved.nsacr)?;
    out.set_item("actlr", saved.actlr)?;
    out.set_item("exception_level", saved.exception_level)?;
    out.set_item("secure", saved.secure)?;
    out.set_item("selector", saved.selector)?;
    out.set_item("operand", saved.operand)?;
    Ok(out)
}

fn delivery<'py>(py: Python<'py>, saved: Delivery) -> PyResult<Bound<'py, PyDict>> {
    let out = PyDict::new(py);
    out.set_item("source_core", saved.source_core)?;
    out.set_item("source_identity", saved.source_identity.get())?;
    out.set_item("event", event(py, saved.event)?)?;
    out.set_item("peer_actlr", saved.peer_actlr)?;
    out.set_item("recipients", saved.recipients)?;
    out.set_item("applied", saved.applied)?;
    out.set_item("excluded", saved.excluded)?;
    out.set_item("acknowledged", saved.acknowledged)?;
    out.set_item("error", saved.error)?;
    out.set_item("raw_error", saved.raw_error)?;
    Ok(out)
}
