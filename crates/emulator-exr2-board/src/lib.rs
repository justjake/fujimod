//! EXR II board construction: builds every node of a checked plan with its
//! real Rust constructor. A remaining language implementation is an explicit
//! adapter node supplied by the native frontend; absent adapters refuse. Final
//! crates call [`register_board_builder`] so `Plan::build` reaches this crate.
#[cfg(test)]
mod tests;
use fujimod_emulator_devices::graph::{DeviceGraph, Node as GraphNode, validate_mappings};
use fujimod_emulator_exr2_plan::construction::{Builder, Built, Constructors, constructors};
use fujimod_emulator_exr2_plan::{AdapterFactory, BuiltBoard, Model, Plan};
use fujimod_emulator_exr2_resources::{Resources, Wire};
use std::collections::{BTreeMap, BTreeSet};

/// Make `Plan::build` construct boards with this crate. Final crates call
/// this before building any EXR II board.
pub fn register_board_builder() {
    fujimod_emulator_exr2_plan::set_board_builder(build);
}

struct DmaMerge {
    remap: Vec<usize>,
}

fn consolidate_dma(
    constructors: &Constructors,
    graph: &mut DeviceGraph,
    node_ids: &mut BTreeMap<String, usize>,
    resources: &mut Resources,
) -> Result<DmaMerge, String> {
    const BASE: u64 = 0xfff02000;
    const END: u64 = BASE + 0x200;
    const GLOBAL: u64 = BASE + 0x300;
    let old_nodes = std::mem::take(&mut graph.nodes);
    let selected: BTreeSet<usize> = old_nodes
        .iter()
        .enumerate()
        .filter_map(|(id, node)| {
            node.mappings
                .iter()
                .any(|mapping| {
                    (mapping.base >= BASE && mapping.base < END)
                        || mapping.base == GLOBAL && mapping.size == 4
                })
                .then_some(id)
        })
        .collect();
    if selected.is_empty() {
        graph.nodes = old_nodes;
        return Ok(DmaMerge {
            remap: (0..graph.nodes.len()).collect(),
        });
    }

    let first = *selected.first().unwrap();
    let mut controller_mappings = Vec::new();
    for &id in &selected {
        let node = &old_nodes[id];
        controller_mappings.extend_from_slice(&node.mappings);
    }
    controller_mappings.sort_by_key(|mapping| mapping.base);
    validate_mappings(&controller_mappings, std::iter::empty())?;
    let mut old_nodes: Vec<Option<GraphNode>> = old_nodes.into_iter().map(Some).collect();
    let mut channels = Vec::with_capacity(selected.len());
    for id in selected.iter().copied() {
        let node = old_nodes[id].take().unwrap();
        channels.push((node.mappings, node.device));
    }
    let mut controller = Some(constructors.dma_controller(channels)?);
    let mut remap = vec![usize::MAX; old_nodes.len()];
    let mut rebuilt = Vec::with_capacity(old_nodes.len() - selected.len() + 1);
    for old_id in 0..old_nodes.len() {
        if old_id == first {
            let index = rebuilt.len();
            remap[first] = index;
            rebuilt.push(GraphNode {
                mappings: controller_mappings.clone(),
                device: controller.take().unwrap(),
                native_dependencies: vec![],
            });
        } else if selected.contains(&old_id) {
            remap[old_id] = remap[first];
        } else {
            remap[old_id] = rebuilt.len();
            rebuilt.push(old_nodes[old_id].take().unwrap());
        }
    }
    graph.nodes = rebuilt;
    for id in node_ids.values_mut() {
        *id = remap[*id];
    }
    for users in resources
        .users
        .values_mut()
        .chain(resources.image_users.values_mut())
    {
        let mapped = users.iter().map(|id| remap[*id]).collect();
        *users = mapped;
    }
    Ok(DmaMerge { remap })
}

fn build(
    plan: Plan,
    mut resources: Resources,
    start_tick: u64,
    adapter: &mut dyn AdapterFactory,
) -> Result<BuiltBoard, String> {
    plan.validate()?;
    let constructors = constructors()?;
    if !resources.wires.is_empty()
        || !resources.users.is_empty()
        || !resources.image_users.is_empty()
        || !resources.gpio.is_empty()
        || !resources.pulses.is_empty()
        || !resources.controllers.is_empty()
        || !resources.encoders.is_empty()
        || !resources.hdmi_sinks.is_empty()
        || !resources.clocks.is_empty()
        || !resources.software_sources.is_empty()
    {
        return Err("EXR II resources must be fresh and explicitly named".into());
    }
    if !fujimod_emulator_exr2_images::images_mut(&mut resources)?
        .wires
        .is_empty()
    {
        return Err("EXR II resources must be fresh and explicitly named".into());
    }
    for (name, kind) in plan.wires {
        use fujimod_emulator_exr2_images::ImageWire;
        use fujimod_emulator_exr2_plan::WireKind;
        let image = match kind {
            WireKind::LogicLevel(high) => Err(Wire::logic_level(high)),
            WireKind::SoftwarePulse => Err(Wire::software_pulse()),
            WireKind::Parameters => Ok(ImageWire::parameters()),
            WireKind::MemoryOutputs => Ok(ImageWire::memory_outputs()),
            WireKind::Stream => Ok(ImageWire::stream()),
            WireKind::OutputChannels => Ok(ImageWire::output_channels()),
            WireKind::IrqRoutes => Err(Wire::irq_routes()),
            WireKind::PhaseEdges => Ok(ImageWire::phase_edges()),
            WireKind::BridgeRegisters => Err(Wire::bridge_registers()),
            WireKind::OpticalPosition => Err(Wire::optical_position()),
            WireKind::Exposure => Ok(ImageWire::exposure()),
            WireKind::Focus => Err(Wire::focus()),
            WireKind::FramePeriod => Ok(ImageWire::frame_period()),
            WireKind::ImageGrid => Ok(ImageWire::image_grid()),
            WireKind::Aperture => Err(Wire::aperture()),
        };
        match image {
            Ok(wire) => {
                fujimod_emulator_exr2_images::images_mut(&mut resources)?
                    .wires
                    .insert(name, wire);
            }
            Err(wire) => {
                resources.wires.insert(name, wire);
            }
        }
    }
    let mut built = BuiltBoard {
        graph: DeviceGraph::new(),
        node_ids: BTreeMap::new(),
        core_interrupts: vec![],
        contacts: BTreeMap::new(),
        resources: Resources::default(),
    };
    let mut dependencies = vec![];
    let mut direct_dependencies = vec![];
    for node in plan
        .nodes
        .into_iter()
        .filter(|n| n.boot_entries.contains(&plan.boot_entry))
    {
        let id = built.graph.nodes.len();
        let mut public_id = id;
        let refs = node
            .model
            .node_references()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let non_mmio = matches!(
            node.model,
            Model::Encoder { .. }
                | Model::ContactActuator { .. }
                | Model::GpioContactResponse { .. }
                | Model::I2cMotorBridge { .. }
                | Model::CoreRelease { .. }
                | Model::ExternalFrames { .. }
                | Model::HdmiSink { .. }
        );
        let single = !matches!(
            node.model,
            Model::Adapter { .. } | Model::Power { .. } | Model::SerialDmaLink { .. }
        );
        if single && non_mmio != node.windows.is_empty() {
            return Err(format!(
                "{} has incorrect MMIO/non-MMIO ownership",
                node.name
            ));
        }
        let made = if let Model::Adapter { adapter: spec } = &node.model {
            for name in &spec.wires {
                resources.connect(name, id)?;
            }
            Built::Device(adapter.construct(
                &node.name,
                spec,
                &node.windows,
                &mut resources,
                &built.node_ids,
            )?)
        } else {
            constructors.build(
                Builder {
                    name: &node.name,
                    id,
                    windows: &node.windows,
                    resources: &mut resources,
                    built: &mut built,
                    start_tick,
                },
                node.model,
            )?
        };
        match made {
            Built::Device(device) => {
                let actual = built.graph.register_windows(node.windows, device)?;
                assert_eq!(actual, id);
            }
            Built::Nodes {
                devices,
                public,
                edges,
            } => {
                let mut ids = Vec::with_capacity(devices.len());
                for (mappings, device) in devices {
                    ids.push(match mappings {
                        Some(mappings) => built.graph.register_windows(mappings, device)?,
                        None => built.graph.attach(device),
                    });
                }
                assert_eq!(ids, (id..id + ids.len()).collect::<Vec<_>>());
                public_id = ids[public];
                direct_dependencies.extend(
                    edges
                        .into_iter()
                        .map(|(target, source)| (ids[target], ids[source])),
                );
            }
        }
        dependencies.push((
            public_id,
            node.dependencies
                .into_iter()
                .chain(refs)
                .collect::<BTreeSet<_>>(),
        ));
        built.node_ids.insert(node.name, public_id);
    }
    let merged = consolidate_dma(
        constructors,
        &mut built.graph,
        &mut built.node_ids,
        &mut resources,
    )?;
    let mut edges = vec![BTreeSet::new(); built.graph.nodes.len()];
    for (old_id, names) in dependencies {
        let id = merged.remap[old_id];
        for name in names {
            let source = *built
                .node_ids
                .get(&name)
                .ok_or("native dependency absent for selected boot entry")?;
            if source != id {
                edges[id].insert(source);
            }
        }
    }
    for (old_target, old_source) in direct_dependencies {
        let target = merged.remap[old_target];
        let source = merged.remap[old_source];
        if target != source {
            edges[target].insert(source);
        }
    }
    for users in resources.users.values() {
        for &id in users {
            edges[id].extend(users.iter().copied().filter(|&source| source != id));
        }
    }
    for (id, sources) in edges.into_iter().enumerate() {
        built.graph.depend_on(id, sources.into_iter().collect())?;
    }
    built.resources = resources;
    Ok(built)
}
