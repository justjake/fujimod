//! Offline rendering of explicit captured register banks, without CPU execution
//! or peripheral adapters. The memory reader belongs only to this host query.
use fujimod_emulator_devices::Device;
use fujimod_emulator_exr2_display::{Scanout, device::Observation};
use pyo3::{
    prelude::*,
    types::{PyBytes, PyDict},
};
#[pyfunction]
#[pyo3(signature=(options,banks,reader: "MemoryReader",instance=0,pixels=true,diagnostic_hashes=true))]
pub fn render_scanout_snapshot<'py>(
    py: Python<'py>,
    options: &str,
    banks: Vec<Vec<(u32, u32)>>,
    reader: Bound<'py, PyAny>,
    instance: usize,
    pixels: bool,
    diagnostic_hashes: bool,
) -> PyResult<
    Option<(
        Bound<'py, PyBytes>,
        Option<Bound<'py, PyBytes>>,
        String,
        usize,
        usize,
    )>,
> {
    let scanout = build(options, banks)?;
    let invalid = |e: String| pyo3::exceptions::PyValueError::new_err(e);
    let mut cause = None;
    let mut read = |address, count| {
        let kwargs = PyDict::new(py);
        let result = kwargs
            .set_item("cpu_id", 0)
            .and_then(|_| reader.call((address, count), Some(&kwargs)))
            .and_then(|v| v.extract());
        result.map_err(|e| {
            let message = e.to_string();
            cause = Some(e);
            message
        })
    };
    let frame = scanout.frame(
        instance,
        pixels,
        &mut Observation {
            read: &mut read,
            diagnostic_hashes,
        },
    );
    let frame = match frame {
        Ok(f) => f,
        Err(e) => return Err(cause.unwrap_or_else(|| invalid(e))),
    };
    frame
        .map(|f| {
            Ok((
                PyBytes::new(py, &f.raw),
                f.rgb.as_deref().map(|v| PyBytes::new(py, v)),
                serde_json::to_string(&f.state.json()).map_err(|e| invalid(e.to_string()))?,
                f.width,
                f.height,
            ))
        })
        .transpose()
}

fn build(options: &str, banks: Vec<Vec<(u32, u32)>>) -> PyResult<Scanout> {
    let invalid = |e: String| pyo3::exceptions::PyValueError::new_err(e);
    if banks.len() != 2 {
        return Err(invalid("scanout snapshot needs two register banks".into()));
    }
    let options =
        serde_json::from_str(options).map_err(|e| invalid(format!("scanout options: {e}")))?;
    let mut scanout = Scanout::new(options).map_err(invalid)?;
    let pipes:Vec<_>=banks.iter().map(|bank|serde_json::json!({"regs":bank,"written":bank.iter().map(|(o,_)|*o).collect::<Vec<_>>(),"generation":0,"reset_completions":0})).collect();
    scanout
        .restore(&serde_json::json!({"pipes":pipes,"deadlines":{}}))
        .map_err(invalid)?;
    Ok(scanout)
}
#[pyfunction]
pub fn scanout_snapshot_state(options: &str, banks: Vec<Vec<(u32, u32)>>) -> PyResult<String> {
    serde_json::to_string(&build(options, banks)?.metadata())
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
}
