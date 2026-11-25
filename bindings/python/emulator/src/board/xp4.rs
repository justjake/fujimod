//! XP4 inputs and media borrowed from the single Board owner. Everything
//! else an XP4 board offers is the Board's own session interface.
use super::*;
use fujimod_python_base::xp4_backend::Opened;

#[pyclass(unsendable, module = "fujimod._native.emulator")]
pub struct Xp4 {
    pub(super) owner: Py<Board>,
}

impl Xp4 {
    fn with<T>(&self, py: Python<'_>, f: impl FnOnce(&mut Opened) -> PyResult<T>) -> PyResult<T> {
        f(self.owner.try_borrow_mut(py)?.xp4_opened()?)
    }
}

#[pymethods]
impl Xp4 {
    /// A copy of the medium in the `host`th SD card slot (SD host declaration order).
    fn card_medium(&self, py: Python<'_>, host: usize) -> PyResult<CardMedium> {
        self.with(py, |o| o.inputs.card_medium(&o.session, host).map(CardMedium))
    }
    /// Reset only program observation; unsettled NOR commands are refused.
    fn reset_nor_program_coverage(&self, py: Python<'_>) -> PyResult<()> {
        self.with(py, |o| o.inputs.reset_nor_program_coverage(&mut o.session))
    }
    /// A copy of the backed NOR medium after completed writes.
    fn nor_medium<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        self.with(py, |o| o.inputs.nor_medium(&o.session).map(|bytes| PyBytes::new(py, &bytes)))
    }
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.owner)
    }
    fn set_adc_input(&self, py: Python<'_>, channel: usize, sample: u16) -> PyResult<()> {
        self.with(py, |o| o.inputs.set_adc_input(&mut o.session, channel, sample))
    }
    fn set_gpio_input(&self, py: Python<'_>, port: usize, pin: usize, high: bool) -> PyResult<()> {
        self.with(py, |o| o.inputs.set_gpio_input(&mut o.session, port, pin, high))
    }
    fn turn_dial(&self, py: Python<'_>, name: &str, edges: i32) -> PyResult<()> {
        self.with(py, |o| o.inputs.turn_dial(&mut o.session, name, edges))
    }
    fn set_usb_connected(&self, py: Python<'_>, connected: bool) -> PyResult<()> {
        self.with(py, |o| o.inputs.set_usb_connected(&mut o.session, connected))
    }
    fn usb_bus_reset(&self, py: Python<'_>) -> PyResult<()> {
        self.with(py, |o| o.inputs.usb_bus_reset(&mut o.session))
    }
    #[pyo3(signature=(physical_endpoint, setup=false, out=None))]
    fn usb_transaction(
        &self,
        py: Python<'_>,
        physical_endpoint: usize,
        setup: bool,
        out: Option<Vec<u8>>,
    ) -> PyResult<u64> {
        self.with(py, |o| o.inputs.usb_transaction(&mut o.session, physical_endpoint, setup, out))
    }
    fn usb_reply_json(&self, py: Python<'_>, id: u64) -> PyResult<Option<String>> {
        self.with(py, |o| o.inputs.usb_reply_json(&o.session, id))
    }
    fn usb_consume_reply(&self, py: Python<'_>, id: u64) -> PyResult<()> {
        self.with(py, |o| o.inputs.usb_consume_reply(&mut o.session, id))
    }
}
