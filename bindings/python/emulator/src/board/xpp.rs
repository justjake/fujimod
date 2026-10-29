//! Native X-Processor Pro graph construction; no host device adapters.
use super::*;
impl Board {
    pub(super) fn xpp_graph(
        graph: factory::Graph,
        devices: &str,
        assets: &Bound<'_, PyDict>,
        read_limit: usize,
        mode: Mode,
        continue_refusals: bool,
        definition: serde_json::Value,
    ) -> PyResult<Self> {
        let mut loaded = BTreeMap::new();
        for (name, bytes) in assets.iter() {
            let bytes = bytes.cast::<PyBytes>()?;
            if bytes.as_bytes().len() > 128 << 20 {
                return Err(PyValueError::new_err(
                    "X-Processor Pro asset exceeds 128 MiB",
                ));
            }
            loaded.insert(name.extract::<String>()?, bytes.as_bytes().to_vec());
        }
        if continue_refusals {
            return Err(PyValueError::new_err(
                "X-Processor Pro does not continue modeled refusals",
            ));
        }
        let built = fujimod_emulator_xpp::board::build(graph, json(devices)?, &loaded, mode)
            .map_err(error)?;
        let mut resources = fujimod_emulator_exr2_resources::Resources::default();
        for (name, level, node) in built.inputs {
            if resources
                .wires
                .insert(
                    name.clone(),
                    fujimod_emulator_exr2_resources::Wire::LogicLevel(level),
                )
                .is_some()
            {
                return Err(PyValueError::new_err(
                    "Repeated X-Processor Pro physical input",
                ));
            }
            resources.users.entry(name).or_default().insert(node);
        }
        Ok(Self {
            driver: None,
            memory_watch: None,
            function_watch: None,
            instruction_points: BTreeMap::new(),
            backend: Some(Backend::Graph(fujimod_emulator_session::Session {
                definition,
                board: built.board,
                controls: None,
                contact_driver: None,
                resources,
                nodes: built.nodes,
                contacts: built.contacts,
                recording_frames: Default::default(),
            })),
            objects: BTreeMap::new(),
            failures: Failures::default(),
            read_limit,
            last_continuation_stop: None,
        })
    }
}
