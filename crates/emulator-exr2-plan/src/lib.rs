//! Checked, CPU-independent EXR II board plans: nodes, wires, resources and
//! their checkpoints. `fujimod-emulator-exr2-board` constructs the device
//! models of a plan through the constructors model crates register in
//! [`construction`]; final crates call its `register_board_builder` and
//! `fujimod_emulator_models::register` so `Plan::build` reaches it. This crate
//! depends on no model crate, so a model edit rebuilds neither the plan nor the
//! board.
pub mod construction;
pub mod serial;
mod spec;
use fujimod_emulator_devices::{
    Device,
    graph::{DeviceGraph, Mapping, validate_mappings},
};
use fujimod_emulator_exr2_resources::{CoreInterrupts, Resources};
pub use spec::{
    Adapter, BootEntry, Card, FocusAxis, InputSignal, Iris, Model, Node, Plan, Sensor,
    SensorControls, SerialEndpoint, SerialRoute, SoftwareOutput, WireKind,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// The device graph built from a plan, with the node indices, interrupt
/// controllers and card contacts the emulator connects to its cores.
pub struct BuiltBoard {
    pub graph: DeviceGraph,
    pub node_ids: BTreeMap<String, usize>,
    pub core_interrupts: Vec<CoreInterrupts>,
    pub contacts: BTreeMap<String, fujimod_emulator_devices::parts::card_contacts::Contacts>,
    pub resources: Resources,
}

/// Constructs every device model of a plan.
pub type BoardBuilder =
    fn(Plan, Resources, u64, &mut dyn AdapterFactory) -> Result<BuiltBoard, String>;

static BOARD_BUILDER: OnceLock<BoardBuilder> = OnceLock::new();

/// Set the builder `Plan::build` calls; later calls are ignored.
pub fn set_board_builder(builder: BoardBuilder) {
    BOARD_BUILDER.get_or_init(|| builder);
}

/// Execution-side memory decoder ports available to this device graph.
pub struct MemoryChoices {
    pub initial: usize,
    pub choices: usize,
}
pub trait AdapterFactory {
    fn construct(
        &mut self,
        name: &str,
        spec: &Adapter,
        windows: &[Mapping],
        resources: &Resources,
        nodes: &BTreeMap<String, usize>,
    ) -> Result<Box<dyn Device>, String>;
}
pub struct RefuseAdapters;
impl AdapterFactory for RefuseAdapters {
    fn construct(
        &mut self,
        name: &str,
        spec: &Adapter,
        _: &[Mapping],
        _: &Resources,
        _: &BTreeMap<String, usize>,
    ) -> Result<Box<dyn Device>, String> {
        Err(format!(
            "{name} requires unavailable adapter {}",
            spec.implementation
        ))
    }
}
pub fn identifier(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}
impl Plan {
    pub fn validate_reset_outputs(&self, released: &[bool]) -> Result<(), String> {
        for node in self
            .nodes
            .iter()
            .filter(|n| n.boot_entries.contains(&self.boot_entry))
        {
            if let Model::CoreRelease { options, core, .. } = &node.model {
                if released.get(*core).copied() != options["released"].as_bool() {
                    return Err("core release reset state disagrees with execution graph".into());
                }
            }
        }
        Ok(())
    }
    pub fn validate_memory_outputs(&self, decoders: &[MemoryChoices]) -> Result<(), String> {
        for node in self
            .nodes
            .iter()
            .filter(|n| n.boot_entries.contains(&self.boot_entry))
        {
            if let Model::Block98000 { options, wiring } = &node.model {
                let decoder = wiring["decoder"]
                    .as_u64()
                    .and_then(|d| decoders.get(d as usize))
                    .ok_or("boot control has no memory decoder")?;
                let selections = wiring["selections"]
                    .as_array()
                    .ok_or("boot control has no selections")?;
                let choice = |s: &serde_json::Value| s["choice"].as_u64().map(|c| c as usize);
                if selections.iter().any(|s| choice(s).is_none_or(|c| c >= decoder.choices))
                    || !selections.iter().any(|s| {
                        s["word"] == options["selection_reset"] && choice(s) == Some(decoder.initial)
                    })
                {
                    return Err(
                        "boot control selections disagree with execution memory decoder".into(),
                    );
                }
            }
        }
        Ok(())
    }
    pub fn parse(json: &[u8]) -> Result<Self, String> {
        let plan: Self = serde_json::from_slice(json).map_err(|e| format!("EXR II graph: {e}"))?;
        plan.validate()?;
        Ok(plan)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 || self.nodes.is_empty() {
            return Err("unsupported or empty EXR II graph".into());
        }
        if self.wires.keys().any(|name| !identifier(name)) {
            return Err("invalid EXR II wire name".into());
        }
        let mut names = BTreeMap::new();
        for (index, node) in self.nodes.iter().enumerate() {
            if !identifier(&node.name)
                || names.insert(node.name.as_str(), index).is_some()
                || node.boot_entries.is_empty()
                || node.boot_entries.iter().collect::<BTreeSet<_>>().len()
                    != node.boot_entries.len()
            {
                return Err("invalid or duplicate EXR II node name/boot entries".into());
            }
        }
        for (index, node) in self.nodes.iter().enumerate() {
            if node
                .windows
                .iter()
                .any(|m| m.base.checked_add(m.size).is_none_or(|end| end > 1 << 32))
            {
                return Err(format!("{} has a non-ARM32 device window", node.name));
            }
            validate_mappings(&node.windows, std::iter::empty())?;
            let mut dependencies = BTreeSet::new();
            for dependency in &node.dependencies {
                if dependency == &node.name || !dependencies.insert(dependency) {
                    return Err("duplicate or self native dependency".into());
                }
            }
            for (name, constructed) in node
                .dependencies
                .iter()
                .map(|n| (n.as_str(), false))
                .chain(node.model.node_references().into_iter().map(|n| (n, true)))
            {
                let target = *names
                    .get(name)
                    .ok_or_else(|| format!("{} references absent {name}", node.name))?;
                if constructed && target >= index {
                    return Err(format!("{} is constructed before {name}", node.name));
                }
                if node
                    .boot_entries
                    .iter()
                    .any(|entry| !self.nodes[target].boot_entries.contains(entry))
                {
                    return Err(format!(
                        "{} depends on {name} outside its boot entries",
                        node.name
                    ));
                }
            }
        }
        for entry in [BootEntry::Rom, BootEntry::Loader] {
            let mut mappings: Vec<Mapping> = vec![];
            for node in self
                .nodes
                .iter()
                .filter(|n| n.boot_entries.contains(&entry))
            {
                validate_mappings(&node.windows, mappings.iter())?;
                mappings.extend_from_slice(&node.windows);
            }
        }
        if !self
            .nodes
            .iter()
            .any(|n| n.boot_entries.contains(&self.boot_entry))
        {
            return Err("EXR II boot entry selects no nodes".into());
        }
        Ok(())
    }
    pub fn build(
        self,
        resources: Resources,
        start_tick: u64,
        adapter: &mut dyn AdapterFactory,
    ) -> Result<BuiltBoard, String> {
        let build = BOARD_BUILDER.get().ok_or(
            "no EXR II board builder is registered: the final crate must call \
             fujimod_emulator_exr2_board::register_board_builder()",
        )?;
        build(self, resources, start_tick, adapter)
    }
}
