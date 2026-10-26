//! What the emulator bindings call on an XP4 session. `fujimod-python-xp4` implements it and
//! registers its factory, so the emulator bindings depend on no XP4 crate.
use crate::discovery::{FrontierEntry, Intervention};
use fujimod_emulator_cpu::{Register, arm};
use pyo3::{
    PyTraverseError, PyVisit,
    prelude::*,
    types::{PyBytes, PyDict, PyList},
};
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub trait Xp4Backend {
    fn run_board(&self, limit: u64, quantum: usize) -> PyResult<()>;
    fn read_register(&self, core: usize, register: Register) -> PyResult<u64>;
    fn write_register(&self, core: usize, register: Register, value: u64) -> PyResult<()>;
    fn coprocessor(
        &self,
        core: usize,
        selector: arm::Coprocessor,
        value: Option<u64>,
    ) -> PyResult<u64>;
    fn driver_guard_begin(
        &self,
        executable: Vec<(u64, u64)>,
        readable: Vec<(u64, u64)>,
        writable: Vec<(u64, u64)>,
        return_address: u64,
        store_limit: usize,
        core: usize,
    ) -> PyResult<()>;
    fn instruction_points(&self, core: usize, addresses: Vec<u64>) -> PyResult<()>;
    fn function_watch_begin(
        &self,
        entries: Vec<u64>,
        quota: u64,
        edge_limit: usize,
        state_limit: usize,
        return_sites: Option<Vec<u64>>,
        quota_per_edge: bool,
    ) -> PyResult<()>;
    fn memory_watch_begin(&self, ranges: Vec<(u64, u64)>, limit: usize) -> PyResult<()>;
    fn memory_watch_end(&self) -> PyResult<()>;
    fn cpr_read(&self, core: usize, crn: u32, crm: u32, opc1: u32, opc2: u32) -> PyResult<u64>;
    fn write(&self, address: usize, data: &[u8]) -> PyResult<()>;
    fn run(&self, core: usize, budget: usize) -> PyResult<()>;
    fn set_adc_input(&self, channel: usize, sample: u16) -> PyResult<()>;
    fn set_gpio_input(&self, port: usize, pin: usize, high: bool) -> PyResult<()>;
    fn set_usb_connected(&self, connected: bool) -> PyResult<()>;
    fn usb_bus_reset(&self) -> PyResult<()>;
    fn usb_transaction(
        &self,
        physical_endpoint: usize,
        setup: bool,
        out: Option<Vec<u8>>,
    ) -> PyResult<u64>;
    fn usb_reply_json(&self, id: u64) -> PyResult<Option<String>>;
    fn usb_consume_reply(&self, id: u64) -> PyResult<()>;
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError>;
    fn __clear__(&self);
    fn set_pc_handler(&self, callback: Option<Py<PyAny>>);
    fn ram_bytes(&self) -> usize;
    fn instructions(&self) -> Vec<u64>;
    fn now(&self) -> u64;
    fn quiescent(&self) -> bool;
    fn driver_guard_end(&self);
    fn driver_guard_state<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>>;
    fn function_watch_end(&self);
    fn function_watch_state<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>>;
    fn memory_watch_enabled(&self) -> bool;
    fn memory_watch_state(&self) -> String;
    fn reg_read(&self, core: usize, register: i32) -> PyResult<u32>;
    fn reg_write(&self, core: usize, register: i32, value: u32) -> PyResult<()>;
    fn read<'py>(
        &self,
        py: Python<'py>,
        address: usize,
        size: usize,
    ) -> PyResult<Bound<'py, PyBytes>>;
    fn read_virtual<'py>(
        &self,
        py: Python<'py>,
        core: usize,
        address: u32,
        size: usize,
    ) -> PyResult<Bound<'py, PyBytes>>;
    fn idle(&self) -> bool;
    fn scanout_frames<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>>;
    fn device_state_json(&self) -> BTreeMap<String, String>;
    fn interrupt_state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>>;
    fn device_records<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>>;
    fn state<'py>(&self, py: Python<'py>, observations: bool) -> PyResult<Bound<'py, PyDict>>;
    fn frontier(&self, by_count: bool) -> Vec<FrontierEntry>;
    fn discovery_shadow(&self) -> Vec<(u64, u8)>;
    fn interventions(&self) -> Vec<Intervention>;
}

/// Builds an XP4 session from its definition JSON and optional storage image.
pub type Xp4Factory = fn(&str, Option<Vec<u8>>) -> PyResult<Box<dyn Xp4Backend>>;
static FACTORY: OnceLock<Xp4Factory> = OnceLock::new();

/// Set the factory `Board.from_xp4` uses; later calls are ignored.
pub fn set_xp4_factory(factory: Xp4Factory) {
    FACTORY.get_or_init(|| factory);
}

pub fn open(
    definition: &str,
    storage_image: Option<Vec<u8>>,
) -> PyResult<Box<dyn Xp4Backend>> {
    let open = FACTORY.get().ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err("XP4 boards need the fujimod-python-xp4 bindings")
    })?;
    open(definition, storage_image)
}
