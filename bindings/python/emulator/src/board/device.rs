use super::context::{DeviceContext, Scope};
use fujimod_emulator_devices::{Cx, Device, graph::Mapping};
use fujimod_emulator_exr2_plan::{Adapter, AdapterFactory};
use fujimod_emulator_exr2_resources::Resources;
use pyo3::{
    exceptions::{PyRuntimeError, PyTypeError},
    prelude::*,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    thread::{self, ThreadId},
};

#[derive(Clone, Default)]
pub struct Failures(Arc<Mutex<Option<PyErr>>>);
impl Failures {
    pub fn take(&self) -> Option<PyErr> {
        self.0.lock().unwrap().take()
    }
    pub fn remember(&self, error: PyErr) {
        let mut first = self.0.lock().unwrap();
        if first.is_none() {
            *first = Some(error);
        }
    }
}
pub struct Object {
    pub model: Py<PyAny>,
    pub context: Py<DeviceContext>,
    owner: ThreadId,
    implementation: String,
    deadline: Mutex<Option<u64>>,
}
impl Object {
    pub fn refresh_deadline(&self, py: Python<'_>, now: u64) -> PyResult<()> {
        self.owner()?;
        let deadline = self
            .model
            .bind(py)
            .call_method1("next_deadline", (now,))?
            .extract::<Option<u64>>()?;
        if deadline.is_some_and(|at| at < now) {
            return Err(PyValueError::new_err(
                "Python device deadline precedes guest time",
            ));
        }
        *self.deadline.lock().unwrap() = deadline;
        Ok(())
    }
    fn owner(&self) -> PyResult<()> {
        if self.owner != thread::current().id() {
            return Err(PyRuntimeError::new_err(
                "Python device moved from its owner thread",
            ));
        }
        Ok(())
    }
}
struct PythonDevice {
    object: Arc<Object>,
    failures: Failures,
}
impl PythonDevice {
    fn callback<R>(
        &mut self,
        cx: &mut Cx<'_>,
        f: impl FnOnce(Python<'_>, &Object) -> PyResult<R>,
    ) -> Option<R> {
        let result = Python::attach(|py| {
            self.object.owner()?;
            let scope = Scope::enter(self.object.context.bind(py).try_borrow()?, cx)?;
            let value = f(py, &self.object)?;
            drop(scope);
            self.object.refresh_deadline(py, cx.now())?;
            Ok(value)
        });
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                cx.fail(format!(
                    "Python device {}: {error}",
                    self.object.implementation
                ));
                self.failures.remember(error);
                None
            }
        }
    }
}
use pyo3::exceptions::PyValueError;
impl Device for PythonDevice {
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        self.object.owner().map_err(|e| e.to_string())?;
        Python::attach(|py| {
            let text: String = self
                .object
                .model
                .bind(py)
                .call_method0("fingerprint")?
                .extract()?;
            let state: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| PyValueError::new_err(e.to_string()))?;
            Ok::<_, PyErr>(
                serde_json::json!({"implementation":self.object.implementation,
                "deadline":*self.object.deadline.lock().unwrap(),"state":state}),
            )
        })
        .map_err(|e| e.to_string())
    }
    fn name(&self) -> &'static str {
        "python-device"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.callback(cx, |py, o| {
            o.model
                .bind(py)
                .call_method1("read", (o.context.bind(py), address, size))?
                .extract()
        })
        .unwrap_or(0)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.callback(cx, |py, o| {
            o.model
                .bind(py)
                .call_method1("write", (o.context.bind(py), address, size, value))
                .map(|_| ())
        });
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        self.callback(cx, |py, o| {
            o.model
                .bind(py)
                .call_method1("advance", (o.context.bind(py),))
                .map(|_| ())
        });
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        self.callback(cx, |py, o| {
            o.model
                .bind(py)
                .call_method1("event", (o.context.bind(py), token))
                .map(|_| ())
        });
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        *self.object.deadline.lock().unwrap()
    }
}

pub struct Adapters {
    pub pending: BTreeMap<String, Py<PyAny>>,
    pub objects: BTreeMap<String, Arc<Object>>,
    pub failures: Failures,
    pub start_tick: u64,
}
impl AdapterFactory for Adapters {
    fn construct(
        &mut self,
        name: &str,
        spec: &Adapter,
        _: &[Mapping],
        resources: &Resources,
        _: &BTreeMap<String, usize>,
    ) -> Result<Box<dyn Device>, String> {
        let result = Python::attach(|py| {
            let factory = self.pending.remove(name).ok_or_else(|| {
                PyValueError::new_err(format!("no Python device supplied for {name}"))
            })?;
            if !factory.bind(py).is_callable() {
                return Err(PyTypeError::new_err(format!(
                    "Python adapter {name} requires a constructor"
                )));
            }
            let options = py
                .import("json")?
                .call_method1("loads", (spec.options.to_string(),))?;
            let model = factory.bind(py).call1((options,))?.unbind();
            for method in [
                "read",
                "write",
                "advance",
                "event",
                "next_deadline",
                "state",
            ] {
                if !model.bind(py).getattr(method)?.is_callable() {
                    return Err(PyTypeError::new_err(format!(
                        "Python device {name}.{method} is not callable"
                    )));
                }
            }
            let context = Py::new(
                py,
                DeviceContext::new(resources, &spec.references, &spec.wires),
            )?;
            let deadline = model
                .bind(py)
                .call_method1("next_deadline", (self.start_tick,))?
                .extract::<Option<u64>>()?;
            if deadline.is_some_and(|at| at < self.start_tick) {
                return Err(PyValueError::new_err(
                    "initial Python deadline precedes guest time",
                ));
            }
            let object = Arc::new(Object {
                model,
                context,
                owner: thread::current().id(),
                implementation: spec.implementation.clone(),
                deadline: Mutex::new(deadline),
            });
            self.objects.insert(name.to_owned(), object.clone());
            Ok(Box::new(PythonDevice {
                object,
                failures: self.failures.clone(),
            }) as Box<dyn Device>)
        });
        result.map_err(|error| {
            let message = error.to_string();
            self.failures.remember(error);
            message
        })
    }
}
