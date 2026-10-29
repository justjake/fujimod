//! File-based construction and synchronous execution on the Python owner thread.
use super::*;
use fujimod_python_base::boot::{Inputs, boot};
use std::{
    marker::PhantomData,
    ptr::NonNull,
    thread::{self, ThreadId},
};

struct Owner<'a> {
    session: NonNull<fujimod_emulator_session::Session>,
    thread: ThreadId,
    _loan: PhantomData<&'a mut fujimod_emulator_session::Session>,
}
// This is a synchronous, lifetime-bound detach capability, not a Send Session.
// Dereferencing is guarded by the original thread ID and Drop never uses it.
unsafe impl Send for Owner<'_> {}
impl<'a> Owner<'a> {
    fn new(session: &'a mut fujimod_emulator_session::Session) -> Self {
        Self {
            session: NonNull::from(session),
            thread: thread::current().id(),
            _loan: PhantomData,
        }
    }
    fn run(mut self, limit: u64) -> PyResult<String> {
        if self.thread != thread::current().id() {
            return Err(error("session execution moved from its owner thread".into()));
        }
        boot()?
            .to_rec(unsafe { self.session.as_mut() }, limit)
            .map_err(error)
    }
}

#[pymethods]
impl Board {
    #[staticmethod]
    #[pyo3(signature=(*, configuration_id, update, boot_rom, overlays=Vec::new(), nand=None, settings=None, clock_seconds_2000=None, preferred_languages=Vec::new(), camera=None))]
    fn from_inputs(
        configuration_id: String,
        update: PathBuf,
        boot_rom: PathBuf,
        overlays: Vec<String>,
        nand: Option<PathBuf>,
        settings: Option<PathBuf>,
        clock_seconds_2000: Option<u32>,
        preferred_languages: Vec<String>,
        camera: Option<PathBuf>,
    ) -> PyResult<Self> {
        let session = boot()?
            .open(Inputs {
            configuration_id,
            overlays,
            update,
            boot_rom,
            nand,
            settings,
            clock_seconds_2000,
            preferred_languages,
            camera,
            })
            .map_err(error)?;
        let graph: factory::Graph = serde_json::from_value(session.definition["execution"].clone())
            .map_err(|e| error(e.to_string()))?;
        let read_limit = graph
            .ram
            .iter()
            .map(|r| r.size)
            .chain(
                graph
                    .processors
                    .iter()
                    .filter_map(|p| p.rprofile)
                    .flat_map(|p| [p.tcm.atcm.bytes, p.tcm.btcm.bytes]),
            )
            .try_fold(0usize, |sum, n| {
                sum.checked_add(n)
                    .ok_or_else(|| error("declared RAM extent overflows".into()))
            })?;
        Ok(Self {
            driver: None,
            memory_watch: None,
            function_watch: None,
            instruction_points: BTreeMap::new(),
            backend: Some(Backend::Graph(session)),
            objects: BTreeMap::new(),
            failures: Failures::default(),
            read_limit,
            last_continuation_stop: None,
        })
    }
    #[getter]
    fn boot_quantum(&self) -> PyResult<usize> {
        self.session()?.definition["quantum"]
            .as_u64()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| error("board was not constructed from boot inputs".into()))
    }
    #[pyo3(signature=(instruction_limit=2_000_000_000))]
    fn boot_to_rec(&mut self, py: Python<'_>, instruction_limit: u64) -> PyResult<String> {
        if self.driver.is_some()
            || self.memory_watch.is_some()
            || self.function_watch.is_some()
            || !self.instruction_points.is_empty()
        {
            return Err(error(
                "boot requires no active host instrumentation or isolated call".into(),
            ));
        }
        let owner = Owner::new(self.session_mut()?);
        py.detach(move || owner.run(instruction_limit))
    }
    fn screenshot<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(
            py,
            &boot()?.screenshot(self.session()?).map_err(error)?,
        ))
    }
}
