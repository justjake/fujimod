//! Python client and transitional devices on the shared runtime.
mod boot;
mod checkpoint;
mod context;
mod loops;
mod observation;
mod xp4;
mod xp5;
mod xpp;
pub use checkpoint::Checkpoint;
use fujimod_python_base::xp4_backend;
mod device;
use fujimod_emulator_preparation as preparation;
mod run;
mod storage;
use device::{Adapters, Failures, Object};
use fujimod_emulator::board::{
    Board as ExecutionBoard,
    bus::{Bus, Mode},
    factory,
};
use fujimod_emulator_cpu::{Register, arm};
use fujimod_emulator_exr2_plan::Plan;
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
    pyclass::{PyTraverseError, PyVisit},
    types::{PyBytes, PyDict},
};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

fn error(message: String) -> PyErr {
    fujimod_python_base::error(message)
}
fn json<T: serde::de::DeserializeOwned>(value: &str) -> PyResult<T> {
    serde_json::from_str(value).map_err(|e| PyValueError::new_err(e.to_string()))
}

enum Backend {
    Graph(fujimod_emulator_session::Session),
    Xp5(Box<xp5::Opened>),
    Xp4(Box<dyn xp4_backend::Xp4Backend>),
}

/// Lend the session of a Python `Board` to another binding crate.
pub(crate) fn borrow_session(
    board: &Bound<'_, PyAny>,
    call: &mut dyn FnMut(&mut fujimod_emulator_session::Session) -> PyResult<()>,
) -> PyResult<()> {
    let mut board = board.cast::<Board>()?.try_borrow_mut()?;
    call(board.session_mut()?)
}

#[pyclass(unsendable, module = "fujimod._native.emulator")]
pub struct Board {
    driver: Option<Box<fujimod_emulator::board::harness::Session>>,
    memory_watch: Option<Box<fujimod_emulator::board::observation::MemoryWatch>>,
    function_watch: Option<Box<fujimod_emulator::board::observation::FunctionWatch>>,
    instruction_points:
        BTreeMap<usize, Box<fujimod_emulator::board::observation::InstructionPoints>>,
    backend: Option<Backend>,
    objects: BTreeMap<String, Arc<Object>>,
    failures: Failures,
    read_limit: usize,
}
impl Board {
    fn begin_continuation(
        &mut self,
        stores: Option<Vec<(u64, u64)>>,
        devices: Option<Vec<String>>,
    ) -> PyResult<()> {
        if stores.is_none() && devices.is_none() {
            return Ok(());
        }
        let session = self.session()?;
        let devices = devices
            .unwrap_or_default()
            .iter()
            .map(|name| session.node(name))
            .collect::<Result<_, _>>()
            .map_err(error)?;
        let board = self.board_mut()?;
        let maps = board.mappings().to_vec();
        board
            .continuation_begin(stores.unwrap_or_default(), devices, maps)
            .map_err(error)
    }
    fn run_graph(
        &mut self,
        py: Python<'_>,
        count: u64,
        quantum: usize,
        pause_after: Option<u64>,
        pause_at_tick: Option<u64>,
    ) -> PyResult<()> {
        let policy = if let Some(driver) = self.driver.as_ref() {
            let pc = self
                .board()?
                .read_register(driver.core(), Register::Pc)
                .map_err(error)?;
            driver
                .resume_policy(pc)
                .map(|step| (driver.core(), driver.interrupt_input(), step))
        } else {
            None
        };
        if let Some((core, _, _)) = policy {
            self.board()?.harness_interrupt_delivery(core, false);
        }
        let result = run::budget(
            py,
            self.board_mut()?,
            if policy.is_some_and(|(_, _, step)| step) {
                count.min(1)
            } else {
                count
            },
            quantum,
            pause_after,
            pause_at_tick,
        );
        if let Some((core, allowed, _)) = policy {
            self.board()?.harness_interrupt_delivery(core, allowed);
        }
        self.outcome(result)
    }
    fn session(&self) -> PyResult<&fujimod_emulator_session::Session> {
        match self.backend.as_ref() {
            Some(Backend::Graph(session)) => Ok(session),
            Some(Backend::Xp5(opened)) => Ok(&opened.session),
            Some(Backend::Xp4(_)) => Err(PyRuntimeError::new_err(
                "Selected XP4 system does not support this graph operation",
            )),
            None => Err(PyRuntimeError::new_err("runtime is closed")),
        }
    }
    pub fn session_mut(&mut self) -> PyResult<&mut fujimod_emulator_session::Session> {
        match self.backend.as_mut() {
            Some(Backend::Graph(session)) => Ok(session),
            Some(Backend::Xp5(opened)) => Ok(&mut opened.session),
            Some(Backend::Xp4(_)) => Err(PyRuntimeError::new_err(
                "Selected XP4 system does not support this graph operation",
            )),
            None => Err(PyRuntimeError::new_err("runtime is closed")),
        }
    }
    fn board(&self) -> PyResult<&ExecutionBoard> {
        Ok(&self.session()?.board)
    }
    fn board_mut(&mut self) -> PyResult<&mut ExecutionBoard> {
        Ok(&mut self.session_mut()?.board)
    }
    fn xp4_session(&self) -> PyResult<&dyn xp4_backend::Xp4Backend> {
        match self.backend.as_ref() {
            Some(Backend::Xp4(session)) => Ok(session.as_ref()),
            Some(Backend::Graph(_) | Backend::Xp5(_)) => Err(PyRuntimeError::new_err(
                "Selected graph system does not support this XP4 operation",
            )),
            None => Err(PyRuntimeError::new_err("runtime is closed")),
        }
    }
    fn outcome(&self, result: Result<(), String>) -> PyResult<()> {
        if let Some(error) = self.failures.take() {
            return Err(error);
        }
        result.map_err(error)
    }
    fn check_read(&self, size: usize) -> PyResult<()> {
        if size > self.read_limit {
            return Err(PyValueError::new_err(
                "host read exceeds declared physical/private RAM",
            ));
        }
        Ok(())
    }
}
impl Drop for Board {
    fn drop(&mut self) {
        self.clear_observation();
        if let Some(mut driver) = self.driver.take() {
            if let Ok(session) = self.session() {
                let _ = session.board.harness_end(&mut driver);
            }
        }
    }
}
#[pymethods]
impl Board {
    fn set_execution_policy(&mut self, policy: &str) -> PyResult<()> {
        let policy = fujimod_emulator_cpu::ExecutionPolicy::parse(policy).map_err(error)?;
        if self.driver.is_some() && policy != fujimod_emulator_cpu::ExecutionPolicy::Exact {
            return Err(PyValueError::new_err(
                "Isolated routines require exact execution",
            ));
        }
        self.board_mut()?
            .set_execution_policy(policy)
            .map_err(error)
    }

    fn execution_policy(&self) -> PyResult<&'static str> {
        Ok(self.board()?.execution_policy().name())
    }
    #[staticmethod]
    #[pyo3(signature=(definition, storage_image=None))]
    fn from_xp4(definition: &str, storage_image: Option<Vec<u8>>) -> PyResult<Self> {
        let session = xp4_backend::open(definition, storage_image)?;
        Ok(Self {
            driver: None,
            memory_watch: None,
            function_watch: None,
            instruction_points: BTreeMap::new(),
            read_limit: session.ram_bytes(),
            backend: Some(Backend::Xp4(session)),
            objects: BTreeMap::new(),
            failures: Failures::default(),
        })
    }
    #[getter]
    fn xp4(slf: PyRef<'_, Self>) -> PyResult<xp4::Xp4> {
        slf.xp4_session()?;
        Ok(xp4::Xp4 { owner: slf.into() })
    }
    #[pyo3(signature=(entries, quota=4, edge_limit=1<<20, state_limit=1<<16,
                      return_sites=None, quota_per_edge=false))]
    #[allow(clippy::too_many_arguments)]
    fn function_watch_begin(
        &mut self,
        entries: Vec<u64>,
        quota: u64,
        edge_limit: usize,
        state_limit: usize,
        return_sites: Option<Vec<u64>>,
        quota_per_edge: bool,
    ) -> PyResult<()> {
        if self.driver.is_some() {
            return Err(error("Function watch requires a whole-board run".into()));
        }
        let recorder = fujimod_emulator::board::observation::calls::Recorder::new(
            entries,
            return_sites.unwrap_or_default(),
            quota,
            quota_per_edge,
            edge_limit,
            state_limit,
        )
        .map_err(error)?;
        let maps =
            serde_json::from_value(self.session()?.definition["execution"]["mappings"].clone())
                .map_err(|e| error(e.to_string()))?;
        self.function_watch_end()?;
        self.function_watch = Some(
            self.board()?
                .function_watch_begin(recorder, maps)
                .map_err(error)?,
        );
        Ok(())
    }
    fn function_watch_state<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>> {
        self.function_watch
            .as_ref()
            .map(|watch| {
                watch.check().map_err(error)?;
                fujimod_python_base::observation::snapshot(
                    py,
                    &watch.recorder.borrow().snapshot(),
                )
            })
            .transpose()
    }
    fn function_watch_end(&mut self) -> PyResult<()> {
        if let Some(mut watch) = self.function_watch.take() {
            self.board()?
                .function_watch_end(&mut watch)
                .map_err(error)?;
        }
        Ok(())
    }
    fn transaction_watch_begin(&self, ranges: Vec<(u64, u64)>, limit: usize) -> PyResult<()> {
        if self.driver.is_some() {
            return Err(PyValueError::new_err("An isolated call is active"));
        }
        self.board()?
            .transaction_watch_begin(ranges, limit)
            .map_err(error)
    }

    fn transaction_watch_end(&self) -> PyResult<String> {
        let batch = self.board()?.transaction_watch_end().map_err(error)?;
        if batch.omitted != 0 {
            return Err(PyRuntimeError::new_err(format!(
                "Bus transaction watch omitted {} accesses",
                batch.omitted
            )));
        }
        serde_json::to_string(&batch).map_err(|e| error(e.to_string()))
    }

    #[pyo3(signature=(ranges,limit=1_000_000))]
    fn memory_watch_begin(&mut self, ranges: Vec<(u64, u64)>, limit: usize) -> PyResult<()> {
        if self.memory_watch.is_some() {
            return Err(error("Another memory watch is active".into()));
        }
        let maps =
            serde_json::from_value(self.session()?.definition["execution"]["mappings"].clone())
                .map_err(|e| error(e.to_string()))?;
        self.memory_watch = Some(
            self.board()?
                .memory_watch_begin(ranges, limit, maps)
                .map_err(error)?,
        );
        Ok(())
    }
    fn memory_watch_enabled(&self) -> bool {
        self.memory_watch.is_some()
    }
    #[pyo3(signature = () -> "dict[str, object]")]
    fn memory_watch_state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let value = self
            .memory_watch
            .as_ref()
            .map(|w| w.records.borrow().state())
            .unwrap_or_else(
                || serde_json::json!({"enabled":false,"ranges":[],"records":[],"violation":null}),
            );
        py.import("json")?
            .call_method1("loads", (value.to_string(),))
    }
    fn memory_watch_end(&mut self) -> PyResult<()> {
        if let Some(mut watch) = self.memory_watch.take() {
            self.board()?.memory_watch_end(&mut watch).map_err(error)?;
        }
        Ok(())
    }
    fn instruction_points(&mut self, core: usize, addresses: Vec<u64>) -> PyResult<()> {
        if self.driver.is_some() {
            return Err(error(
                "Use DriverHarness.observe during an isolated call".into(),
            ));
        }
        if let Some(mut points) = self.instruction_points.remove(&core) {
            self.board()?
                .instruction_points_end(core, &mut points)
                .map_err(error)?;
        }
        if !addresses.is_empty() {
            let points = self
                .board()?
                .instruction_points_begin(core, addresses)
                .map_err(error)?;
            self.instruction_points.insert(core, points);
        }
        Ok(())
    }
    fn instruction_event(&self) -> Option<(usize, u64)> {
        self.instruction_points
            .iter()
            .find_map(|(&core, p)| p.event().map(|pc| (core, pc)))
    }
    fn instruction_resume(&self, core: usize, observed_pc: u64) -> PyResult<()> {
        if let Some(points) = self.instruction_points.get(&core) {
            let pc = self
                .board()?
                .read_register(core, Register::Pc)
                .map_err(error)?;
            points.resume(pc, observed_pc);
        }
        Ok(())
    }

    #[pyo3(signature=(previous=None))]
    fn checkpoint(&self, previous: Option<PyRef<'_, Checkpoint>>) -> PyResult<Checkpoint> {
        if self.driver.is_some()
            || self.memory_watch.is_some()
            || self.function_watch.is_some()
            || !self.instruction_points.is_empty()
            || self.board()?.transaction_watch_active().map_err(error)?
        {
            return Err(PyRuntimeError::new_err(
                "keyframe requires no active host instrumentation or isolated call",
            ));
        }
        let saved = self
            .session()?
            .checkpoint(previous.as_ref().map(|p| &p.saved))
            .map_err(error)?;
        Ok(Checkpoint { saved })
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> PyResult<()> {
        if self.driver.is_some()
            || self.memory_watch.is_some()
            || self.function_watch.is_some()
            || !self.instruction_points.is_empty()
            || self.board()?.transaction_watch_active().map_err(error)?
        {
            return Err(PyRuntimeError::new_err(
                "keyframe requires no active host instrumentation or isolated call",
            ));
        }
        self.session_mut()?.restore(&saved.saved).map_err(error)
    }

    #[new]
    #[pyo3(signature = (execution, devices, scene, assets, models, *, execution_mode="strict", continue_refusals=false, engine="unicorn"))]
    fn new(
        execution: &str,
        devices: &str,
        scene: &str,
        assets: &Bound<'_, PyDict>,
        models: &Bound<'_, PyDict>,
        execution_mode: &str,
        continue_refusals: bool,
        engine: &str,
    ) -> PyResult<Self> {
        let engine = match engine {
            "unicorn" => factory::Engine::Unicorn,
            "hypervisor" => factory::Engine::Hypervisor,
            _ => {
                return Err(PyValueError::new_err(
                    "engine must be unicorn or hypervisor",
                ));
            }
        };
        let mode = match execution_mode {
            "strict" => Mode::Strict,
            "discovery" => Mode::Discovery,
            _ => {
                return Err(PyValueError::new_err(
                    "execution_mode must be strict or discovery",
                ));
            }
        };
        if continue_refusals && mode != Mode::Discovery {
            return Err(PyValueError::new_err(
                "continuing modeled refusals requires discovery",
            ));
        }
        let graph: factory::Graph = json(execution)?;
        graph.validate().map_err(error)?;
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
            .try_fold(0usize, |sum, bytes| {
                sum.checked_add(bytes)
                    .ok_or_else(|| PyValueError::new_err("declared RAM extent overflows host size"))
            })?;
        if json::<serde_json::Value>(devices)?
            .get("kind")
            .and_then(serde_json::Value::as_str)
            == Some("xp5")
        {
            if !models.is_empty() || json::<serde_json::Value>(scene)? != serde_json::json!({}) {
                return Err(PyValueError::new_err(
                    "XP5 requires its native devices and explicit assets",
                ));
            }
            return Self::xp5_graph(
                graph, devices, assets, read_limit, mode, continue_refusals, engine,
                serde_json::json!({"execution":json::<serde_json::Value>(execution)?,"devices":json::<serde_json::Value>(devices)?}),
            );
        }
        if json::<serde_json::Value>(devices)?.get("kind").and_then(serde_json::Value::as_str) == Some("xpp") {
            if !matches!(engine, factory::Engine::Unicorn) || !models.is_empty()
                || json::<serde_json::Value>(scene)? != serde_json::json!({}) {
                return Err(PyValueError::new_err("X-Processor Pro requires its native devices and explicit assets"));
            }
            return Self::xpp_graph(graph, devices, assets, read_limit, mode, continue_refusals,
                serde_json::json!({"execution":json::<serde_json::Value>(execution)?,"devices":json::<serde_json::Value>(devices)?}));
        }
        let plan = Plan::parse(devices.as_bytes()).map_err(error)?;
        plan.validate_reset_outputs(
            &graph
                .processors
                .iter()
                .map(|p| p.released)
                .collect::<Vec<_>>(),
        )
        .map_err(error)?;
        plan.validate_memory_outputs(
            &graph
                .decoders
                .iter()
                .map(|d| fujimod_emulator_exr2_plan::MemoryChoices {
                    initial: d.initial,
                    choices: d.choices.len(),
                })
                .collect::<Vec<_>>(),
        )
        .map_err(error)?;
        let mut bytes = BTreeMap::new();
        for (key, value) in assets.iter() {
            let value = value.cast::<PyBytes>()?;
            if value.as_bytes().len() > 128 << 20 {
                return Err(PyValueError::new_err("scene asset exceeds 128 MiB"));
            }
            bytes.insert(key.extract::<String>()?, value.as_bytes().to_vec());
        }
        let mut pending = BTreeMap::new();
        for (key, value) in models.iter() {
            pending.insert(key.extract::<String>()?, value.unbind());
        }
        let mut adapters = Adapters {
            pending,
            objects: BTreeMap::new(),
            failures: Failures::default(),
            start_tick: 0,
        };
        let resources = json::<preparation::Spec>(scene)?
            .build(&bytes)
            .map_err(error)?;
        let built = plan
            .build(resources, 0, &mut adapters)
            .map_err(|reason| adapters.failures.take().unwrap_or_else(|| error(reason)))?;
        if !adapters.pending.is_empty() {
            return Err(PyValueError::new_err(
                "Python devices supplied for nodes that do not select an adapter",
            ));
        }
        let mut bus = Bus::from_graph(mode, built.graph).map_err(error)?;
        bus.continue_refusals(continue_refusals).map_err(error)?;
        let mut board = graph
            .build(
                factory::Options::new(engine),
                bus,
                None,
            )
            .map_err(error)?;
        for controller in built.core_interrupts {
            board
                .configure_core_interrupts(
                    controller.core,
                    controller.device,
                    Box::new(controller.inputs),
                )
                .map_err(error)?;
        }
        Ok(Self {
            driver: None,
            memory_watch: None,
            function_watch: None,
            instruction_points: BTreeMap::new(),
            backend: Some(Backend::Graph(fujimod_emulator_session::Session {
                definition: serde_json::json!({"execution":json::<serde_json::Value>(execution)?,"devices":json::<serde_json::Value>(devices)?,"scene":json::<serde_json::Value>(scene)?}),
                board,
                controls: None,
                contact_driver: None,
                nodes: built.node_ids,
                contacts: built.contacts,
                recording_frames: Default::default(),
                resources: built.resources,
            })),
            objects: adapters.objects,
            failures: adapters.failures,
            read_limit,
        })
    }
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        if let Some(Backend::Xp4(session)) = &self.backend {
            return session.__traverse__(visit);
        }
        // The board's adapter and this table share one Arc, hence exactly one
        // owning Python reference to each model/context is visited here.
        for object in self.objects.values() {
            visit.call(&object.model)?;
            visit.call(&object.context)?;
        }
        Ok(())
    }
    fn __clear__(&mut self) {
        self.clear_observation();
        if let Some(mut driver) = self.driver.take() {
            if let Ok(session) = self.session() {
                let _ = session.board.harness_end(&mut driver);
            }
        }
        if let Some(Backend::Xp4(session)) = &self.backend {
            session.__clear__();
        }
        self.backend.take();
        self.objects.clear();
    }
    fn close(&mut self) {
        self.__clear__();
    }
    fn usb_reset(&mut self, name: &str) -> PyResult<()> {
        self.session_mut()?.usb_reset(name).map_err(error)
    }
    fn usb_setup(&mut self, name: &str, packet: &[u8]) -> PyResult<()> {
        self.session_mut()?.usb_setup(name, packet).map_err(error)
    }
    fn usb_out(&mut self, name: &str, endpoint: u8, packet: &[u8]) -> PyResult<()> {
        self.session_mut()?
            .usb_out(name, endpoint, packet)
            .map_err(error)
    }
    fn usb_in<'py>(
        &mut self,
        py: Python<'py>,
        name: &str,
        endpoint: u8,
        size: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let packet = self
            .session_mut()?
            .usb_in(name, endpoint, size)
            .map_err(error)?;
        Ok(PyBytes::new(py, &packet))
    }
    fn usb_in_token(&mut self, name: &str, endpoint: u8) -> PyResult<()> {
        self.session_mut()?
            .usb_in_token(name, endpoint)
            .map_err(error)
    }
    fn usb_status(&self, name: &str) -> PyResult<String> {
        serde_json::to_string(&self.session()?.usb_status(name).map_err(error)?)
            .map_err(|e| error(e.to_string()))
    }

    fn driver_begin(&mut self, options: &str, transaction_limit: usize) -> PyResult<()> {
        if self.driver.is_some()
            || self.function_watch.is_some()
            || !self.instruction_points.is_empty()
            || self.board()?.transaction_watch_active().map_err(error)?
        {
            return Err(PyValueError::new_err("isolated call is already active"));
        }
        self.driver = Some(self.board()?.harness_begin(json(options)?).map_err(error)?);
        self.board()?.observe_transactions(Some(transaction_limit));
        Ok(())
    }
    fn driver_resume(&self, skip_once: Option<u64>) -> PyResult<()> {
        self.driver
            .as_ref()
            .ok_or_else(|| PyValueError::new_err("no active isolated call"))?
            .resume(skip_once);
        Ok(())
    }
    fn driver_state(&self) -> PyResult<String> {
        let mut state = self
            .driver
            .as_ref()
            .ok_or_else(|| PyValueError::new_err("no active isolated call"))?
            .state();
        state["bus"] = self.board()?.transactions();
        serde_json::to_string(&state).map_err(|e| error(e.to_string()))
    }
    fn driver_end(&mut self) -> PyResult<()> {
        if let Some(mut driver) = self.driver.take() {
            self.board()?.harness_end(&mut driver).map_err(error)?;
            self.board()?.observe_transactions(None);
        }
        Ok(())
    }
    fn driver_select_core(&self, core: usize) -> PyResult<()> {
        self.board()?.harness_select_core(core).map_err(error)
    }
    fn driver_advance(&mut self, tick: u64, quantum: usize) -> PyResult<()> {
        self.board_mut()?
            .harness_advance(tick, quantum)
            .map_err(error)
    }
    fn raise_irq(&mut self, core: usize, line: u32) -> PyResult<()> {
        self.board_mut()?.raise_irq(core, line).map_err(error)
    }
    fn rprofile_identity(&self, core: usize, value: Option<u32>) -> PyResult<u32> {
        self.board()?
            .cpu(core)
            .map_err(error)?
            .rprofile_identity(value)
            .map_err(error)
    }
    fn analog_inputs(&self, node: &str) -> PyResult<AnalogInputs> {
        let session = self.session()?;
        let inputs = session
            .board
            .inspect_capability::<dyn fujimod_emulator_devices::capability::AnalogInputs, _>(
                session.node(node).map_err(error)?,
                |adc| Ok(adc.external_inputs()),
            )
            .map_err(error)?;
        Ok(AnalogInputs { inputs })
    }
    fn adc_samples(&self, node: &str) -> PyResult<Vec<u16>> {
        self.session()?.adc_samples(node).map_err(error)
    }
    fn set_adc_samples(&mut self, node: &str, samples: Vec<u16>) -> PyResult<()> {
        self.session_mut()?
            .set_adc_samples(node, &samples)
            .map_err(error)
    }
    fn configure_controls(&mut self, spec: &str) -> PyResult<()> {
        self.session_mut()?
            .configure_controls(json(spec)?)
            .map_err(error)
    }
    fn controls_connected(&self) -> PyResult<bool> {
        Ok(self.session()?.controls.is_some())
    }
    fn input_graph(&self) -> PyResult<String> {
        let definition = &self.session()?.definition;
        let nodes = definition["devices"]["nodes"]
            .as_array()
            .ok_or_else(|| error("runtime has no device definition".into()))?
            .iter()
            .filter(|n| {
                matches!(
                    n["model"]["kind"].as_str(),
                    Some("gpio" | "adc" | "encoder" | "i2c" | "spi-controller")
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        serde_json::to_string(
            &serde_json::json!({"execution":{"ram":definition["execution"]["ram"],
            "mappings":definition["execution"]["mappings"]},"devices":{"nodes":nodes}}),
        )
        .map_err(|e| error(e.to_string()))
    }
    fn set_position(&mut self, id: &str, position: &str) -> PyResult<()> {
        self.session_mut()?
            .set_position(id, position)
            .map_err(error)
    }
    #[pyo3(signature = (id, stage, release))]
    fn contact_input(&mut self, id: &str, stage: Option<&str>, release: bool) -> PyResult<()> {
        self.session_mut()?
            .contact_input(id, stage, release)
            .map_err(error)
    }
    fn control_state(&self) -> PyResult<String> {
        serde_json::to_string(&self.session()?.control_state().map_err(error)?)
            .map_err(|e| error(e.to_string()))
    }
    fn configure_contact_driver(&mut self, spec: &str) -> PyResult<()> {
        self.session_mut()?
            .configure_contact_driver(json(spec)?)
            .map_err(error)
    }
    fn rotate_control(&mut self, id: &str, steps: i32) -> PyResult<()> {
        let (node, cycles) = self.session()?.rotation(id, steps).map_err(error)?;
        self.session_mut()?.rotate(&node, cycles).map_err(error)
    }
    fn touch_script(&mut self, node: &str, script: &str) -> PyResult<()> {
        self.session_mut()?
            .touch(
                node,
                &json::<Vec<fujimod_emulator_devices::parts::touch_input::ContactInput>>(
                    script,
                )?,
            )
            .map_err(error)
    }
    fn touch_control(&mut self, id: &str, phase: &str, x: u16, y: u16) -> PyResult<()> {
        let (node, script) = self
            .session()?
            .touch_input(id, phase, x, y)
            .map_err(error)?;
        self.session_mut()?.touch(&node, &script).map_err(error)
    }
    fn configure_frames(&mut self, node: &str, period_ticks: u64, cadence: &str) -> PyResult<()> {
        self.session_mut()?
            .configure_frames(node, period_ticks, json(cadence)?)
            .map_err(error)
    }
    fn scanout_state(&self, node: &str) -> PyResult<String> {
        serde_json::to_string(&self.session()?.scanout_state(node).map_err(error)?)
            .map_err(|e| error(e.to_string()))
    }
    fn scanout<'py>(
        &self,
        py: Python<'py>,
        node: &str,
        instance: usize,
        pixels: bool,
        diagnostic_hashes: bool,
    ) -> PyResult<
        Option<(
            Bound<'py, pyo3::types::PyBytes>,
            Option<Bound<'py, pyo3::types::PyBytes>>,
            String,
            usize,
            usize,
        )>,
    > {
        self.session()?
            .scanout(node, instance, pixels, diagnostic_hashes)
            .map_err(error)?
            .map(|f| {
                Ok((
                    pyo3::types::PyBytes::new(py, &f.raw),
                    f.rgb.as_deref().map(|v| pyo3::types::PyBytes::new(py, v)),
                    serde_json::to_string(&f.state.json()).map_err(|e| error(e.to_string()))?,
                    f.width,
                    f.height,
                ))
            })
            .transpose()
    }
    fn recording_frame<'py>(
        &self,
        py: Python<'py>,
        node: &str,
        instance: usize,
        diagnostic_hashes: bool,
    ) -> PyResult<Option<(Bound<'py, pyo3::types::PyBytes>, String, usize, usize)>> {
        self.session()?
            .recording_frame(node, instance, diagnostic_hashes)
            .map_err(error)?
            .map(|frame| {
                Ok((
                    pyo3::types::PyBytes::new(
                        py,
                        frame
                            .rgb
                            .as_deref()
                            .ok_or_else(|| error("Capture omitted RGB".into()))?,
                    ),
                    serde_json::to_string(&frame.state.json()).map_err(|e| error(e.to_string()))?,
                    frame.width,
                    frame.height,
                ))
            })
            .transpose()
    }
    fn scanout_main<'py>(
        &self,
        py: Python<'py>,
        node: &str,
        instance: usize,
        pixels: bool,
        diagnostic_hashes: bool,
    ) -> PyResult<
        Option<(
            Bound<'py, pyo3::types::PyBytes>,
            Option<Bound<'py, pyo3::types::PyBytes>>,
            String,
            usize,
            usize,
        )>,
    > {
        self.session()?
            .scanout_main(node, instance, pixels, diagnostic_hashes)
            .map_err(error)?
            .map(|f| {
                Ok((
                    pyo3::types::PyBytes::new(py, &f.raw),
                    f.rgb.as_deref().map(|v| pyo3::types::PyBytes::new(py, v)),
                    serde_json::to_string(&f.state.json()).map_err(|e| error(e.to_string()))?,
                    f.width,
                    f.height,
                ))
            })
            .transpose()
    }
    fn frame_state(&self, node: &str) -> PyResult<String> {
        serde_json::to_string(&self.session()?.frame_state(node).map_err(error)?)
            .map_err(|e| error(e.to_string()))
    }
    fn serial_state(&self, node: &str) -> PyResult<String> {
        serde_json::to_string(&self.session()?.serial_state(node).map_err(error)?)
            .map_err(|e| error(e.to_string()))
    }
    fn serial_status_input(
        &mut self,
        node: &str,
        route: usize,
        register: u8,
        value: u8,
    ) -> PyResult<()> {
        self.session_mut()?
            .serial_status_input(node, route, register, value)
            .map_err(error)
    }
    fn watch_pcs(&mut self, addresses: Vec<u64>) -> PyResult<()> {
        self.board_mut()?.watch_pcs(&addresses).map_err(error)
    }
    fn pc_entries(&self) -> PyResult<String> {
        serde_json::to_string(&self.board()?.pc_entries()).map_err(|e| error(e.to_string()))
    }
    #[getter]
    fn now(&self) -> PyResult<u64> {
        if let Some(Backend::Xp4(session)) = &self.backend {
            return Ok(session.now());
        }
        Ok(self.board()?.now())
    }
    #[getter]
    fn quiescent(&self) -> PyResult<bool> {
        if let Some(Backend::Xp4(session)) = &self.backend {
            return Ok(session.quiescent());
        }
        Ok(self.board()?.quiescent())
    }
    #[getter]
    fn instructions(&self) -> PyResult<Vec<u64>> {
        if let Some(Backend::Xp4(session)) = &self.backend {
            return Ok(session.instructions());
        }
        Ok(self.board()?.instruction_counts())
    }
    fn run_until(&mut self, py: Python<'_>, tick: u64, quantum: usize) -> PyResult<()> {
        let result = run::until(py, self.board_mut()?, tick, quantum);
        self.outcome(result)
    }
    fn run_for_instructions(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        count: u64,
        quantum: usize,
    ) -> PyResult<()> {
        {
            let owner = slf.try_borrow()?;
            if let Some(Backend::Xp4(session)) = &owner.backend {
                return session.run_board(count, quantum);
            }
        }
        let mut owner = slf.try_borrow_mut()?;
        let result = run::instructions(py, owner.board_mut()?, count, quantum);
        owner.outcome(result)
    }
    fn read_set_begin(&self) -> PyResult<()> {
        self.board()?.read_set_begin().map_err(error)
    }
    fn read_set_end(&self) -> PyResult<String> {
        let reads = self.board()?.read_set_end().map_err(error)?;
        let session = self.session()?;
        let names = reads
            .devices
            .iter()
            .map(|id| {
                session
                    .nodes
                    .iter()
                    .find(|(_, node)| *node == id)
                    .map(|(name, _)| name.clone())
                    .ok_or_else(|| error("Read set names an absent device".into()))
            })
            .collect::<PyResult<Vec<_>>>()?;
        serde_json::to_string(&serde_json::json!({"stores":reads.stores,"devices":names}))
            .map_err(|e| error(e.to_string()))
    }
    #[pyo3(signature = (count, quantum, pause_after=None, pause_at_tick=None, store_watches=None, device_watches=None))]
    fn run(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        count: u64,
        quantum: usize,
        pause_after: Option<u64>,
        pause_at_tick: Option<u64>,
        store_watches: Option<Vec<(u64, u64)>>,
        device_watches: Option<Vec<String>>,
    ) -> PyResult<bool> {
        {
            let owner = slf.try_borrow()?;
            if let Some(Backend::Xp4(session)) = &owner.backend {
                if pause_after.is_some()
                    || pause_at_tick.is_some()
                    || store_watches.is_some()
                    || device_watches.is_some()
                {
                    return Err(PyValueError::new_err(
                        "XP4 execution does not support pause boundaries",
                    ));
                }
                return session.run_board(count, quantum).map(|_| false);
            }
        }
        let mut owner = slf.try_borrow_mut()?;
        owner.begin_continuation(store_watches, device_watches)?;
        let result = owner.run_graph(py, count, quantum, pause_after, pause_at_tick);
        let cleanup = owner.board_mut()?.continuation_end().map_err(error);
        result.and(cleanup)
    }
    #[pyo3(signature = (end, quantum, pause_after=None, pause_at_tick=None, store_watches=None, device_watches=None))]
    fn run_window(
        &mut self,
        py: Python<'_>,
        end: u64,
        quantum: usize,
        pause_after: Option<u64>,
        pause_at_tick: Option<u64>,
        store_watches: Option<Vec<(u64, u64)>>,
        device_watches: Option<Vec<String>>,
    ) -> PyResult<bool> {
        if self.driver.is_some() {
            return Err(error(
                "Instruction windows require a whole-board run".into(),
            ));
        }
        self.begin_continuation(store_watches, device_watches)?;
        let result = run::window(
            py,
            self.board_mut()?,
            end,
            quantum,
            pause_after,
            pause_at_tick,
        );
        let cleanup = self.board_mut()?.continuation_end().map_err(error);
        self.outcome(result).and(cleanup)
    }
    fn reg_read(&self, core: usize, register: &str) -> PyResult<u64> {
        if let Some(Backend::Xp4(session)) = &self.backend {
            return session.read_register(core, json(register)?);
        }
        self.board()?
            .read_register(core, json::<Register>(register)?)
            .map_err(error)
    }
    fn reg_write(&self, core: usize, register: &str, value: u64) -> PyResult<()> {
        if let Some(Backend::Xp4(session)) = &self.backend {
            return session.write_register(core, json(register)?, value);
        }
        self.board()?
            .write_register(core, json::<Register>(register)?, value)
            .map_err(error)
    }
    fn coprocessor(&self, core: usize, selector: &str, value: Option<u64>) -> PyResult<u64> {
        if let Some(Backend::Xp4(session)) = &self.backend {
            return session.coprocessor(core, json(selector)?, value);
        }
        self.board()?
            .cpu(core)
            .map_err(error)?
            .coprocessor(json::<arm::Coprocessor>(selector)?, value)
            .map_err(error)
    }
    fn mem_read<'py>(
        &self,
        py: Python<'py>,
        core: usize,
        address: u64,
        size: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        self.check_read(size)?;
        if let Some(Backend::Xp4(session)) = &self.backend {
            let address = u32::try_from(address)
                .map_err(|_| PyValueError::new_err("ARMv7-A virtual address exceeds 32 bits"))?;
            return session.read_virtual(py, core, address, size);
        }
        PyBytes::new_with(py, size, |out| {
            self.board()?.read_memory(core, address, out).map_err(error)
        })
    }
    fn mem_write(&self, core: usize, address: u64, bytes: &[u8]) -> PyResult<()> {
        self.board()?
            .write_memory(core, address, bytes)
            .map_err(error)
    }
    fn physical_read<'py>(
        &self,
        py: Python<'py>,
        address: u64,
        size: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        self.check_read(size)?;
        if let Some(Backend::Xp4(session)) = &self.backend {
            let address = usize::try_from(address)
                .map_err(|_| PyValueError::new_err("Physical address exceeds host size"))?;
            return session.read(py, address, size);
        }
        PyBytes::new_with(py, size, |out| {
            self.board()?.read_physical(address, out).map_err(error)
        })
    }
    /// Read physical memory that an active wait observes but does not depend on.
    fn physical_inspect<'py>(
        &self,
        py: Python<'py>,
        address: u64,
        size: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        self.check_read(size)?;
        if let Some(Backend::Xp4(session)) = &self.backend {
            let address = usize::try_from(address)
                .map_err(|_| PyValueError::new_err("Physical address exceeds host size"))?;
            return session.read(py, address, size);
        }
        PyBytes::new_with(py, size, |out| {
            self.board()?.inspect_physical(address, out).map_err(error)
        })
    }
    /// Batch passive physical reads at one stopped execution boundary.
    fn physical_read_many<'py>(
        &self,
        py: Python<'py>,
        ranges: Vec<(u64, usize)>,
    ) -> PyResult<Vec<Bound<'py, PyBytes>>> {
        if ranges.len() > 4096 {
            return Err(PyValueError::new_err("Too many memory observation ranges"));
        }
        let total = ranges
            .iter()
            .try_fold(0usize, |sum, (_, size)| sum.checked_add(*size))
            .ok_or_else(|| PyValueError::new_err("Memory observation extent overflow"))?;
        self.check_read(total)?;
        ranges
            .into_iter()
            .map(|(address, size)| self.physical_read(py, address, size))
            .collect()
    }
    fn physical_write(&self, address: u64, bytes: &[u8]) -> PyResult<()> {
        if let Some(Backend::Xp4(session)) = &self.backend {
            let address = usize::try_from(address)
                .map_err(|_| PyValueError::new_err("Physical address exceeds host size"))?;
            return session.write(address, bytes);
        }
        self.board()?.write_physical(address, bytes).map_err(error)
    }
    fn input_level(&mut self, name: &str, high: bool) -> PyResult<()> {
        self.session_mut()?.logic_input(name, high).map_err(error)
    }
    fn gpio_input(&mut self, name: &str, port: u8, pin: u8, high: bool) -> PyResult<()> {
        self.session_mut()?
            .gpio(name, port, pin, high)
            .map_err(error)
    }
    fn hdmi_connection(&mut self, sink: &str, transmitter: &str, connected: bool) -> PyResult<()> {
        self.session_mut()?
            .hdmi_connection(sink, transmitter, connected)
            .map_err(error)
    }
    fn gpio_input_levels(&self, name: &str) -> PyResult<Vec<u32>> {
        Ok(self
            .session()?
            .gpio_input_levels(name)
            .map_err(error)?
            .to_vec())
    }
    fn gpio_level(&self, name: &str, port: u8, pin: u8) -> PyResult<bool> {
        self.session()?.gpio_level(name, port, pin).map_err(error)
    }
    fn gpio_pad_state(&self, name: &str, port: u8, pin: u8) -> PyResult<String> {
        serde_json::to_string(
            &self
                .session()?
                .gpio_pad_state(name, port, pin)
                .map_err(error)?,
        )
        .map_err(|e| error(e.to_string()))
    }
    fn start_host_profile(&self) -> PyResult<()> {
        self.board()?.start_host_profile().map_err(error)
    }
    fn host_profile(&self) -> PyResult<String> {
        serde_json::to_string(&self.board()?.host_profile()).map_err(|e| error(e.to_string()))
    }
    fn encoder_cycles(&mut self, name: &str, cycles: i32) -> PyResult<()> {
        self.session_mut()?.rotate(name, cycles).map_err(error)
    }
    fn image_rgb(&mut self, name: &str, width: usize, height: usize, rgb: &[u8]) -> PyResult<()> {
        self.session_mut()?
            .rgb(name, width, height, rgb)
            .map_err(error)
    }
    fn image_analytic(&mut self, name: &str) -> PyResult<()> {
        self.session_mut()?.analytic(name).map_err(error)
    }
    fn image_sensor(&mut self, name: &str) -> PyResult<()> {
        self.session_mut()?.sensor_image(name).map_err(error)
    }
    fn sensor_rgb(
        &self,
        name: &str,
        width: usize,
        height: usize,
        rgb: &[u8],
        white_luminance: f64,
    ) -> PyResult<()> {
        self.session()?
            .sensor_rgb(name, width, height, rgb, white_luminance)
            .map_err(error)
    }
    fn sensor_raw16(&self, name: &str, width: usize, height: usize, raw16: &[u8]) -> PyResult<()> {
        self.session()?
            .sensor_raw16(name, width, height, raw16)
            .map_err(error)
    }
    fn sensor_raf(&self, name: &str, raf: &[u8]) -> PyResult<()> {
        self.session()?.sensor_raf(name, raf).map_err(error)
    }
    fn card_initialize_revision(&mut self, name: &str, revision: u64) -> PyResult<()> {
        self.session_mut()?
            .initialize_card_revision(name, revision)
            .map_err(error)
    }
    fn card_contacts(&mut self, name: &str, present: bool, protected: bool) -> PyResult<()> {
        self.session_mut()?
            .card_contacts(name, present, protected)
            .map_err(error)
    }
    /// `fault` is a JSON data CRC fault: direction, sector, after_sectors, occurrences.
    fn card_data_fault(&mut self, name: &str, fault: &str) -> PyResult<()> {
        self.session_mut()?
            .card_data_fault(name, json(fault)?)
            .map_err(error)
    }
    fn card_clear_data_faults(&mut self, name: &str) -> PyResult<()> {
        self.session_mut()?
            .card_clear_data_faults(name)
            .map_err(error)
    }
    fn card_state(&self, name: &str) -> PyResult<String> {
        serde_json::to_string(&self.session()?.card_state(name).map_err(error)?)
            .map_err(|e| error(e.to_string()))
    }
    #[pyo3(signature = (name, revision=None))]
    fn card_tree(&self, name: &str, revision: Option<u64>) -> PyResult<String> {
        serde_json::to_string(&self.session()?.card_tree(name, revision).map_err(error)?)
            .map_err(|e| error(e.to_string()))
    }
    #[pyo3(signature = (name, path, revision=None, max_bytes=67108864))]
    fn card_read<'py>(
        &self,
        py: Python<'py>,
        name: &str,
        path: &str,
        revision: Option<u64>,
        max_bytes: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let bytes = self
            .session()?
            .card_read(name, path, revision, max_bytes)
            .map_err(error)?;
        Ok(PyBytes::new(py, &bytes))
    }
    fn card_format(
        &mut self,
        name: &str,
        options: &str,
        files: &Bound<'_, PyDict>,
    ) -> PyResult<()> {
        let files = files
            .iter()
            .map(|(k, v)| {
                Ok((
                    k.extract::<String>()?,
                    v.cast::<PyBytes>()?.as_bytes().to_vec(),
                ))
            })
            .collect::<PyResult<Vec<_>>>()?;
        self.session_mut()?
            .card_format(name, &json(options)?, &files)
            .map_err(error)
    }
    fn card_medium(&self, name: &str) -> PyResult<CardMedium> {
        Ok(CardMedium(
            self.session()?.card_medium(name).map_err(error)?,
        ))
    }
    fn nand_state(&self, name: &str) -> PyResult<String> {
        serde_json::to_string(&self.session()?.nand_state(name).map_err(error)?)
            .map_err(|e| error(e.to_string()))
    }
    fn nand_medium(&self, name: &str) -> PyResult<NandMedium> {
        Ok(NandMedium(
            self.session()?.nand_medium(name).map_err(error)?,
        ))
    }
    fn nand_retain(&mut self, name: &str, medium: &NandMedium) -> PyResult<()> {
        self.session_mut()?
            .retain_nand(name, medium.0.clone())
            .map_err(error)
    }
    fn card_insert(
        &mut self,
        name: &str,
        medium: &CardMedium,
        present: bool,
        protected: bool,
    ) -> PyResult<()> {
        self.session_mut()?
            .insert_card(name, medium.0.clone(), present, protected)
            .map_err(error)?;
        Ok(())
    }
    fn card_retain(
        &mut self,
        name: &str,
        medium: &CardMedium,
        present: bool,
        protected: bool,
    ) -> PyResult<()> {
        self.session_mut()?
            .retain_card(name, medium.0.clone(), present, protected)
            .map_err(error)
    }
    fn inputs_changed(&mut self, py: Python<'_>, name: &str) -> PyResult<()> {
        let now = self.board()?.now();
        if let Some(object) = self.objects.get(name) {
            object.refresh_deadline(py, now)?;
        }
        let id = self.session()?.node(name).map_err(error)?;
        self.board_mut()?.inputs_changed(id).map_err(error)
    }
    fn observation(&self, py: Python<'_>) -> PyResult<String> {
        if let Some(Backend::Xp4(session)) = &self.backend {
            return py
                .import("json")?
                .call_method1("dumps", (session.state(py, false)?,))?
                .extract();
        }
        serde_json::to_string(&self.board()?.observation().map_err(error)?)
            .map_err(|e| error(e.to_string()))
    }
    fn device_states(&self, kind: &str) -> PyResult<String> {
        serde_json::to_string(&self.board()?.device_states(kind).map_err(error)?)
            .map_err(|e| error(e.to_string()))
    }
    fn device_fingerprints(&self, kind: &str) -> PyResult<String> {
        serde_json::to_string(&self.board()?.device_fingerprints(kind).map_err(error)?)
            .map_err(|e| error(e.to_string()))
    }
    fn device_buffer<'py>(&self, py: Python<'py>, kind: &str, name: &str) -> PyResult<Bound<'py, PyBytes>> {
        let bytes = self.board()?.device_buffer(kind, name).map_err(error)?;
        Ok(PyBytes::new(py, &bytes))
    }
    fn state(&self, py: Python<'_>) -> PyResult<String> {
        if let Some(Backend::Xp4(session)) = &self.backend {
            return py
                .import("json")?
                .call_method1("dumps", (session.state(py, false)?,))?
                .extract();
        }
        let mut state = self.board()?.state();
        let serializer = py.import("json")?;
        for (name, object) in &self.objects {
            let snapshot = object.model.bind(py).call_method0("state")?;
            let text: String = serializer.call_method1("dumps", (snapshot,))?.extract()?;
            state["bus"]["devices"][self.session()?.nodes[name]]["state"] =
                json::<serde_json::Value>(&text)?;
        }
        serde_json::to_string(&state).map_err(|e| error(e.to_string()))
    }
    fn fingerprint(&self) -> PyResult<String> {
        serde_json::to_string(&self.session()?.fingerprint().map_err(error)?)
            .map_err(|e| error(e.to_string()))
    }
}

#[pyclass(module = "fujimod._native.emulator")]
pub struct CardMedium(fujimod_emulator_devices::parts::sd_card::Card);
#[pyclass(module = "fujimod._native.emulator")]
pub struct NandMedium(fujimod_emulator_devices::parts::nand::Nand);

pub(super) use context::DeviceContext;
pub(super) use storage::{normalize_card_files, persistent_packet};
pub(super) use xp4::Xp4;

#[pyclass(unsendable, module = "fujimod._native.emulator")]
pub struct AnalogInputs {
    inputs: fujimod_emulator_devices::parts::analog::Samples,
}
#[pymethods]
impl AnalogInputs {
    #[getter]
    fn samples(&self) -> Vec<u16> {
        self.inputs.values()
    }
    #[setter]
    fn set_samples(&self, values: Vec<u16>) -> PyResult<()> {
        self.inputs.replace(&values).map_err(error)
    }
}
