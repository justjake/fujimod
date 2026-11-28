//! Catalog-driven EXR II plan assembly.
//!
//! A body's board exports its native construction: the node identities,
//! register windows, dependencies and model wiring, and each node's startup
//! options by node identity. This module joins them into the plan the EXR II
//! board builds. Body differences
//! stay in catalog data; no identity or device value is chosen here.

use fujimod_catalog::ResolvedSelection;
use fujimod_catalog_configuration::{Exr2RunScene, RunVirtualCard};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_exr2_plan::{BootEntry, Model, Plan};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// The body's board, when it exports a native construction.
pub(super) fn construction_board(
    resolved: &ResolvedConfiguration,
) -> Option<&fujimod_catalog::profiles::BodyBoard> {
    resolved.board().filter(|board| board.construction.is_some())
}

/// Whether the body's board exports a native construction.
pub(super) fn available(resolved: &ResolvedConfiguration) -> bool {
    construction_board(resolved).is_some()
}

/// Build the closed EXR II plan the body's board exports.
pub(super) fn assemble(
    resolved: &ResolvedConfiguration,
    scene_input: &Exr2RunScene,
) -> Result<(Plan, Value, Value), String> {
    let configuration = &resolved.configuration;
    let selection = &resolved.reference;
    let preset = configuration.preset.as_ref();
    let virtual_card = preset.and_then(|preset| preset.virtual_card.as_ref());
    let board = construction_board(resolved).ok_or_else(|| {
        format!(
            "body {} board declares no exported native construction",
            selection.body.id
        )
    })?;
    let construction = board
        .construction
        .as_ref()
        .expect("construction_board checks the construction");
    let mut nodes = Vec::with_capacity(construction.nodes.len());
    for node in &construction.nodes {
        let mut model = serde_json::to_value(&node.model)
            .map_err(|error| format!("node {}: {error}", node.name))?;
        let kind = model.get("kind").cloned().unwrap_or(Value::Null);
        let kind_name = kind.as_str().unwrap_or_default();
        // A construction's `hardware` facts are part of the plan model's
        // startup options: the I2C node keeps its registers and parts there.
        let mut options = match model.get("hardware").cloned() {
            Some(hardware) => {
                model
                    .as_object_mut()
                    .ok_or_else(|| format!("node {} model is not an object", node.name))?
                    .remove("hardware");
                hardware
                    .as_object()
                    .cloned()
                    .ok_or_else(|| format!("node {} hardware is not an object", node.name))?
            }
            None => serde_json::Map::new(),
        };
        let mut configured = !options.is_empty();
        if let Some(inputs) = board.native_device_inputs.get(&node.name) {
            configured = true;
            let inputs = serde_json::to_value(inputs)
                .map_err(|error| format!("node {} inputs: {error}", node.name))?;
            if inputs.get("kind") != Some(&kind) {
                return Err(format!(
                    "node {} inputs select {} but its construction is {kind}",
                    node.name,
                    inputs.get("kind").unwrap_or(&Value::Null)
                ));
            }
            let inputs = inputs
                .as_object()
                .cloned()
                .ok_or_else(|| format!("node {} inputs are not an object", node.name))?;
            options.extend(inputs.into_iter().filter(|(key, _)| key != "kind"));
        }
        if configured {
            if Model::carries_options(kind_name) {
                model["options"] = Value::Object(options);
            } else if !options.is_empty() {
                return Err(format!(
                    "node {} inputs declare options but its {kind_name} model reads none",
                    node.name
                ));
            }
        }
        nodes.push(json!({
            "name": node.name,
            "boot_entries": node.boot_entries,
            "windows": node.windows,
            "dependencies": node.dependencies,
            "model": model,
        }));
    }
    // A run that declares a virtual card replaces any clock-only node with the
    // card host. The host owns the SDHCI clock and reset windows.
    if let Some(card) = virtual_card {
        nodes.retain(|node| node.get("name").and_then(Value::as_str) != Some("sd_clocks"));
        let high_speed = preset.and_then(|preset| preset.factory_options.sd_card_profile.clone())
            == Some(
                fujimod_catalog_run_inputs::RunSdCardProfile::DeclaredBlankSdhcHighSpeedUnmeasured,
            );
        card_node(selection, card, high_speed, &mut nodes)?;
    }
    let mut wires = serde_json::to_value(&construction.wires)
        .map_err(|error| {
            format!("body {} board wires: {error}", selection.body.id)
        })?
        .as_object()
        .cloned()
        .ok_or_else(|| "construction wires are not an object".to_string())?;

    let mut names = BTreeSet::new();
    for node in &nodes {
        let name = node
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("run {} plan node has no name", configuration.id))?;
        if !names.insert(name.to_owned()) {
            return Err(format!(
                "run {} construction repeats node {name}",
                configuration.id
            ));
        }
    }

    let browser = configuration.browser_camera.is_some();
    if browser {
        wires.insert("sensor_exposure".into(), json!("exposure"));
        wires.insert("sensor_frame_period".into(), json!("frame-period"));
        wires.insert("sensor_statistics_grid".into(), json!("image-grid"));
    }
    let plan_value = json!({
        "schema": 1,
        "boot_entry": "rom",
        "wires": wires,
        "nodes": nodes,
    });
    let plan: Plan = serde_json::from_value(plan_value.clone())
        .map_err(|error| format!("body {} plan: {error}", selection.body.id))?;
    plan.validate()?;
    debug_assert!(matches!(plan.boot_entry, BootEntry::Rom));

    let mut scene_value = scene(selection, scene_input)?;
    if virtual_card.is_some() {
        scene_value["files"] = json!({"virtual_card_files": {}});
    }
    Ok((plan, plan_value, scene_value))
}

/// The SDHC card host with its virtual card, wired through the body's slot.
fn card_node(
    selection: &ResolvedSelection,
    card: &RunVirtualCard,
    high_speed: bool,
    nodes: &mut Vec<Value>,
) -> Result<(), String> {
    let slot = selection
        .body
        .card_slot
        .as_ref()
        .ok_or_else(|| format!("body {} declares no card slot", selection.body.id))?;
    let (detect, present_high) = card_contact(selection, &slot.detect_connection_id)?;
    let (protect, protected_high) = card_contact(selection, &slot.write_protect_connection_id)?;
    let present = true;
    let protected = false;
    let windows = ["sdhci-card-0", "sdhci-card-2", "sdhci-card-1"]
        .iter()
        .map(|id| {
            let block = soc_block(selection, id)?;
            Ok(json!({"base": block.base, "size": block.size}))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let host = soc_block(selection, "sdhci-card-0")?;
    let interrupt = host
        .interrupts
        .iter()
        .find(|interrupt| interrupt.output == "host")
        .ok_or_else(|| format!("SoC {} card host has no interrupt", selection.soc.id))?;
    let core = interrupt
        .core
        .as_ref()
        .ok_or_else(|| format!("SoC {} card host interrupt names no core", selection.soc.id))?;
    let core = selection
        .soc
        .processors
        .iter()
        .position(|processor| &processor.id == core)
        .ok_or_else(|| format!("SoC {} has no processor core {core}", selection.soc.id))?;
    let node = json!({
        "name": "sd",
        "boot_entries": ["rom", "loader"],
        "windows": windows,
        "dependencies": ["serial_gpio"],
        "model": {
            "kind": "sd-host",
            "options": {
                "base": host.base,
                "protocol": "sdhc",
                "transfer_ticks": 100,
                "clock_ticks": 2,
                "rca": 4660,
                "ram_bytes": selection.body.dram_bytes,
                "high_speed": high_speed,
            },
            "card": {
                "kind": "formatted",
                "options": {
                    "sectors": card.sectors,
                    "max_dirty_sectors": 1 << 20,
                    "label": card.label,
                    "partition_start": card.partition_start,
                    "sectors_per_cluster": card.sectors_per_cluster,
                },
                "files": "virtual_card_files",
            },
            "pins": {
                "gpio": "serial_gpio",
                "detect": {"port": detect.0, "pin": detect.1},
                "present_high": present_high,
                "protect": {"port": protect.0, "pin": protect.1},
                "protected_high": protected_high,
            },
            "present": present,
            "protected": protected,
            "irq": {"Core": {"core": core, "line": interrupt.line}},
        },
    });
    set_contact_level(nodes, detect, present, present_high)?;
    set_contact_level(nodes, protect, protected, protected_high)?;
    nodes.push(node);
    Ok(())
}

/// Set one card contact's reset pad level in the constructed GPIO model.
fn set_contact_level(
    nodes: &mut [Value],
    (port, pin): (u8, u8),
    asserted: bool,
    active_high: bool,
) -> Result<(), String> {
    let node = nodes
        .iter_mut()
        .find(|node| node.get("name").and_then(Value::as_str) == Some("serial_gpio"))
        .ok_or("a card slot needs the body's serial GPIO node")?;
    let levels = node["model"]["options"]["input_levels"]
        .as_array_mut()
        .ok_or("the serial GPIO node declares no input levels")?;
    let word = levels
        .get_mut(usize::from(port))
        .ok_or("a card contact names a port outside the GPIO input levels")?;
    let mut value = u32::try_from(word.as_u64().unwrap_or(0))
        .map_err(|_| "the GPIO input level exceeds one port word")?;
    if asserted == active_high {
        value |= 1 << pin;
    } else {
        value &= !(1 << pin);
    }
    *word = json!(value);
    Ok(())
}

fn soc_block<'a>(
    selection: &'a ResolvedSelection,
    id: &str,
) -> Result<&'a fujimod_catalog::reference::SocBlock, String> {
    selection
        .soc
        .blocks
        .iter()
        .find(|block| block.id == id)
        .ok_or_else(|| format!("SoC {} has no block {id}", selection.soc.id))
}

/// One GPIO contact of a card slot connection and its active level.
fn card_contact(selection: &ResolvedSelection, id: &str) -> Result<((u8, u8), bool), String> {
    let connection = selection
        .body
        .connections
        .iter()
        .find(|connection| connection.id == id)
        .ok_or_else(|| format!("body {} has no connection {id}", selection.body.id))?;
    match connection.signals.as_slice() {
        [
            fujimod_catalog::reference::ConnectionSignal::Gpio {
                port,
                pin,
                active_level,
            },
        ] => Ok(((*port, *pin), *active_level)),
        _ => Err(format!(
            "card contact {id} must declare exactly one GPIO signal"
        )),
    }
}

/// The declared scene: its images, statistics spans and NAND medium.
pub(super) fn scene(selection: &ResolvedSelection, scene: &Exr2RunScene) -> Result<Value, String> {
    let nand = selection
        .body
        .nand
        .ok_or_else(|| format!("body {} declares no NAND geometry", selection.body.id))?;
    let image = &scene.images.live_image;
    let statistics = scene
        .statistics
        .live_statistics
        .iter()
        .map(|span| json!({"destination":span.destination,"offset":span.offset,"asset":span.asset}))
        .collect::<Vec<_>>();
    Ok(json!({
        "images": { "live_image": {
            "width": image.width,
            "height": image.height,
            "stride": image.stride,
            "asset": image.asset,
        }},
        "statistics": { "live_statistics": statistics },
        "nands": { "nand": {
            "kind": "raw",
            "options": {
                "bytes": nand.bytes,
                "page_bytes": nand.page_bytes,
                "pages_per_block": nand.pages_per_block,
                "spare_bytes": nand.spare_bytes,
            },
            "base": "nand_base",
            "overlays": [],
            "spare": [],
        }},
    }))
}
