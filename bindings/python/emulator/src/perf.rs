//! Python surface for the always-on performance ledger.
use fujimod_perf::{ByteCounter, GuestProgress, SpanKind};
use pyo3::prelude::*;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn error(message: impl Into<String>) -> PyErr {
    pyo3::exceptions::PyValueError::new_err(message.into())
}

#[pyfunction]
fn begin_guide(guide_id: &str) {
    fujimod_perf::begin_guide(guide_id);
}

#[pyfunction]
fn end_guide() -> PyResult<String> {
    serde_json::to_string(&fujimod_perf::end_guide())
        .map_err(|cause| error(cause.to_string()))
}

#[pyfunction]
fn record_wait_evaluation(description: &str) {
    fujimod_perf::record_wait_evaluation(description);
}

#[pyfunction]
fn record_wait_stop(description: &str, cause: &str) {
    fujimod_perf::record_wait_stop(description, cause);
}

/// Accept the existing per-core exit-counter snapshot vocabulary.
#[pyfunction]
fn set_cpu_counters(snapshot: &str) -> PyResult<()> {
    let counters: Vec<Result<BTreeMap<String, u64>, String>> =
        serde_json::from_str(snapshot).map_err(|cause| error(cause.to_string()))?;
    fujimod_perf::set_cpu_counters(counters);
    Ok(())
}

#[pyfunction]
#[pyo3(signature = (kind, wait=None, instructions=0, ticks=0))]
fn begin_span(
    kind: &str,
    wait: Option<&str>,
    instructions: u64,
    ticks: u64,
) -> PyResult<bool> {
    let kind = SpanKind::parse(kind, wait).map_err(error)?;
    Ok(fujimod_perf::begin_span(
        kind,
        GuestProgress {
            instructions,
            ticks,
        },
    ))
}

#[pyfunction]
#[pyo3(signature = (instructions=0, ticks=0, properties=Vec::new()))]
fn end_span(instructions: u64, ticks: u64, properties: Vec<(String, u64)>) {
    let properties = properties
        .iter()
        .map(|(name, value)| (name.as_str(), *value))
        .collect::<Vec<_>>();
    fujimod_perf::end_span(
        GuestProgress {
            instructions,
            ticks,
        },
        &properties,
    );
}

#[pyfunction]
fn add_bytes(counter: &str, bytes: u64) -> PyResult<()> {
    fujimod_perf::add_bytes(ByteCounter::parse(counter).map_err(error)?, bytes);
    Ok(())
}

#[pyfunction]
fn write_chrome_trace(path: PathBuf) -> PyResult<()> {
    fujimod_perf::write_chrome_trace(&path).map_err(|cause| error(cause.to_string()))
}

#[pymodule(name = "perf", module = "fujimod._native", submodule, gil_used = true)]
mod binding {
    #[pymodule_export]
    use super::{
        add_bytes, begin_guide, begin_span, end_guide, end_span, record_wait_evaluation,
        record_wait_stop, set_cpu_counters,
        write_chrome_trace,
    };
}

pub fn register(py: Python<'_>, parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let module = binding::_PYO3_DEF.make_module(py)?;
    parent.add_submodule(module.bind(py))?;
    let modules = py
        .import("sys")?
        .getattr("modules")?
        .cast_into::<pyo3::types::PyDict>()?;
    modules.set_item("fujimod._native.perf", &module)?;
    Ok(())
}
