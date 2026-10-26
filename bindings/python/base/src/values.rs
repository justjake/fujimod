//! Python values and refusals shared by host algorithms and the A-profile client.
use fujimod_emulator_devices::Value;
use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};

create_exception!(
    _native,
    DeviceRefusal,
    PyException,
    "A modeled device refusal (UnsupportedDevice)."
);

pub fn value_to_py<'py>(py: Python<'py>, value: &Value) -> PyResult<Bound<'py, PyAny>> {
    Ok(match value {
        Value::None => py.None().into_bound(py),
        Value::Int(v) => v.into_pyobject(py)?.into_any(),
        Value::Hex(v) => format!("{v:#x}").into_pyobject(py)?.into_any(),
        Value::Float(v) => v.into_pyobject(py)?.into_any(),
        Value::Bool(v) => pyo3::types::PyBool::new(py, *v).to_owned().into_any(),
        Value::Str(v) => v.into_pyobject(py)?.into_any(),
        Value::Static(v) => v.into_pyobject(py)?.into_any(),
        Value::List(items) => {
            let list = PyList::empty(py);
            for item in items {
                list.append(value_to_py(py, item)?)?;
            }
            list.into_any()
        }
        Value::Tuple(items) => {
            let values = items
                .iter()
                .map(|item| value_to_py(py, item))
                .collect::<PyResult<Vec<_>>>()?;
            PyTuple::new(py, values)?.into_any()
        }
        Value::Bytes(bytes) => pyo3::types::PyBytes::new(py, bytes).into_any(),
        Value::Dict(items) => {
            let dict = PyDict::new(py);
            for (key, value) in items {
                dict.set_item(value_to_py(py, key)?, value_to_py(py, value)?)?;
            }
            dict.into_any()
        }
    })
}
