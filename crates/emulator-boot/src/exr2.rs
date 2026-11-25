//! Catalog-driven EXR II plan assembly.
//!
//! A body's hardware profile exports its native construction: the node
//! identities, register windows, dependencies and model wiring. The run
//! configuration supplies each node's startup options by node identity. This
//! module joins the two into the plan the EXR II board builds. Body differences
//! stay in catalog data; no identity or device value is chosen here.

use fujimod_catalog::ResolvedSelection;
use fujimod_catalog_configuration::Exr2RunScene;
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_exr2_plan::{BootEntry, Model, Plan};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// The body hardware profile whose exported construction a run builds, when
/// the configuration selects one that declares a construction.
pub(super) fn construction_profile(
    resolved: &ResolvedConfiguration,
) -> Option<(&str, &fujimod_catalog::profiles::BodyProfile)> {
    let configuration = &resolved.configuration;
    let profile_id = configuration
        .preset
        .as_ref()?
        .hardware_selection
        .as_ref()?
        .body_profile_id
        .as_str();
    let profile = resolved
        .reference
        .body
        .hardware
        .as_ref()?
        .profiles
        .get(profile_id)?;
    profile.construction.as_ref()?;
    Some((profile_id, profile))
}

/// Whether the run selects a body profile that exports a native construction.
pub(super) fn available(resolved: &ResolvedConfiguration) -> bool {
    construction_profile(resolved).is_some()
}

/// Build the closed EXR II plan a configuration's body profile exports.
pub(super) fn assemble(
    resolved: &ResolvedConfiguration,
    scene_input: &Exr2RunScene,
) -> Result<(Plan, Value, Value), String> {
    let configuration = &resolved.configuration;
    let selection = &resolved.reference;
    let preset = configuration
        .preset
        .as_ref()
        .ok_or_else(|| format!("run {} declares no preset", configuration.id))?;
    let (profile_id, profile) = construction_profile(resolved).ok_or_else(|| {
        format!(
            "body {} profile declares no exported native construction",
            selection.body.id
        )
    })?;
    let construction = profile
        .construction
        .as_ref()
        .expect("construction_profile checks the construction");
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
        if let Some(inputs) = preset.native_device_inputs.get(&node.name) {
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
    let mut wires = serde_json::to_value(&construction.wires)
        .map_err(|error| {
            format!(
                "body {} profile {profile_id} wires: {error}",
                selection.body.id
            )
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

    let scene_value = scene(selection, scene_input)?;
    Ok((plan, plan_value, scene_value))
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
