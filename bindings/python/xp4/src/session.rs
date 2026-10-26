//! Python callbacks and value conversion for the runtime's XP4 Session.
mod driver_observation;
mod observation;
use fujimod_emulator_cpu::{Register, arm};
use fujimod_emulator_xp4_board::{Definition, Session};
use pyo3::{
    PyTraverseError, PyVisit,
    prelude::*,
    types::{PyBytes, PyDict, PyList},
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

pub struct Xp4Session {
    inner: Session,
    handler: Rc<RefCell<Option<Py<PyAny>>>>,
}

macro_rules! forward {
    ($name:ident($($argument:ident: $kind:ty),* $(,)?) -> $result:ty) => {
        pub fn $name(&self, $($argument: $kind),*) -> PyResult<$result> {
            self.inner.$name($($argument),*).map_err(fujimod_python_base::error)
        }
    };
}

impl Xp4Session {
    pub fn from_definition(definition: &str, storage_image: Option<Vec<u8>>) -> PyResult<Self> {
        let definition: Definition = serde_json::from_str(definition)
            .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
        let inner =
            Session::new(definition, storage_image).map_err(fujimod_python_base::error)?;
        let handler = Rc::new(RefCell::new(None::<Py<PyAny>>));
        let observer = handler.clone();
        inner.set_instruction_observer(Some(Rc::new(move |core, address| {
            Python::attach(|py| {
                let callback = observer
                    .borrow()
                    .as_ref()
                    .map(|callback| callback.clone_ref(py));
                if let Some(callback) = callback {
                    callback
                        .call1(py, (core, address))
                        .map_err(|error| error.to_string())?;
                }
                Ok(())
            })
        })));
        Ok(Self { inner, handler })
    }
    pub fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        if let Ok(handler) = self.handler.try_borrow()
            && let Some(handler) = handler.as_ref()
        {
            visit.call(handler)?;
        }
        Ok(())
    }
    pub fn __clear__(&self) {
        self.handler.borrow_mut().take();
    }
    pub fn set_pc_handler(&self, callback: Option<Py<PyAny>>) {
        *self.handler.borrow_mut() = callback;
    }
    pub fn ram_bytes(&self) -> usize {
        self.inner.ram_bytes()
    }
    pub fn instructions(&self) -> Vec<u64> {
        self.inner.instructions()
    }
    pub fn now(&self) -> u64 {
        self.inner.now()
    }
    pub fn quiescent(&self) -> bool {
        self.inner.quiescent()
    }
    forward!(run_board(limit: u64, quantum: usize) -> ());
    forward!(read_register(core: usize, register: Register) -> u64);
    forward!(write_register(core: usize, register: Register, value: u64) -> ());
    forward!(coprocessor(core: usize, selector: arm::Coprocessor, value: Option<u64>) -> u64);
    forward!(
        driver_guard_begin(
            executable: Vec<(u64, u64)>,
            readable: Vec<(u64, u64)>,
            writable: Vec<(u64, u64)>,
            return_address: u64,
            store_limit: usize,
            core: usize,
        ) -> ()
    );
    pub fn driver_guard_end(&self) {
        self.inner.driver_guard_end();
    }
    pub fn driver_guard_state<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>> {
        self.inner
            .driver_guard_state()
            .map(|saved| driver_observation::state(py, saved))
            .transpose()
    }
    forward!(instruction_points(core: usize, addresses: Vec<u64>) -> ());
    forward!(
        function_watch_begin(
            entries: Vec<u64>,
            quota: u64,
            edge_limit: usize,
            state_limit: usize,
            return_sites: Option<Vec<u64>>,
            quota_per_edge: bool,
        ) -> ()
    );
    pub fn function_watch_end(&self) {
        self.inner.function_watch_end();
    }
    pub fn function_watch_state<'py>(
        &self,
        py: Python<'py>,
    ) -> PyResult<Option<Bound<'py, PyDict>>> {
        self.inner
            .function_watch_state()
            .map(|saved| fujimod_python_base::observation::snapshot(py, &saved))
            .transpose()
    }
    forward!(memory_watch_begin(ranges: Vec<(u64, u64)>, limit: usize) -> ());
    forward!(memory_watch_end() -> ());
    pub fn memory_watch_enabled(&self) -> bool {
        self.inner.memory_watch_enabled()
    }
    pub fn memory_watch_state(&self) -> fujimod_emulator_xp4_board::observation::MemorySnapshot {
        self.inner.memory_watch_state()
    }
    pub fn reg_read(&self, core: usize, register: i32) -> PyResult<u32> {
        let value = self.read_register(
            core,
            client_register(register).map_err(fujimod_python_base::error)?,
        )?;
        u32::try_from(value).map_err(|_| {
            fujimod_python_base::error("ARMv7-A scalar register exceeds 32 bits".into())
        })
    }
    pub fn reg_write(&self, core: usize, register: i32, value: u32) -> PyResult<()> {
        self.write_register(
            core,
            client_register(register).map_err(fujimod_python_base::error)?,
            u64::from(value),
        )
    }
    forward!(cpr_read(core: usize, crn: u32, crm: u32, opc1: u32, opc2: u32) -> u64);
    pub fn read<'py>(
        &self,
        py: Python<'py>,
        address: usize,
        size: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        self.inner
            .validate_read(address, size)
            .map_err(fujimod_python_base::error)?;
        PyBytes::new_with(py, size, |out| {
            self.inner
                .read(address, out)
                .map_err(fujimod_python_base::error)
        })
    }
    forward!(write(address: usize, data: &[u8]) -> ());
    pub fn read_virtual<'py>(
        &self,
        py: Python<'py>,
        core: usize,
        address: u32,
        size: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let bytes = self
            .inner
            .read_virtual(core, address, size)
            .map_err(fujimod_python_base::error)?;
        Ok(PyBytes::new(py, &bytes))
    }
    forward!(run(core: usize, budget: usize) -> ());
    pub fn idle(&self) -> bool {
        self.inner.idle()
    }
    forward!(set_adc_input(channel: usize, sample: u16) -> ());
    forward!(set_gpio_input(port: usize, pin: usize, high: bool) -> ());
    forward!(set_usb_connected(connected: bool) -> ());
    forward!(usb_bus_reset() -> ());
    forward!(usb_transaction(physical_endpoint: usize, setup: bool, out: Option<Vec<u8>>) -> u64);
    forward!(usb_reply_json(id: u64) -> Option<String>);
    forward!(usb_consume_reply(id: u64) -> ());
    pub fn scanout_frames<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        observation::frames(py, self.inner.scanout_frames())
    }
    pub fn device_state_json(&self) -> BTreeMap<String, String> {
        self.inner.device_state_json()
    }
    pub fn interrupt_state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        observation::interrupts(
            py,
            self.inner
                .interrupt_state()
                .map_err(fujimod_python_base::error)?,
        )
    }
    pub fn device_records<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        observation::records(py, self.inner.device_records())
    }
    pub fn state<'py>(&self, py: Python<'py>, observations: bool) -> PyResult<Bound<'py, PyDict>> {
        observation::state(py, self.inner.state(observations))
    }
    pub fn frontier(
        &self,
        by_count: bool,
    ) -> Vec<fujimod_python_base::discovery::FrontierEntry> {
        self.inner
            .frontier(by_count)
            .into_iter()
            .map(Into::into)
            .collect()
    }
    pub fn discovery_shadow(&self) -> Vec<(u64, u8)> {
        self.inner.discovery_shadow()
    }
    pub fn interventions(&self) -> Vec<fujimod_python_base::discovery::Intervention> {
        self.inner
            .interventions()
            .into_iter()
            .map(Into::into)
            .collect()
    }
}

impl fujimod_python_base::xp4_backend::Xp4Backend for Xp4Session {
    fn run_board(&self, limit: u64, quantum: usize) -> PyResult<()> {
        Xp4Session::run_board(self, limit, quantum)
    }
    fn read_register(&self, core: usize, register: Register) -> PyResult<u64> {
        Xp4Session::read_register(self, core, register)
    }
    fn write_register(&self, core: usize, register: Register, value: u64) -> PyResult<()> {
        Xp4Session::write_register(self, core, register, value)
    }
    fn coprocessor(
        &self,
        core: usize,
        selector: arm::Coprocessor,
        value: Option<u64>,
    ) -> PyResult<u64> {
        Xp4Session::coprocessor(self, core, selector, value)
    }
    fn driver_guard_begin(
        &self,
        executable: Vec<(u64, u64)>,
        readable: Vec<(u64, u64)>,
        writable: Vec<(u64, u64)>,
        return_address: u64,
        store_limit: usize,
        core: usize,
    ) -> PyResult<()> {
        Xp4Session::driver_guard_begin(
            self,
            executable,
            readable,
            writable,
            return_address,
            store_limit,
            core,
        )
    }
    fn instruction_points(&self, core: usize, addresses: Vec<u64>) -> PyResult<()> {
        Xp4Session::instruction_points(self, core, addresses)
    }
    fn function_watch_begin(
        &self,
        entries: Vec<u64>,
        quota: u64,
        edge_limit: usize,
        state_limit: usize,
        return_sites: Option<Vec<u64>>,
        quota_per_edge: bool,
    ) -> PyResult<()> {
        Xp4Session::function_watch_begin(
            self,
            entries,
            quota,
            edge_limit,
            state_limit,
            return_sites,
            quota_per_edge,
        )
    }
    fn memory_watch_begin(&self, ranges: Vec<(u64, u64)>, limit: usize) -> PyResult<()> {
        Xp4Session::memory_watch_begin(self, ranges, limit)
    }
    fn memory_watch_end(&self) -> PyResult<()> {
        Xp4Session::memory_watch_end(self)
    }
    fn cpr_read(&self, core: usize, crn: u32, crm: u32, opc1: u32, opc2: u32) -> PyResult<u64> {
        Xp4Session::cpr_read(self, core, crn, crm, opc1, opc2)
    }
    fn write(&self, address: usize, data: &[u8]) -> PyResult<()> {
        Xp4Session::write(self, address, data)
    }
    fn run(&self, core: usize, budget: usize) -> PyResult<()> {
        Xp4Session::run(self, core, budget)
    }
    fn set_adc_input(&self, channel: usize, sample: u16) -> PyResult<()> {
        Xp4Session::set_adc_input(self, channel, sample)
    }
    fn set_gpio_input(&self, port: usize, pin: usize, high: bool) -> PyResult<()> {
        Xp4Session::set_gpio_input(self, port, pin, high)
    }
    fn set_usb_connected(&self, connected: bool) -> PyResult<()> {
        Xp4Session::set_usb_connected(self, connected)
    }
    fn usb_bus_reset(&self) -> PyResult<()> {
        Xp4Session::usb_bus_reset(self)
    }
    fn usb_transaction(
        &self,
        physical_endpoint: usize,
        setup: bool,
        out: Option<Vec<u8>>,
    ) -> PyResult<u64> {
        Xp4Session::usb_transaction(self, physical_endpoint, setup, out)
    }
    fn usb_reply_json(&self, id: u64) -> PyResult<Option<String>> {
        Xp4Session::usb_reply_json(self, id)
    }
    fn usb_consume_reply(&self, id: u64) -> PyResult<()> {
        Xp4Session::usb_consume_reply(self, id)
    }
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        Xp4Session::__traverse__(self, visit)
    }
    fn __clear__(&self) {
        Xp4Session::__clear__(self)
    }
    fn set_pc_handler(&self, callback: Option<Py<PyAny>>) {
        Xp4Session::set_pc_handler(self, callback)
    }
    fn ram_bytes(&self) -> usize {
        Xp4Session::ram_bytes(self)
    }
    fn instructions(&self) -> Vec<u64> {
        Xp4Session::instructions(self)
    }
    fn now(&self) -> u64 {
        Xp4Session::now(self)
    }
    fn quiescent(&self) -> bool {
        Xp4Session::quiescent(self)
    }
    fn driver_guard_end(&self) {
        Xp4Session::driver_guard_end(self)
    }
    fn driver_guard_state<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>> {
        Xp4Session::driver_guard_state(self, py)
    }
    fn function_watch_end(&self) {
        Xp4Session::function_watch_end(self)
    }
    fn function_watch_state<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>> {
        Xp4Session::function_watch_state(self, py)
    }
    fn memory_watch_enabled(&self) -> bool {
        Xp4Session::memory_watch_enabled(self)
    }
    fn memory_watch_state(&self) -> String {
        serde_json::to_string(&Xp4Session::memory_watch_state(self))
            .expect("XP4 memory watch state serializes")
    }
    fn reg_read(&self, core: usize, register: i32) -> PyResult<u32> {
        Xp4Session::reg_read(self, core, register)
    }
    fn reg_write(&self, core: usize, register: i32, value: u32) -> PyResult<()> {
        Xp4Session::reg_write(self, core, register, value)
    }
    fn read<'py>(
        &self,
        py: Python<'py>,
        address: usize,
        size: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        Xp4Session::read(self, py, address, size)
    }
    fn read_virtual<'py>(
        &self,
        py: Python<'py>,
        core: usize,
        address: u32,
        size: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        Xp4Session::read_virtual(self, py, core, address, size)
    }
    fn idle(&self) -> bool {
        Xp4Session::idle(self)
    }
    fn scanout_frames<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        Xp4Session::scanout_frames(self, py)
    }
    fn device_state_json(&self) -> BTreeMap<String, String> {
        Xp4Session::device_state_json(self)
    }
    fn interrupt_state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        Xp4Session::interrupt_state(self, py)
    }
    fn device_records<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        Xp4Session::device_records(self, py)
    }
    fn state<'py>(&self, py: Python<'py>, observations: bool) -> PyResult<Bound<'py, PyDict>> {
        Xp4Session::state(self, py, observations)
    }
    fn frontier(&self, by_count: bool) -> Vec<fujimod_python_base::discovery::FrontierEntry> {
        Xp4Session::frontier(self, by_count)
    }
    fn discovery_shadow(&self) -> Vec<(u64, u8)> {
        Xp4Session::discovery_shadow(self)
    }
    fn interventions(&self) -> Vec<fujimod_python_base::discovery::Intervention> {
        Xp4Session::interventions(self)
    }
}

fn client_register(id: i32) -> Result<fujimod_emulator_cpu::Register, String> {
    use fujimod_emulator_cpu::arm::Register as R;
    Ok(fujimod_emulator_cpu::Register::Arm32(match id {
        66..=78 => R::R((id - 66) as u8),
        10 => R::R(14),
        11 => R::R(15),
        12 => R::R(13),
        3 => R::Cpsr,
        13 => R::Spsr,
        4 => R::Fpexc,
        111 => R::Cpacr,
        140 => {
            return Ok(fujimod_emulator_cpu::Register::Diagnostic(
                fujimod_emulator_cpu::Diagnostic::ExceptionSyndrome,
            ));
        }
        6 => R::Fpscr,
        8 => R::Fpsid,
        14..=45 => R::D((id - 14) as u8),
        50..=65 => R::Q((id - 50) as u8),
        79..=110 => R::S((id - 79) as u8),
        _ => return Err(format!("unknown native ARM register ID {id}")),
    }))
}
