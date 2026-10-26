//! Python conversion of owned runtime observations.
use fujimod_emulator_xp4_board::observation;
use pyo3::{
    prelude::*,
    types::{PyBytes, PyDict, PyList},
};

pub(super) fn state<'py>(
    py: Python<'py>,
    saved: observation::State,
) -> PyResult<Bound<'py, PyDict>> {
    let out = PyDict::new(py);
    out.set_item("instructions", saved.instructions)?;
    out.set_item("virtual_ticks", saved.virtual_ticks)?;
    out.set_item("per_core", saved.per_core)?;
    out.set_item("waiting", saved.waiting)?;
    out.set_item("irq_entries", saved.irq_entries)?;
    out.set_item("undefined_entries", saved.undefined_entries)?;
    out.set_item("prefetch_aborts", saved.prefetch_aborts)?;
    out.set_item("data_aborts", saved.data_aborts)?;
    out.set_item("next_device_tick", saved.next_device_tick)?;
    out.set_item("eligible_irqs", saved.eligible_irqs)?;
    out.set_item("failure", saved.failure)?;
    out.set_item("cbar_reads", saved.cbar_reads)?;
    if let Some(history) = saved.history {
        out.set_item("recent", history.recent)?;
        out.set_item("milestones", history.milestones)?;
        out.set_item(
            "calls",
            history
                .calls
                .into_iter()
                .map(|call| {
                    (
                        call.core,
                        call.pc,
                        call.instructions,
                        call.lr,
                        call.sp,
                        call.args,
                        call.result,
                    )
                })
                .collect::<Vec<_>>(),
        )?;
    }
    Ok(out)
}

pub(super) fn frames<'py>(
    py: Python<'py>,
    saved: Vec<observation::Frame>,
) -> PyResult<Bound<'py, PyList>> {
    let list = PyList::empty(py);
    for frame in saved {
        let row = PyDict::new(py);
        row.set_item("output", frame.output)?;
        row.set_item("sequence", frame.sequence)?;
        row.set_item("width", frame.width)?;
        row.set_item("height", frame.height)?;
        row.set_item("rgba", PyBytes::new(py, &frame.rgba))?;
        row.set_item("state_json", frame.state_json)?;
        list.append(row)?;
    }
    Ok(list)
}

pub(super) fn interrupts<'py>(
    py: Python<'py>,
    saved: Vec<observation::InterruptState>,
) -> PyResult<Bound<'py, PyList>> {
    let list = PyList::empty(py);
    for state in saved {
        let row = PyDict::new(py);
        row.set_item("distributor_control", state.distributor_control)?;
        row.set_item("cpu_control", state.cpu_control)?;
        row.set_item("enabled", state.enabled)?;
        row.set_item("pending", state.pending)?;
        row.set_item("active", state.active)?;
        list.append(row)?;
    }
    Ok(list)
}

pub(super) fn records<'py>(
    py: Python<'py>,
    saved: Vec<fujimod_emulator_devices::Record>,
) -> PyResult<Bound<'py, PyList>> {
    let list = PyList::empty(py);
    for record in saved {
        let row = PyDict::new(py);
        row.set_item("name", record.name.as_ref())?;
        for (key, value) in record.fields.iter() {
            row.set_item(
                key.as_ref(),
                fujimod_python_base::values::value_to_py(py, value)?,
            )?;
        }
        list.append(row)?;
    }
    Ok(list)
}
