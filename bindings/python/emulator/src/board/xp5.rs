//! XP5 ownership on the common native board, with its declared physical inputs.
use super::*;

pub(super) struct Opened {
    pub session: fujimod_emulator_session::Session,
    pub inputs: fujimod_emulator_xp5_board::session::Inputs,
}

impl Board {
    pub(super) fn xp5_graph(
        graph: factory::Graph,
        devices: &str,
        assets: &Bound<'_, PyDict>,
        read_limit: usize,
        mode: Mode,
        continue_refusals: bool,
        engine: factory::Engine,
        definition: serde_json::Value,
    ) -> PyResult<Self> {
        if continue_refusals {
            return Err(PyValueError::new_err(
                "XP5 does not continue modeled refusals",
            ));
        }
        let mut loaded = BTreeMap::new();
        for (name, bytes) in assets.iter() {
            let bytes = bytes.cast::<PyBytes>()?;
            if bytes.as_bytes().len() > fujimod_emulator_storage::fat::MAX_PRELOAD_BYTES as usize {
                return Err(PyValueError::new_err("XP5 asset exceeds 512 MiB"));
            }
            loaded.insert(name.extract::<String>()?, bytes.as_bytes().to_vec());
        }
        let spec: fujimod_emulator_xp5_board::definition::Definition = json(devices)?;
        let (session, inputs) = spec
            .open(graph, &loaded, engine, mode, definition)
            .map_err(error)?;
        Ok(Self {
            driver: None,
            memory_watch: None,
            function_watch: None,
            instruction_points: BTreeMap::new(),
            backend: Some(Backend::Xp5(Box::new(Opened { session, inputs }))),
            objects: BTreeMap::new(),
            failures: Failures::default(),
            read_limit,
        })
    }
}

#[pymethods]
impl Board {
    fn xp5_gpio_input(&mut self, port: usize, pin: usize, high: bool) -> PyResult<()> {
        let Some(Backend::Xp5(opened)) = self.backend.as_mut() else {
            return Err(PyValueError::new_err(
                "Physical XP5 GPIO input requires an XP5 graph",
            ));
        };
        opened
            .inputs
            .gpio(&mut opened.session, port, pin, high)
            .map_err(error)
    }
    fn xp5_gpio_level(&self, port: usize, pin: usize) -> PyResult<bool> {
        let Some(Backend::Xp5(opened)) = self.backend.as_ref() else {
            return Err(PyValueError::new_err(
                "Physical XP5 GPIO input requires an XP5 graph",
            ));
        };
        opened.inputs.gpio_level(port, pin).map_err(error)
    }
    fn xp5_key(&mut self, at_ns: u64, key: &str, pressed: bool) -> PyResult<()> {
        let Some(Backend::Xp5(opened)) = self.backend.as_mut() else {
            return Err(PyValueError::new_err(
                "Physical XP5 key input requires an XP5 graph",
            ));
        };
        let key = fujimod_emulator_xp5_board::session::Key::parse(key).map_err(error)?;
        opened
            .inputs
            .key(&mut opened.session, at_ns, key, pressed)
            .map_err(error)
    }
}
