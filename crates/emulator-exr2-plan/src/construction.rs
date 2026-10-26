//! Construction of EXR II plan nodes. Each model crate registers a constructor per model
//! kind; the board builds every node through the registered [`Constructors`].
mod interface;
pub use interface::register_interface_models;

use crate::{BuiltBoard, Model};
use fujimod_emulator_exr2_resources::Resources;
use fujimod_emulator_devices::exr2::irq;
use fujimod_emulator_devices::{Device, bus::InterruptLine, graph::Mapping};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Decode a model's options, which plans carry as JSON for the model crate to read.
pub fn decode<T: serde::de::DeserializeOwned>(
    value: impl std::borrow::Borrow<serde_json::Value>,
) -> Result<T, String> {
    serde_json::from_value(value.borrow().clone()).map_err(|e| format!("EXR II model options: {e}"))
}

pub fn route(route: fujimod_emulator_devices::exr2::irq_routes::Irq) -> Result<fujimod_emulator_devices::exr2::irq_routes::Irq, String> {
    if route.core >= 2 || route.line >= 128 {
        return Err("EXR II interrupt route exceeds two cores/128 sources".into());
    }
    Ok(route)
}
pub fn line(line: InterruptLine) -> Result<InterruptLine, String> {
    if !matches!(
        line,
        InterruptLine::Core {
            core: 0..2,
            line: 0..128
        }
    ) {
        return Err("EXR II requires a per-core interrupt source".into());
    }
    Ok(line)
}
pub fn controllers(
    built: &BuiltBoard,
    names: Vec<String>,
) -> Result<Vec<(usize, irq::Inputs)>, String> {
    names
        .into_iter()
        .map(|name| {
            let node = built
                .node_ids
                .get(&name)
                .ok_or_else(|| format!("unknown interrupt controller {name}"))?;
            let controller = built
                .core_interrupts
                .iter()
                .find(|c| c.device == *node)
                .ok_or_else(|| format!("{name} is not an interrupt controller"))?;
            Ok((controller.core, controller.inputs.clone()))
        })
        .collect()
}

/// One plan node being constructed and the board state its constructor wires it to.
pub struct Builder<'a> {
    pub name: &'a str,
    pub id: usize,
    pub windows: &'a [Mapping],
    pub resources: &'a mut Resources,
    pub built: &'a mut BuiltBoard,
    pub start_tick: u64,
}
/// What a constructor makes of its node.
pub enum Built {
    /// One device on the node's declared windows.
    Device(Box<dyn Device>),
    /// Several devices, each on its own windows or attached without any. The
    /// node's name refers to `devices[public]`; `edges` are (target, source)
    /// wake dependencies between them.
    Nodes {
        devices: Vec<(Option<Vec<Mapping>>, Box<dyn Device>)>,
        public: usize,
        edges: Vec<(usize, usize)>,
    },
}

pub type Constructor = fn(Builder<'_>, Model) -> Result<Built, String>;
/// Builds one controller from the DMA channel nodes the board consolidates.
pub type DmaController =
    fn(Vec<(Vec<Mapping>, Box<dyn Device>)>) -> Result<Box<dyn Device>, String>;

/// Constructors by plan model kind.
#[derive(Default)]
pub struct Constructors {
    models: BTreeMap<&'static str, Constructor>,
    dma_controller: Option<DmaController>,
}
impl Constructors {
    /// Add the constructor of one model kind; each kind has one constructor.
    pub fn add(&mut self, kind: &'static str, constructor: Constructor) {
        assert!(
            self.models.insert(kind, constructor).is_none(),
            "two constructors for EXR II model {kind}"
        );
    }
    pub fn set_dma_controller(&mut self, constructor: DmaController) {
        assert!(
            self.dma_controller.replace(constructor).is_none(),
            "two EXR II DMA controllers"
        );
    }
    pub fn build(&self, builder: Builder<'_>, model: Model) -> Result<Built, String> {
        let kind = model.kind();
        let constructor = self
            .models
            .get(kind)
            .ok_or_else(|| format!("no constructor is registered for EXR II model {kind}"))?;
        constructor(builder, model)
    }
    pub fn dma_controller(
        &self,
        channels: Vec<(Vec<Mapping>, Box<dyn Device>)>,
    ) -> Result<Box<dyn Device>, String> {
        let constructor = self
            .dma_controller
            .ok_or("no EXR II DMA controller constructor is registered")?;
        constructor(channels)
    }
}

static CONSTRUCTORS: OnceLock<Constructors> = OnceLock::new();

/// Set the constructors boards build with; later calls are ignored. Final
/// crates call `fujimod_emulator_models::register`, which calls this.
pub fn set_constructors(constructors: fn() -> Constructors) {
    CONSTRUCTORS.get_or_init(constructors);
}

pub fn constructors() -> Result<&'static Constructors, String> {
    CONSTRUCTORS.get().ok_or_else(|| {
        "EXR II constructors are not registered; call fujimod_emulator_models::register()".into()
    })
}
