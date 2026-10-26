//! Shared EXR II construction from the checked runtime/device plans. Python
//! transition adapters refuse before CPU construction on every target.
use super::session::{Firmware, Session, decode};
use fujimod_emulator::board::{
    bus::{Bus, Mode},
    factory,
};
use fujimod_emulator_exr2_plan::{MemoryChoices, Plan, RefuseAdapters};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Config {
    schema: u32,
    graph: Graph,
    firmware_id: String,
    boot_source: BootSource,
    #[serde(default)]
    initial_positions: BTreeMap<String, String>,
    #[serde(default)]
    boot_inputs: Vec<super::lifecycle::BootInput>,
    clock_seconds2000: Option<u32>,
}
// Select the same constructor identity as the native client without retaining
// transport byte arrays or reparsing those arrays into a second asset tree.
#[derive(Deserialize)]
struct DefinitionEnvelope {
    graph: Definition,
}
#[derive(Deserialize, Serialize)]
struct Definition {
    execution: serde_json::Value,
    devices: serde_json::Value,
    #[serde(default = "empty_object")]
    scene: serde_json::Value,
}
fn empty_object() -> serde_json::Value {
    serde_json::json!({})
}
#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum BootSource {
    Internal,
    Sd,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Graph {
    schema: u32,
    firmware_id: String,
    boot_source: BootSource,
    execution: factory::Graph,
    devices: Plan,
    #[serde(default)]
    scene: fujimod_emulator_preparation::Spec,
    #[serde(default)]
    assets: BTreeMap<String, Asset>,
    firmware: Firmware,
    quantum: usize,
    #[serde(default)]
    coprocessors: Vec<Coprocessor>,
    #[serde(default)]
    gpio: Vec<Gpio>,
    #[serde(default)]
    watch_pcs: Vec<u64>,
    card: Option<Card>,
    scene_input: Option<Scene>,
    controls: Option<fujimod_emulator_devices::parts::switch_panel::Spec>,
    contact_driver: Option<fujimod_emulator_session::contact_driver::Spec>,
    preview: Option<Preview>,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Asset {
    Bytes {
        bytes: Vec<u8>,
    },
    Dat {
        offset: u64,
        size: u64,
        inverted: bool,
    },
    Repeat {
        pattern: Vec<u8>,
        size: usize,
    },
}
impl Asset {
    fn build(self, dat: &[u8]) -> Result<Vec<u8>, String> {
        match self {
            Self::Bytes { bytes } if bytes.len() <= 128 << 20 => Ok(bytes),
            Self::Dat {
                offset,
                size,
                inverted,
            } if size <= 128 << 20 => {
                let bytes =
                    fujimod_firmware::extent(dat, offset, size).map_err(|e| e.to_string())?;
                Ok(if inverted {
                    fujimod_firmware::invert(bytes)
                } else {
                    bytes.to_vec()
                })
            }
            Self::Repeat { pattern, size }
                if size <= 128 << 20 && !pattern.is_empty() && size % pattern.len() == 0 =>
            {
                Ok(pattern.repeat(size / pattern.len()))
            }
            _ => Err("invalid or oversized scene asset".into()),
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Coprocessor {
    core: usize,
    register: fujimod_emulator_cpu::arm::Coprocessor,
    value: Option<u64>,
    #[serde(default)]
    set_bits: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Gpio {
    node: String,
    port: u8,
    pin: u8,
    high: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Card {
    pub node: String,
    pub format: fujimod_emulator_storage::fat::Options,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Scene {
    pub images: String,
    pub sensor: Option<String>,
    pub white_luminance: Option<f64>,
    pub node: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Preview {
    pub node: String,
    pub tick_hz: u64,
    pub period_ticks: u64,
    pub cadence: fujimod_emulator_devices::capability::StatisticsCadence,
}
pub(super) struct Metadata {
    pub card: Option<Card>,
    pub storage: fujimod_emulator_session::durable::Spec,
    pub identity: [u8; 32],
    pub scene: Option<Scene>,
    pub scanout: Option<String>,
    pub preview: Option<Preview>,
}

/// Browser fields applied after boot-time construction, shared by both constructors.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrowserFields {
    #[serde(default)]
    watch_pcs: Vec<u64>,
    #[serde(default)]
    initial_positions: BTreeMap<String, String>,
    #[serde(default)]
    boot_inputs: Vec<super::lifecycle::BootInput>,
    card: Option<Card>,
    scene_input: Option<Scene>,
    controls: Option<fujimod_emulator_devices::parts::switch_panel::Spec>,
    contact_driver: Option<fujimod_emulator_session::contact_driver::Spec>,
    preview: Option<Preview>,
}

/// A prepared run and its browser fields, `{schema, prepared, browser}`.
/// `prepared` is `fujimod_emulator_boot::Prepared::to_json` output.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreparedConfig {
    schema: u32,
    prepared: serde_json::Value,
    browser: BrowserFields,
}

/// Preserve the prepared document's numeric representation while forwarding run inputs.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreparedRequest {
    document: String,
    clock_seconds2000: Option<u32>,
    #[serde(default)]
    preferred_languages: Vec<String>,
    /// The camera's zstd-compressed NAND sections, in the order their bytes are
    /// packed in the camera span: section name and compressed length.
    #[serde(default)]
    camera: Vec<(String, usize)>,
}

/// Split the packed camera span into its sections by the request's lengths.
fn camera_sections(
    sections: &[(String, usize)],
    packed: &[u8],
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut camera = BTreeMap::new();
    let mut rest = packed;
    for (name, length) in sections {
        let (bytes, tail) = rest
            .split_at_checked(*length)
            .ok_or("camera sections exceed the camera span")?;
        if camera.insert(name.clone(), bytes.to_vec()).is_some() {
            return Err("camera sections repeat".into());
        }
        rest = tail;
    }
    if !rest.is_empty() {
        return Err("camera span holds bytes no section declares".into());
    }
    Ok(camera)
}

/// Everything a constructed camera needs beyond its boot-time core session.
struct Attachments {
    quantum: usize,
    gpio: Vec<Gpio>,
    watch_pcs: Vec<u64>,
    card: Option<Card>,
    scene: Option<Scene>,
    preview: Option<Preview>,
    controls: Option<fujimod_emulator_devices::parts::switch_panel::Spec>,
    contact_driver: Option<fujimod_emulator_session::contact_driver::Spec>,
    storage: fujimod_emulator_session::durable::Spec,
    identity: [u8; 32],
    scanout: Option<String>,
    initial_positions: BTreeMap<String, String>,
    boot_inputs: Vec<super::lifecycle::BootInput>,
}

/// Scanout and NAND roles selected by the plan's boot entry.
fn device_roles(plan: &Plan) -> Result<(Option<String>, Vec<String>), String> {
    let scanouts = plan
        .nodes
        .iter()
        .filter(|n| {
            n.boot_entries.contains(&plan.boot_entry)
                && matches!(n.model, fujimod_emulator_exr2_plan::Model::Scanout { .. })
        })
        .map(|n| n.name.clone())
        .collect::<Vec<_>>();
    let scanout = match scanouts.as_slice() {
        [] => None,
        [name] => Some(name.clone()),
        _ => return Err("session has multiple scanout devices".into()),
    };
    let nands = plan
        .nodes
        .iter()
        .filter(|n| {
            n.boot_entries.contains(&plan.boot_entry)
                && matches!(n.model, fujimod_emulator_exr2_plan::Model::Nand { .. })
        })
        .map(|n| n.name.clone())
        .collect();
    Ok((scanout, nands))
}

/// Storage identity: the firmware identity, its DAT hash and the declared storage roles.
fn camera_identity(
    firmware_id: &str,
    firmware_sha256: &str,
    storage: &fujimod_emulator_session::durable::Spec,
) -> Result<[u8; 32], String> {
    Ok(Sha256::digest(
        serde_json::to_vec(&serde_json::json!({
            "firmwareId": firmware_id,
            "firmware": firmware_sha256,
            "storage": storage,
        }))
        .map_err(|e| e.to_string())?,
    )
    .into())
}

impl Session {
    /// Build a camera from the native prepared run: the same boot-time construction
    /// as `boot::open_prepared`, with the browser's card, scene, preview and controls.
    pub fn create_prepared(
        document: &[u8],
        update: Vec<u8>,
        boot_rom: Vec<u8>,
        nand: Option<Vec<u8>>,
        settings: Option<Vec<u8>>,
        camera: &[u8],
        policy: fujimod_emulator_cpu::ExecutionPolicy,
    ) -> Result<Self, String> {
        let construction = super::snapshot::Construction::prepared(
            document,
            &update,
            &boot_rom,
            nand.as_deref(),
            settings.as_deref(),
            camera,
        );
        let request: PreparedRequest =
            serde_json::from_slice(document).map_err(|e| e.to_string())?;
        let camera = camera_sections(&request.camera, camera)?;
        let config: PreparedConfig =
            serde_json::from_str(&request.document).map_err(|e| e.to_string())?;
        if config.schema != 1 {
            return Err("prepared camera document schema mismatch".into());
        }
        let quantum = config.prepared["quantum"]
            .as_u64()
            .and_then(|q| usize::try_from(q).ok())
            .filter(|q| *q > 0)
            .ok_or("prepared run has no positive quantum")?;
        let firmware_id = config.prepared["firmware_id"]
            .as_str()
            .ok_or("prepared run has no firmware identity")?
            .to_owned();
        let firmware_sha256 =
            config.prepared["resolved"]["reference"]["firmware"]["update"]["sha256"]
                .as_str()
                .ok_or("prepared run has no firmware update identity")?
                .to_owned();
        let plan = Plan::parse(
            &serde_json::to_vec(&config.prepared["devices"]).map_err(|e| e.to_string())?,
        )?;
        let (scanout, nands) = device_roles(&plan)?;
        fujimod_emulator_exr2_board::register_board_builder();
        fujimod_emulator_models::register();
        let prepared = fujimod_emulator_boot::Prepared::from_json(&config.prepared)?;
        let mut core = fujimod_emulator_boot::open_prepared(
            prepared,
            fujimod_emulator_boot::InputBytes {
                update,
                boot_rom,
                nand,
                settings,
                clock_seconds_2000: request.clock_seconds2000,
                preferred_languages: request.preferred_languages,
                camera,
            },
        )?;
        core.board.set_execution_policy(policy)?;
        let browser = config.browser;
        let storage = fujimod_emulator_session::durable::Spec {
            cards: browser
                .card
                .as_ref()
                .map(|c| vec![c.node.clone()])
                .unwrap_or_default(),
            nands,
        };
        let identity = camera_identity(&firmware_id, &firmware_sha256, &storage)?;
        let mut session = Self::start_camera(
            core,
            Attachments {
                quantum,
                gpio: vec![],
                watch_pcs: browser.watch_pcs,
                card: browser.card,
                scene: browser.scene_input,
                preview: browser.preview,
                controls: browser.controls,
                contact_driver: browser.contact_driver,
                storage,
                identity,
                scanout,
                initial_positions: browser.initial_positions,
                boot_inputs: browser.boot_inputs,
            },
        )?;
        session.construction = Some(construction);
        Ok(session)
    }

    pub fn create_camera(
        config: &[u8],
        dat: &[u8],
        policy: fujimod_emulator_cpu::ExecutionPolicy,
    ) -> Result<Self, String> {
        let construction = super::snapshot::Construction::camera(config, dat);
        let identity: DefinitionEnvelope =
            serde_json::from_slice(config).map_err(|e| e.to_string())?;
        let mut definition = serde_json::to_value(identity.graph).map_err(|e| e.to_string())?;
        let c: Config = serde_json::from_slice(config).map_err(|e| e.to_string())?;
        let mut g = c.graph;
        if c.schema != 1
            || g.schema != 1
            || c.firmware_id != g.firmware_id
            || c.boot_source != g.boot_source
            || g.quantum == 0
        {
            return Err("session graph identity, boot source or schema mismatch".into());
        }
        if let Some(clock) = c.clock_seconds2000 {
            let mut devices = definition["devices"].clone();
            fujimod_emulator_boot::apply_clock(&mut devices, clock)?;
            g.devices = Plan::parse(&serde_json::to_vec(&devices).map_err(|e| e.to_string())?)?;
            definition["devices"] = devices;
        }
        if let Some(preview) = &g.preview {
            if preview.tick_hz == 0 || preview.period_ticks == 0 {
                return Err("preview clock requires positive frequency and period".into());
            }
            preview.cadence.validate()?;
        }
        let images = decode(&g.firmware, dat)?;
        g.execution.validate()?;
        g.devices.validate_reset_outputs(
            &g.execution
                .processors
                .iter()
                .map(|p| p.released)
                .collect::<Vec<_>>(),
        )?;
        g.devices.validate_memory_outputs(
            &g.execution
                .decoders
                .iter()
                .map(|d| MemoryChoices {
                    initial: d.initial,
                    choices: d.choices.len(),
                })
                .collect::<Vec<_>>(),
        )?;
        let (scanout, nands) = device_roles(&g.devices)?;
        let assets = g
            .assets
            .into_iter()
            .map(|(name, a)| Ok((name, a.build(dat)?)))
            .collect::<Result<_, String>>()?;
        fujimod_emulator_exr2_board::register_board_builder();
        fujimod_emulator_models::register();
        let resources = g.scene.build(&assets)?;
        let storage = fujimod_emulator_session::durable::Spec {
            cards: g
                .card
                .as_ref()
                .map(|c| vec![c.node.clone()])
                .unwrap_or_default(),
            nands,
        };
        let identity = camera_identity(&g.firmware_id, &g.firmware.sha256, &storage)?;
        let built = g.devices.build(resources, 0, &mut RefuseAdapters)?;
        let bus = Bus::from_graph(Mode::Strict, built.graph)?;
        let mut board = g.execution.build(
            factory::Options::new(factory::Engine::Unicorn),
            bus,
            None,
        )?;
        board.set_execution_policy(policy)?;
        for irq in built.core_interrupts {
            board.configure_core_interrupts(irq.core, irq.device, Box::new(irq.inputs))?;
        }
        for image in images {
            board.write_physical(image.base, &image.bytes)?;
        }
        for c in g.coprocessors {
            let cpu = board.cpu(c.core)?;
            let value = match c.value {
                Some(value) => value,
                None => cpu.coprocessor(c.register, None)?,
            };
            cpu.coprocessor(c.register, Some(value | c.set_bits))?;
        }
        let core = fujimod_emulator_session::Session {
            definition,
            controls: None,
            contact_driver: None,
            board,
            nodes: built.node_ids,
            resources: built.resources,
            contacts: built.contacts,
            recording_frames: Default::default(),
        };
        let mut session = Self::start_camera(
            core,
            Attachments {
                quantum: g.quantum,
                gpio: g.gpio,
                watch_pcs: g.watch_pcs,
                card: g.card,
                scene: g.scene_input,
                preview: g.preview,
                controls: g.controls,
                contact_driver: g.contact_driver,
                storage,
                identity,
                scanout,
                initial_positions: c.initial_positions,
                boot_inputs: c.boot_inputs,
            },
        )?;
        session.construction = Some(construction);
        Ok(session)
    }

    /// Apply the browser attachments to a booted core and start its checkpoint.
    fn start_camera(
        mut core: fujimod_emulator_session::Session,
        a: Attachments,
    ) -> Result<Self, String> {
        core.board.watch_pcs(&a.watch_pcs)?;
        for p in a.gpio {
            core.gpio(&p.node, p.port, p.pin, p.high)?;
        }
        if let Some(spec) = a.controls {
            core.configure_controls(spec)?;
        }
        if let Some(spec) = a.contact_driver {
            core.configure_contact_driver(spec)?;
        }
        if let Some(preview) = &a.preview {
            core.frame_state(&preview.node)?;
        }
        if let Some(card) = &a.card {
            core.card_state(&card.node)?;
        }
        if let Some(scene) = &a.scene {
            core.node(&scene.node)?;
            if scene.sensor.is_some()
                && !scene
                    .white_luminance
                    .is_some_and(|v| v.is_finite() && v > 0.)
            {
                return Err(
                    "sensor scene requires declared positive finite white luminance in cd/m²"
                        .into(),
                );
            }
            let images = core.resources.images()?;
            if !images.has_image(&scene.images)
                || scene
                    .sensor
                    .as_ref()
                    .is_some_and(|s| !images.has_sensor(s))
            {
                return Err("session scene input is not in the declared graph".into());
            }
        }
        let mut session = Self {
            construction: None,
            capture_order: std::cell::Cell::new(1),
            core,
            reset_image: None,
            powered_on: true,
            generation: 0,
            quantum: a.quantum,
            input: None,
            frames: Default::default(),
            history: None,
            failed: false,
            owner: super::session::next_owner(),
            camera: Some(Metadata {
                card: a.card,
                storage: a.storage,
                identity: a.identity,
                scene: a.scene,
                scanout: a.scanout,
                preview: a.preview,
            }),
        };
        session.reset_image = Some(Arc::new(session.core.checkpoint(None)?));
        for (id, position) in a.initial_positions {
            session.core.set_position(&id, &position)?;
        }
        session.boot_holds(&a.boot_inputs)?;
        Ok(session)
    }
}
