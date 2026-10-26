//! A scoped device capability. No Python object retains a runtime borrow.
use fujimod_emulator_devices::{
    Cx,
    exr2::{gpio::Pads, irq::Inputs, pulse::PulsePins},
    parts::pins::{Pin, Pins},
};
use fujimod_emulator_exr2_resources::Resources;
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
    types::PyBytes,
};
use std::marker::PhantomData;
use std::{cell::Cell, collections::BTreeMap, ffi::c_void, ptr::NonNull};

#[pyclass(unsendable, module = "fujimod._native.emulator")]
pub struct DeviceContext {
    active: Cell<Option<NonNull<c_void>>>,
    borrowed: Cell<bool>,
    pads: BTreeMap<String, Pads>,
    pulses: BTreeMap<String, PulsePins>,
    controllers: BTreeMap<String, Inputs>,
    bridges: BTreeMap<String, fujimod_emulator_devices::parts::lens_bridge::Registers>,
}
impl DeviceContext {
    pub fn new(
        resources: &Resources,
        references: &BTreeMap<String, String>,
        wires: &[String],
    ) -> Self {
        let mut pads = BTreeMap::new();
        let mut pulses = BTreeMap::new();
        let mut controllers = BTreeMap::new();
        for (port, name) in references {
            if let Ok(value) = resources.pads(name) {
                pads.insert(port.clone(), value);
            }
            if let Ok(value) = resources.pulses(name) {
                pulses.insert(port.clone(), value);
            }
            if let Ok(value) = resources.controller(name) {
                controllers.insert(port.clone(), value);
            }
        }
        Self {
            active: Cell::new(None),
            borrowed: Cell::new(false),
            pads,
            pulses,
            controllers,
            bridges: wires
                .iter()
                .filter_map(|name| {
                    resources
                        .bridge_registers(name)
                        .ok()
                        .map(|r| (name.clone(), r))
                })
                .collect(),
        }
    }
    fn with_cx<R>(&self, f: impl FnOnce(&mut Cx<'_>) -> PyResult<R>) -> PyResult<R> {
        let pointer = self
            .active
            .get()
            .ok_or_else(|| PyRuntimeError::new_err("device context is inactive"))?;
        if self.borrowed.replace(true) {
            return Err(PyRuntimeError::new_err("nested device-context access"));
        }
        struct Loan<'a>(&'a Cell<bool>);
        impl Drop for Loan<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _loan = Loan(&self.borrowed);
        // Scope holds the Python context alive, clears this pointer before
        // returning to its caller, and never permits nested access. R cannot
        // borrow from the context supplied to this private closure.
        f(unsafe { &mut *pointer.as_ptr().cast::<Cx<'_>>() })
    }
}
pub struct Scope<'py, 'call, 'host> {
    context: PyRef<'py, DeviceContext>,
    _loan: PhantomData<&'call mut Cx<'host>>,
}
impl<'py, 'call, 'host> Scope<'py, 'call, 'host> {
    pub fn enter(context: PyRef<'py, DeviceContext>, cx: &'call mut Cx<'host>) -> PyResult<Self> {
        if context.active.get().is_some() || context.borrowed.get() {
            return Err(PyRuntimeError::new_err("device callback is already active"));
        }
        context.active.set(Some(NonNull::from(cx).cast()));
        Ok(Self {
            context,
            _loan: PhantomData,
        })
    }
}
impl Drop for Scope<'_, '_, '_> {
    fn drop(&mut self) {
        self.context.active.set(None);
    }
}
#[pymethods]
impl DeviceContext {
    #[getter]
    fn now(&self) -> PyResult<u64> {
        self.with_cx(|cx| Ok(cx.now()))
    }
    #[getter]
    fn cpu(&self) -> PyResult<usize> {
        self.with_cx(|cx| Ok(cx.cpu))
    }
    #[getter]
    fn device(&self) -> PyResult<usize> {
        self.with_cx(|cx| Ok(cx.device))
    }
    fn ram_read<'py>(
        &self,
        py: Python<'py>,
        address: u64,
        size: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        if size > 64 << 20 {
            return Err(PyValueError::new_err("adapter DMA read exceeds 64 MiB"));
        }
        self.with_cx(|cx| {
            PyBytes::new_with(py, size, |out| {
                cx.ram_read(address, out).map_err(super::error)
            })
        })
    }
    fn ram_write(&self, address: u64, bytes: &[u8]) -> PyResult<()> {
        if bytes.len() > 64 << 20 {
            return Err(PyValueError::new_err("adapter DMA write exceeds 64 MiB"));
        }
        self.with_cx(|cx| cx.ram_write(address, bytes).map_err(super::error))
    }
    fn raise_irq(&self, core: usize, line: u32) -> PyResult<()> {
        self.with_cx(|cx| cx.raise_irq(core, line).map_err(super::error))
    }
    fn set_core_irq(&self, core: usize, line: u32, high: bool) -> PyResult<()> {
        self.with_cx(|cx| cx.set_core_irq(core, line, high).map_err(super::error))
    }
    fn reset_core(&self, core: usize, asserted: bool) -> PyResult<()> {
        self.with_cx(|cx| cx.reset_core(core, asserted).map_err(super::error))
    }
    fn schedule(&self, at: u64, token: u64) -> PyResult<()> {
        self.with_cx(|cx| {
            if at < cx.now() {
                return Err(PyValueError::new_err("device event precedes guest time"));
            }
            cx.schedule(at, token);
            Ok(())
        })
    }
    fn select_memory(&self, decoder: usize, choice: usize) -> PyResult<()> {
        self.with_cx(|cx| cx.select_memory(decoder, choice).map_err(super::error))
    }
    fn irq_configuration(&self, port: &str) -> PyResult<String> {
        let pins = self
            .controllers
            .get(port)
            .ok_or_else(|| PyValueError::new_err("adapter has no such interrupt connection"))?;
        Ok(pins.configuration().to_string())
    }
    fn gpio_input(&self, port: &str, bank: u8, pin: u8, high: bool) -> PyResult<()> {
        let mut pads = self
            .pads
            .get(port)
            .cloned()
            .ok_or_else(|| PyValueError::new_err("adapter has no such GPIO connection"))?;
        self.with_cx(|cx| {
            pads.input(cx, Pin { port: bank, pin }, high)
                .map_err(super::error)
        })
    }
    fn gpio_output(&self, port: &str, bank: u8, pin: u8) -> PyResult<Option<bool>> {
        self.with_cx(|_| {
            self.pads
                .get(port)
                .ok_or_else(|| PyValueError::new_err("adapter has no such GPIO connection"))?
                .output_level(Pin { port: bank, pin })
                .map_err(super::error)
        })
    }
    fn gpio_level(&self, port: &str, bank: u8, pin: u8) -> PyResult<bool> {
        self.with_cx(|_| {
            self.pads
                .get(port)
                .ok_or_else(|| PyValueError::new_err("adapter has no such GPIO connection"))?
                .input_level(Pin { port: bank, pin })
                .map_err(super::error)
        })
    }
    fn bridge_register(&self, port: &str, address: u8, value: u8) -> PyResult<()> {
        self.with_cx(|_| {
            self.bridges
                .get(port)
                .ok_or_else(|| PyValueError::new_err("adapter has no such bridge connection"))?
                .write_register(address, value)
                .map_err(super::error)
        })
    }
    fn counter_edges(&self, port: &str, channel: u8, count: u64) -> PyResult<()> {
        self.with_cx(|_| {
            self.pulses
                .get(port)
                .ok_or_else(|| PyValueError::new_err("adapter has no such pulse connection"))?
                .counter_edges(channel, count)
                .map_err(super::error)
        })
    }
    fn refuse(&self, reason: String) -> PyResult<()> {
        self.with_cx(|cx| {
            cx.fail(reason);
            Ok(())
        })
    }
    #[pyo3(signature = (reason=None))]
    fn unmodeled(&self, reason: Option<String>) -> PyResult<()> {
        self.with_cx(|cx| {
            cx.unsupported(reason.unwrap_or_else(|| "unmodeled register".into()));
            Ok(())
        })
    }
    fn log(&self, line: &str) -> PyResult<()> {
        self.with_cx(|cx| {
            cx.log(line);
            Ok(())
        })
    }
}
