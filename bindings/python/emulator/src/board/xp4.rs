//! XP4-specific host capabilities borrowed from the single Board owner.
use super::*;
use pyo3::types::PyList;

#[pyclass(unsendable, module = "fujimod._native.emulator")]
pub struct Xp4 {
    pub(super) owner: Py<Board>,
}

impl Xp4 {
    fn with<T>(
        &self,
        py: Python<'_>,
        f: impl FnOnce(&dyn fujimod_python_base::xp4_backend::Xp4Backend) -> PyResult<T>,
    ) -> PyResult<T> {
        f(self.owner.try_borrow(py)?.xp4_session()?)
    }
}

#[pymethods]
impl Xp4 {
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.owner)
    }
    #[pyo3(signature = (callback: "collections.abc.Callable[[int, int], object] | None"))]
    fn set_pc_handler(&self, py: Python<'_>, callback: Option<Py<PyAny>>) -> PyResult<()> {
        self.with(py, |s| {
            s.set_pc_handler(callback);
            Ok(())
        })
    }
    #[pyo3(signature=(executable, readable, writable, return_address, store_limit=1<<20, core=0))]
    fn driver_guard_begin(
        &self,
        py: Python<'_>,
        executable: Vec<(u64, u64)>,
        readable: Vec<(u64, u64)>,
        writable: Vec<(u64, u64)>,
        return_address: u64,
        store_limit: usize,
        core: usize,
    ) -> PyResult<()> {
        self.with(py, |s| {
            s.driver_guard_begin(
                executable,
                readable,
                writable,
                return_address,
                store_limit,
                core,
            )
        })
    }
    fn driver_guard_end(&self, py: Python<'_>) -> PyResult<()> {
        self.with(py, |s| {
            s.driver_guard_end();
            Ok(())
        })
    }
    fn driver_guard_state<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>> {
        self.with(py, |s| s.driver_guard_state(py))
    }
    fn instruction_points(&self, py: Python<'_>, core: usize, addresses: Vec<u64>) -> PyResult<()> {
        self.with(py, |s| s.instruction_points(core, addresses))
    }
    #[pyo3(signature=(entries, quota=4, edge_limit=1<<20, state_limit=1<<16, return_sites=None, quota_per_edge=false))]
    fn function_watch_begin(
        &self,
        py: Python<'_>,
        entries: Vec<u64>,
        quota: u64,
        edge_limit: usize,
        state_limit: usize,
        return_sites: Option<Vec<u64>>,
        quota_per_edge: bool,
    ) -> PyResult<()> {
        self.with(py, |s| {
            s.function_watch_begin(
                entries,
                quota,
                edge_limit,
                state_limit,
                return_sites,
                quota_per_edge,
            )
        })
    }
    fn function_watch_end(&self, py: Python<'_>) -> PyResult<()> {
        self.with(py, |s| {
            s.function_watch_end();
            Ok(())
        })
    }
    fn function_watch_state<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>> {
        self.with(py, |s| s.function_watch_state(py))
    }
    #[pyo3(signature=(ranges, limit=1_000_000))]
    fn memory_watch_begin(
        &self,
        py: Python<'_>,
        ranges: Vec<(u64, u64)>,
        limit: usize,
    ) -> PyResult<()> {
        self.with(py, |s| s.memory_watch_begin(ranges, limit))
    }
    fn memory_watch_end(&self, py: Python<'_>) -> PyResult<()> {
        self.with(py, |s| s.memory_watch_end())
    }
    fn memory_watch_enabled(&self, py: Python<'_>) -> PyResult<bool> {
        self.with(py, |s| Ok(s.memory_watch_enabled()))
    }
    fn memory_watch_state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.with(py, |s| {
            let value = s.memory_watch_state();
            py.import("json")?.call_method1("loads", (value,))
        })
    }
    fn reg_read(&self, py: Python<'_>, core: usize, register: i32) -> PyResult<u32> {
        self.with(py, |s| s.reg_read(core, register))
    }
    fn reg_write(&self, py: Python<'_>, core: usize, register: i32, value: u32) -> PyResult<()> {
        self.with(py, |s| s.reg_write(core, register, value))
    }
    fn cpr_read(
        &self,
        py: Python<'_>,
        core: usize,
        crn: u32,
        crm: u32,
        opc1: u32,
        opc2: u32,
    ) -> PyResult<u64> {
        self.with(py, |s| s.cpr_read(core, crn, crm, opc1, opc2))
    }
    fn read<'py>(
        &self,
        py: Python<'py>,
        address: usize,
        size: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        self.with(py, |s| s.read(py, address, size))
    }
    fn write(&self, py: Python<'_>, address: usize, data: &[u8]) -> PyResult<()> {
        self.with(py, |s| s.write(address, data))
    }
    fn read_virtual<'py>(
        &self,
        py: Python<'py>,
        core: usize,
        address: u32,
        size: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        self.with(py, |s| s.read_virtual(py, core, address, size))
    }
    fn run(&self, py: Python<'_>, core: usize, budget: usize) -> PyResult<()> {
        self.with(py, |s| s.run(core, budget))
    }
    fn idle(&self, py: Python<'_>) -> PyResult<bool> {
        self.with(py, |s| Ok(s.idle()))
    }
    fn set_adc_input(&self, py: Python<'_>, channel: usize, sample: u16) -> PyResult<()> {
        self.with(py, |s| s.set_adc_input(channel, sample))
    }
    fn set_gpio_input(&self, py: Python<'_>, port: usize, pin: usize, high: bool) -> PyResult<()> {
        self.with(py, |s| s.set_gpio_input(port, pin, high))
    }
    fn set_usb_connected(&self, py: Python<'_>, connected: bool) -> PyResult<()> {
        self.with(py, |s| s.set_usb_connected(connected))
    }
    fn usb_bus_reset(&self, py: Python<'_>) -> PyResult<()> {
        self.with(py, |s| s.usb_bus_reset())
    }
    #[pyo3(signature=(physical_endpoint, setup=false, out=None))]
    fn usb_transaction(
        &self,
        py: Python<'_>,
        physical_endpoint: usize,
        setup: bool,
        out: Option<Vec<u8>>,
    ) -> PyResult<u64> {
        self.with(py, |s| s.usb_transaction(physical_endpoint, setup, out))
    }
    fn usb_reply_json(&self, py: Python<'_>, id: u64) -> PyResult<Option<String>> {
        self.with(py, |s| s.usb_reply_json(id))
    }
    fn usb_consume_reply(&self, py: Python<'_>, id: u64) -> PyResult<()> {
        self.with(py, |s| s.usb_consume_reply(id))
    }
    fn scanout_frames<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        self.with(py, |s| s.scanout_frames(py))
    }
    fn device_state_json(&self, py: Python<'_>) -> PyResult<BTreeMap<String, String>> {
        self.with(py, |s| Ok(s.device_state_json()))
    }
    fn interrupt_state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        self.with(py, |s| s.interrupt_state(py))
    }
    fn device_records<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        self.with(py, |s| s.device_records(py))
    }
    #[pyo3(signature=(observations=true))]
    fn state<'py>(&self, py: Python<'py>, observations: bool) -> PyResult<Bound<'py, PyDict>> {
        self.with(py, |s| s.state(py, observations))
    }
    #[pyo3(signature=(by_count=false))]
    fn frontier(
        &self,
        py: Python<'_>,
        by_count: bool,
    ) -> PyResult<Vec<fujimod_python_base::discovery::FrontierEntry>> {
        self.with(py, |s| Ok(s.frontier(by_count)))
    }
    fn discovery_shadow(&self, py: Python<'_>) -> PyResult<Vec<(u64, u8)>> {
        self.with(py, |s| Ok(s.discovery_shadow()))
    }
    fn interventions(
        &self,
        py: Python<'_>,
    ) -> PyResult<Vec<fujimod_python_base::discovery::Intervention>> {
        self.with(py, |s| Ok(s.interventions()))
    }
}
