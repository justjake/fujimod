//! File-based construction and synchronous execution on the Python owner thread.
use super::*;
use fujimod_python_base::boot::{Inputs, Opened, boot};
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
            return Err(error(
                "session execution moved from its owner thread".into(),
            ));
        }
        boot()?
            .to_rec(unsafe { self.session.as_mut() }, limit)
            .map_err(error)
    }
}

/// The most bytes a host may read from a graph session's RAM.
fn graph_read_limit(session: &fujimod_emulator_session::Session) -> PyResult<usize> {
    let graph: factory::Graph = serde_json::from_value(session.definition["execution"].clone())
        .map_err(|e| error(e.to_string()))?;
    graph
        .ram
        .iter()
        .map(|r| r.size)
        .chain(
            graph
                .processors
                .iter()
                .filter_map(|p| p.rprofile.clone())
                .flat_map(|p| [p.tcm.atcm.bytes, p.tcm.btcm.bytes]),
        )
        .try_fold(0usize, |sum, n| {
            sum.checked_add(n)
                .ok_or_else(|| error("declared RAM extent overflows".into()))
        })
}

/// The measured power-on RAM byte: a body's own measurement, else its
/// processor's DRAM region.
fn declared_power_on_ram_fill(definition: &serde_json::Value) -> Option<u8> {
    let reference = &definition["resolved"]["reference"];
    let body = reference["body"]["power_on_ram_fill"]["byte"].as_u64();
    let soc = reference["soc"]["memory_regions"]
        .as_array()
        .and_then(|regions| {
            regions
                .iter()
                .filter(|region| region["kind"] == "dram")
                .find_map(|region| region["power_on_ram_fill"]["byte"].as_u64())
        });
    body.or(soc).and_then(|value| u8::try_from(value).ok())
}

#[pymethods]
impl Board {
    #[staticmethod]
    #[pyo3(signature=(*, configuration_id, update, boot_rom=None, card_image=None, card_files=Vec::new(), second_card_image=None, second_card_files=Vec::new(), scene=None, overlays=Vec::new(), nand=None, settings=None, clock_seconds_2000=None, preferred_languages=Vec::new(), camera=None, access_point=None, capability_reply=None, camera_settings=std::collections::BTreeMap::new(), engine=None, manifest=None, application_plan=None, discovery=false, quantum=None, instruction_rate=None))]
    fn from_inputs(
        configuration_id: String,
        update: PathBuf,
        boot_rom: Option<PathBuf>,
        card_image: Option<PathBuf>,
        card_files: Vec<(String, PathBuf)>,
        second_card_image: Option<PathBuf>,
        second_card_files: Vec<(String, PathBuf)>,
        scene: Option<PathBuf>,
        overlays: Vec<String>,
        nand: Option<PathBuf>,
        settings: Option<PathBuf>,
        clock_seconds_2000: Option<u32>,
        preferred_languages: Vec<String>,
        camera: Option<PathBuf>,
        access_point: Option<String>,
        capability_reply: Option<Vec<u8>>,
        camera_settings: std::collections::BTreeMap<String, String>,
        engine: Option<String>,
        manifest: Option<PathBuf>,
        application_plan: Option<PathBuf>,
        discovery: bool,
        quantum: Option<usize>,
        instruction_rate: Option<(u64, u64)>,
    ) -> PyResult<Self> {
        let opened = boot()?
            .open(Inputs {
                configuration_id,
                overlays,
                update,
                boot_rom,
                card_image,
                card_files,
                second_card_image,
                second_card_files,
                scene,
                nand,
                settings,
                clock_seconds_2000,
                preferred_languages,
                camera,
                access_point,
                capability_reply,
                camera_settings,
                engine,
                manifest,
                application_plan,
                discovery,
                quantum,
                instruction_rate,
            })
            .map_err(error)?;
        let (backend, read_limit) = match opened {
            Opened::Graph(session) => {
                let read_limit = graph_read_limit(&session)?;
                return Ok(Self::from_graph_session(session, read_limit));
            }
            Opened::Xp5 {
                session,
                inputs,
                read_limit,
            } => (
                Backend::Xp5(Box::new(super::xp5::Opened { session, inputs })),
                read_limit,
            ),
        };
        Ok(Self {
            driver: None,
            memory_watch: None,
            function_watch: None,
            instruction_points: BTreeMap::new(),
            backend: Some(backend),
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
    /// The byte this board's DRAM holds at power-on, or None when no camera
    /// measured one (RAM then starts zeroed).
    #[getter]
    fn power_on_ram_fill(&self) -> PyResult<Option<u8>> {
        Ok(declared_power_on_ram_fill(&self.session()?.definition))
    }
    /// The catalog run configuration this board was prepared from.
    #[getter]
    fn configuration_id(&self) -> PyResult<Option<String>> {
        Ok(self
            .session()?
            .definition
            .get("configuration_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned))
    }
    /// The catalog overlays this board was prepared with.
    #[getter]
    fn overlays(&self) -> PyResult<Option<Vec<String>>> {
        let Some(overlays) = self.session()?.definition.get("overlays") else {
            return Ok(None);
        };
        let Some(overlays) = overlays.as_array() else {
            return Err(error("board overlays are not a list".into()));
        };
        overlays
            .iter()
            .map(|overlay| {
                overlay
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| error("board overlay is not a string".into()))
            })
            .collect::<PyResult<Vec<_>>>()
            .map(Some)
    }
    #[pyo3(signature=(instruction_limit=2_000_000_000))]
    fn boot_to_rec(&mut self, py: Python<'_>, instruction_limit: u64) -> PyResult<String> {
        if self.driver.is_some() || !self.instruction_points.is_empty() {
            return Err(error(
                "boot requires no instruction points or isolated call".into(),
            ));
        }
        let owner = Owner::new(self.session_mut()?);
        let result = py.detach(move || owner.run(instruction_limit));
        // Cold reconstruction replaces the CPUs and their hooks. Memory
        // watch records remain available with an incomplete-observation error.
        if self
            .function_watch
            .as_ref()
            .is_some_and(|watch| !watch.is_live())
        {
            self.function_watch.take();
            return Err(error(
                "boot restarted while a function watch was active".into(),
            ));
        }
        result
    }
    fn screenshot<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(
            py,
            &boot()?.screenshot(self.session()?).map_err(error)?,
        ))
    }
}
