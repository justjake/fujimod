//! Standalone boot from catalog facts and explicitly selected user files.
//!
//! Preparation runs before execution. Host readiness checks only observe RAM;
//! first-start acceptance drives the ordinary ADC input on the guest timeline.
#[cfg(all(feature = "xp5-startup", not(target_arch = "wasm32")))]
pub mod application;
pub mod firmware_file;
mod first_start;
pub mod host;
mod journal_settings;
#[cfg(all(feature = "xp5-startup", not(target_arch = "wasm32")))]
pub mod media;
#[cfg(all(feature = "xp5-startup", not(target_arch = "wasm32")))]
pub mod mod_loader;
#[cfg(all(feature = "xp5-startup", not(target_arch = "wasm32")))]
pub mod output;
#[cfg(all(feature = "xp5-startup", not(target_arch = "wasm32")))]
pub mod patch;
#[cfg(all(feature = "xp5-startup", not(target_arch = "wasm32")))]
#[cfg(all(feature = "xp5-startup", not(target_arch = "wasm32")))]
mod png_writer;
use fujimod_emulator_board_graph as graph;
mod exr2;
mod x70;
#[cfg(all(feature = "xp5-startup", not(target_arch = "wasm32")))]
pub mod xp5;
pub mod xpp;

pub use firmware_file::FirmwareFile;
use fujimod_catalog::reference::PartModel;
use fujimod_catalog_assets::Content;
use fujimod_catalog_configuration::{
    Exr2RunInputs, RunExecution, RunInputs, RunPurpose, RunScene, RunStart,
};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator::board::{
    bus::{Bus, Mode},
    factory,
};
use fujimod_emulator_cpu::arm::Coprocessor;
use fujimod_emulator_exr2_plan::{MemoryChoices, Plan, RefuseAdapters};
use fujimod_emulator_preparation as preparation;
use fujimod_emulator_session::Session;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

/// Every binary input location is supplied by the embedding program.
#[derive(Clone, Debug)]
pub struct Options {
    pub update: FirmwareFile,
    pub boot_rom: FirmwareFile,
    /// Committed NAND image, or a main-area image: exactly the geometry's main bytes,
    /// optionally followed by a managed-spare trailer (`FJNANDS1`, a big-endian count,
    /// then big-endian page row and 16-byte tag entries) for the spare areas it declares.
    pub nand: Option<PathBuf>,
    /// Sixteen-byte checksum-protected MCU persistent settings packet.
    pub settings: Option<PathBuf>,
    /// Real-time clock in seconds since 2000-01-01, or
    /// `DEFAULT_CLOCK_SECONDS_2000`.
    pub clock_seconds_2000: Option<u32>,
    /// Preferred BCP 47 language tags, in caller order.
    pub preferred_languages: Vec<String>,
    /// Directory of the camera's zstd-compressed NAND sections
    /// (`nand-<section>.bin.zst`), for a run whose settings name a camera.
    pub camera: Option<PathBuf>,
    /// One virtual access point for the MT5931 endpoint, as its serialized
    /// device option. Absence leaves the endpoint without an RF peer.
    pub access_point: Option<Value>,
    /// Explicit 20-byte MT5931 capability fixture for a run that exercises
    /// stock Wi-Fi initialization. Absence refuses the capability query.
    pub capability_reply: Option<[u8; 20]>,
    /// Guest ticks the modeled SD card host charges for each data transfer.
    /// Absence keeps the run's catalog value.
    pub sd_card_transfer_ticks: Option<u64>,
    /// FAT geometry for the SD card, in place of the run's catalog layout.
    pub card_layout: Option<CardLayout>,
    /// The modeled SD host's initialization fault, as the catalog device
    /// model names it, in place of none.
    pub card_initialization_fault: Option<serde_json::Value>,
}

/// A declared FAT card geometry for a stock SD card.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardLayout {
    /// Sectors on the blank card.
    pub sectors: u32,
    /// One to eleven ASCII bytes.
    pub label: String,
    /// First sector of the FAT partition.
    pub partition_start: u32,
    /// Sectors per FAT cluster: a power of two from 1 to 128.
    pub sectors_per_cluster: u8,
}

impl CardLayout {
    /// Check the geometry a FAT formatter accepts.
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=11).contains(&self.label.len()) || !self.label.is_ascii() {
            return Err("card layout label must be one to eleven ASCII bytes".into());
        }
        if self.sectors == 0 {
            return Err("card layout needs at least one sector".into());
        }
        if self.partition_start == 0 || self.partition_start >= self.sectors {
            return Err("card layout partition start must lie inside the card".into());
        }
        if !matches!(self.sectors_per_cluster, 1 | 2 | 4 | 8 | 16 | 32 | 64 | 128) {
            return Err("card layout cluster size must be 1, 2, 4, 8, 16, 32, 64 or 128".into());
        }
        Ok(())
    }
}

/// Largest compressed camera NAND section accepted from the caller.
const CAMERA_SECTION_MAX_BYTES: u64 = 16 << 20;

#[derive(Clone)]
struct Rom {
    content: Content,
    address: u64,
}
#[derive(Clone)]
struct Generated {
    name: String,
    pattern: Vec<u8>,
    size: usize,
}
#[derive(Clone)]
struct Reset {
    core: usize,
    register: Coprocessor,
    set_bits: u64,
}
#[derive(Clone)]
struct Mcu {
    node: String,
    route: usize,
    first_register: u8,
}
#[derive(Clone)]
struct Observation {
    ui_mode: u64,
    system_mode: u64,
    menu_active: u64,
    pending_mode: u64,
    menu_node: u64,
    rec_mode: u32,
    setup_mode: u32,
    scanout: String,
    readiness: Readiness,
}
#[derive(Clone)]
struct Readiness {
    category_one: u64,
    category_two: u64,
    input_pending: Option<u64>,
    override_address: u64,
    excluded_mask: u64,
    operation_alert_timer: Option<u64>,
}
#[derive(Clone)]
struct FirstStart {
    /// Debug key name pressed to accept a first-start page.
    key: String,
    located: fujimod_catalog_wiring::Located,
    hold_ticks: u64,
    settle_ticks: u64,
    max_presses: usize,
}
/// A fully resolved run assembled before loading caller files or creating CPUs.
pub struct Prepared {
    resolved: ResolvedConfiguration,
    execution: factory::Graph,
    execution_value: Value,
    plan: Plan,
    devices_value: Value,
    scene: Value,
    execution_policy: RunExecution,
    quantum: usize,
    update: Content,
    flash_offset: usize,
    boot_rom: Rom,
    generated_assets: Vec<Generated>,
    reset_coprocessors: Vec<Reset>,
    mcu: Option<Mcu>,
    observation: Observation,
    first_start: Option<FirstStart>,
}

fn content(size: u64, sha256: &str) -> Content {
    Content {
        size,
        sha256: sha256.to_owned(),
    }
}

fn first_start(
    resolved: &ResolvedConfiguration,
    inputs: Option<&Exr2RunInputs>,
) -> Result<Option<FirstStart>, String> {
    let Some(inputs) = inputs else {
        return Ok(None);
    };
    let Some(policy) = &inputs.first_start else {
        return Ok(None);
    };
    let key_id = &policy.key_id;
    let located = fujimod_catalog_wiring::locate(&resolved.reference)?;
    fujimod_emulator_controls::check_key(&located, &resolved.reference, key_id)
        .map_err(|error| format!("first-start key {key_id}: {error}"))?;
    Ok(Some(FirstStart {
        key: key_id.to_owned(),
        located,
        hold_ticks: policy.hold_ticks,
        settle_ticks: policy.settle_ticks,
        max_presses: usize::from(policy.max_presses),
    }))
}

fn prepared(resolved: ResolvedConfiguration) -> Result<Prepared, String> {
    let configuration = &resolved.configuration;
    configuration.validate(&configuration.id)?;
    fujimod_catalog_resolution::validate_selection(configuration, &resolved.reference)?;
    match configuration.purpose {
        RunPurpose::RomBoot => (),
        RunPurpose::ServiceSession => {
            return Err("native X70 boot does not support service-session runs".into());
        }
        RunPurpose::Rehearsal => {
            return Err("native X70 boot does not support rehearsal runs".into());
        }
        RunPurpose::BootAccess => {
            return Err("native X70 boot does not support boot-access runs".into());
        }
        RunPurpose::Shutdown => {
            return Err("native X70 boot does not support shutdown runs".into());
        }
    }
    match configuration.start {
        RunStart::Rom => (),
        RunStart::Loader => {
            return Err("native X70 boot does not support loader-start configurations".into());
        }
        RunStart::SdCard => {
            return Err("native X70 boot does not support SD-card-start configurations".into());
        }
        RunStart::Setup => {
            return Err("native X70 boot does not support setup-start configurations".into());
        }
    }
    let inputs = match configuration.inputs.as_ref() {
        Some(RunInputs::Exr2(inputs)) => Some(inputs),
        Some(RunInputs::Xp4(_)) => {
            return Err("native X70 boot does not support XP4 startup inputs".into());
        }
        Some(RunInputs::Xp5(_)) => {
            return Err("native X70 boot does not support XP5 startup inputs".into());
        }
        None => None,
    };
    // A body profile that exports its native construction carries the reset
    // values as catalog data, so the run may omit startup inputs.
    let construction = exr2::available(&resolved);
    if inputs.is_none() && !construction {
        return Err("native EXR II boot requires declared startup inputs".into());
    }
    let scene_input = match configuration.scene.as_ref() {
        Some(RunScene::Exr2(scene)) => scene,
        Some(RunScene::Xp4(_)) => {
            return Err("native X70 boot does not support XP4 scene inputs".into());
        }
        Some(RunScene::Xp5(_)) => {
            return Err("native X70 boot does not support XP5 scene inputs".into());
        }
        None => return Err("native X70 boot requires a declared generated scene".into()),
    };
    let execution_policy = configuration
        .execution
        .ok_or("native X70 boot requires a declared execution policy")?;
    let firmware = &resolved.reference.firmware;
    let update_identity = firmware
        .update
        .as_ref()
        .ok_or("selected firmware has no update content identity")?;
    let rom_identity = firmware
        .boot_rom
        .as_ref()
        .ok_or("selected firmware has no boot-ROM identity")?;
    let installation = firmware
        .installation
        .as_ref()
        .ok_or("selected firmware has no installation layout")?;
    let observation: fujimod_catalog_remote::FirmwareObservation = serde_json::from_value(
        firmware
            .generated_facts
            .as_ref()
            .and_then(|facts| facts.observation.as_ref())
            .cloned()
            .ok_or("selected firmware has no observation facts")?,
    )
    .map_err(|error| format!("firmware observation facts: {error}"))?;
    let quantum = execution_policy.quantum;
    let board_ticks_per_second = configuration
        .inputs
        .as_ref()
        .and_then(|inputs| match inputs {
            RunInputs::Xp5(inputs) => Some(inputs.ticks_per_second),
            RunInputs::Exr2(_) | RunInputs::Xp4(_) => None,
        });
    let dram_address_space_limit_bytes = resolved.dram_address_space_limit_bytes()?;
    let execution = graph::assemble(
        &resolved.reference,
        dram_address_space_limit_bytes,
        execution_policy.instructions_per_tick,
        board_ticks_per_second,
    )?;
    let execution_value = graph::value(
        &resolved.reference,
        &execution,
        execution_policy.instructions_per_tick,
    );
    let (plan, devices_value, scene) = if construction {
        exr2::assemble(&resolved, scene_input)?
    } else {
        x70::assemble(
            &resolved.reference,
            configuration,
            dram_address_space_limit_bytes,
            inputs.ok_or("native X70 boot requires declared startup inputs")?,
            scene_input,
        )?
    };
    plan.validate_reset_outputs(
        &execution
            .processors
            .iter()
            .map(|processor| processor.released)
            .collect::<Vec<_>>(),
    )?;
    plan.validate_memory_outputs(
        &execution
            .decoders
            .iter()
            .map(|decoder| MemoryChoices {
                initial: decoder.initial,
                choices: decoder.choices.len(),
            })
            .collect::<Vec<_>>(),
    )?;
    let generated_assets = scene_input
        .generated_assets
        .iter()
        .map(|asset| {
            Ok(Generated {
                name: asset.id.clone(),
                pattern: asset.pattern.clone(),
                size: asset.size,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut core_indexes = BTreeMap::new();
    for (index, processor) in resolved.reference.soc.processors.iter().enumerate() {
        core_indexes.insert(processor.id.as_str(), index);
    }
    let reset_coprocessors = resolved
        .reference
        .soc
        .reset
        .coprocessor_writes
        .iter()
        .map(|write| {
            Ok(Reset {
                core: *core_indexes
                    .get(write.core.as_str())
                    .ok_or_else(|| format!("reset write targets absent core {}", write.core))?,
                register: Coprocessor {
                    cp: u32::from(write.cp),
                    opc1: u32::from(write.opc1),
                    crn: u32::from(write.crn),
                    crm: u32::from(write.crm),
                    opc2: u32::from(write.opc2),
                    secure: u32::from(write.secure),
                    wide: write.wide,
                },
                set_bits: write.set_bits,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mcu = match resolved.reference.body.microcontroller.as_ref() {
        Some(mcu_attachment) => {
            let mcu_part = resolved
                .reference
                .parts
                .iter()
                .find(|part| part.id == mcu_attachment.part_id)
                .ok_or("MCU part is unresolved")?;
            let PartModel::Microcontroller {
                packet_registers, ..
            } = &mcu_part.model
            else {
                return Err("MCU attachment is not a microcontroller part".into());
            };
            if construction {
                return Err("a body construction does not yet map an attached MCU".into());
            }
            Some(Mcu {
                node: x70::node_for_block(&mcu_attachment.controller_id)?,
                route: usize::from(mcu_attachment.channel),
                first_register: u8::try_from(packet_registers.first)
                    .map_err(|_| "MCU packet register exceeds one byte")?,
            })
        }
        None => resolved
            .configuration
            .preset
            .as_ref()
            .and_then(|preset| preset.mcu.as_ref())
            .map(|mcu| {
                Ok::<Mcu, String>(Mcu {
                    node: mcu.node.clone(),
                    route: usize::try_from(mcu.route)
                        .map_err(|_| "MCU packet route exceeds host address space")?,
                    first_register: u8::try_from(mcu.first_register)
                        .map_err(|_| "MCU packet register exceeds one byte")?,
                })
            })
            .transpose()?,
    };
    let readiness = &observation.readiness;
    let observation = Observation {
        ui_mode: observation.ui_mode,
        system_mode: observation.system_mode,
        menu_active: observation.menu_active,
        pending_mode: observation.pending_mode,
        menu_node: observation.menu_node,
        rec_mode: observation.rec_mode,
        setup_mode: observation.setup_mode,
        scanout: "scanout".into(),
        readiness: Readiness {
            category_one: readiness.category_one,
            category_two: readiness.category_two,
            input_pending: Some(readiness.input_pending),
            override_address: readiness.override_address,
            excluded_mask: u64::from(readiness.excluded_mask),
            operation_alert_timer: Some(readiness.operation_alert_timer),
        },
    };
    let first_start = first_start(&resolved, inputs)?;
    let flash_offset = usize::try_from(installation.flash_offset)
        .map_err(|_| "firmware flash offset exceeds host address space")?;
    let update = content(update_identity.size, &update_identity.sha256);
    let boot_rom = Rom {
        content: content(rom_identity.content.size, &rom_identity.content.sha256),
        address: rom_identity.address,
    };
    Ok(Prepared {
        resolved,
        execution,
        execution_value,
        plan,
        devices_value,
        scene,
        execution_policy,
        quantum,
        update,
        flash_offset,
        boot_rom,
        generated_assets,
        reset_coprocessors,
        mcu,
        observation,
        first_start,
    })
}

/// Resolve and compile a named configuration before any input file or CPU is used.
pub fn prepare(configuration_id: &str, overlays: &[String]) -> Result<Prepared, String> {
    prepared(ResolvedConfiguration::with_overlays(
        configuration_id,
        overlays,
    )?)
}

impl Prepared {
    fn duplicate(&self) -> Result<Self, String> {
        Ok(Self {
            resolved: self.resolved.clone(),
            execution: serde_json::from_value(self.execution_value.clone())
                .map_err(|e| e.to_string())?,
            execution_value: self.execution_value.clone(),
            plan: Plan::parse(
                &serde_json::to_vec(&self.devices_value).map_err(|e| e.to_string())?,
            )?,
            devices_value: self.devices_value.clone(),
            scene: self.scene.clone(),
            execution_policy: self.execution_policy,
            quantum: self.quantum,
            update: self.update.clone(),
            flash_offset: self.flash_offset,
            boot_rom: self.boot_rom.clone(),
            generated_assets: self.generated_assets.clone(),
            reset_coprocessors: self.reset_coprocessors.clone(),
            mcu: self.mcu.clone(),
            observation: self.observation.clone(),
            first_start: self.first_start.clone(),
        })
    }
    /// Produce the stable JSON projection used by the native Python adapter.
    pub fn to_json(&self) -> Value {
        let readiness = &self.observation.readiness;
        json!({
            "schema": 1,
            "configuration_id": self.resolved.configuration.id,
            "body_id": self.resolved.configuration.body_id,
            "firmware_id": self.resolved.configuration.firmware_id,
            "overlays": self.resolved.configuration.overlays,
            "resolved": self.resolved,
            "quantum": self.quantum,
            "execution": self.execution_value,
            "devices": self.devices_value,
            "scene": self.scene,
            "inputs": {
                "update": self.update,
                "boot_rom": {"content":self.boot_rom.content,"address":self.boot_rom.address},
                "flash_offset": self.flash_offset,
                "mcu": self.mcu.as_ref().map(|mcu| json!({
                    "node":mcu.node,
                    "route":mcu.route,
                    "first_register":mcu.first_register,
                })),
                "reset_coprocessors": self.reset_coprocessors.iter().map(|reset| json!({
                    "core":reset.core,
                    "register": {
                        "cp": reset.register.cp,
                        "wide": reset.register.wide,
                        "secure": reset.register.secure,
                        "crn": reset.register.crn,
                        "crm": reset.register.crm,
                        "opc1": reset.register.opc1,
                        "opc2": reset.register.opc2,
                    },
                    "set_bits":reset.set_bits
                })).collect::<Vec<_>>(),
                "observation": {
                    "ui_mode":self.observation.ui_mode,
                    "system_mode":self.observation.system_mode,
                    "menu_active":self.observation.menu_active,
                    "pending_mode":self.observation.pending_mode,
                    "menu_node":self.observation.menu_node,
                    "rec_mode":self.observation.rec_mode,
                    "setup_mode":self.observation.setup_mode,
                    "scanout":self.observation.scanout,
                    "readiness": json!({
                        "category_one":readiness.category_one,
                        "category_two":readiness.category_two,
                        "input_pending":readiness.input_pending,
                        "override_address":readiness.override_address,
                        "excluded_mask":readiness.excluded_mask,
                        "operation_alert_timer":readiness.operation_alert_timer,
                    }),
                },
                "first_start": self.first_start.as_ref().map(|start| json!({
                    "key_id":start.key,
                    "hold_ticks":start.hold_ticks,"settle_ticks":start.settle_ticks,
                    "max_presses":start.max_presses,
                })),
                "generated_assets": self.generated_assets.iter().map(|asset| json!({
                    "name":asset.name,"pattern":asset.pattern,"size":asset.size,
                })).collect::<Vec<_>>(),
            }
        })
    }
}

/// Read a bounded regular file, rejecting size changes during preparation.
fn read(path: &Path, max_bytes: u64) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return Err(format!(
            "{} must be a regular file of at most {max_bytes} bytes",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 != metadata.len() {
        return Err(format!("{} changed during preparation", path.display()));
    }
    Ok(bytes)
}
/// Read an update file, decoding zstd in memory by its magic number.
fn checked_update(input: FirmwareFile, content: &Content) -> Result<Vec<u8>, String> {
    let bytes = input.read(firmware_file::MAX_BYTES)?;
    content
        .check(&bytes)
        .map_err(|error| format!("update: {error}"))?;
    Ok(bytes)
}

/// Read a boot ROM capture, decoding zstd and keeping the ROM's first bytes.
fn checked_boot_rom(input: FirmwareFile, content: &Content) -> Result<Vec<u8>, String> {
    let bytes = input.read(firmware_file::MAX_BYTES)?;
    if bytes.len() < content.size as usize {
        return Err(format!(
            "boot ROM capture is {} bytes, shorter than the {} the catalog names",
            bytes.len(),
            content.size
        ));
    }
    let bytes = bytes[..content.size as usize].to_vec();
    content
        .check(&bytes)
        .map_err(|error| format!("boot ROM: {error}"))?;
    Ok(bytes)
}
fn packet(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() != 16
        || bytes[14] != 0xaa
        || bytes.iter().fold(0u8, |s, b| s.wrapping_add(*b)) != 0
    {
        return Err("settings require a sixteen-byte MCU packet with marker 0xaa and an eight-bit zero checksum".into());
    }
    Ok(())
}

/// Apply caller local wall time to the single RTC declared by a device plan.
pub fn apply_clock(devices: &mut Value, clock_seconds: u32) -> Result<(), String> {
    let mut count = 0;
    set_clock_recursive(devices, clock_seconds, &mut count)?;
    if count != 1 {
        return Err("selected configuration has no single real-time clock".into());
    }
    Ok(())
}

fn set_clock_recursive(
    value: &mut Value,
    clock_seconds: u32,
    count: &mut usize,
) -> Result<(), String> {
    match value {
        Value::Object(map) => {
            // Check if this object has a "clock" key with "initial_seconds"
            if let Some(clock_obj) = map.get_mut("clock")
                && let Value::Object(clock_map) = clock_obj
                && clock_map.contains_key("initial_seconds")
            {
                clock_map.insert("initial_seconds".to_string(), json!(clock_seconds));
                *count += 1;
            }
            // Recurse into all values
            for v in map.values_mut() {
                set_clock_recursive(v, clock_seconds, count)?;
            }
            Ok(())
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                set_clock_recursive(v, clock_seconds, count)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Magic that opens a managed-spare trailer after a main-area NAND image.
const MANAGED_SPARE_MAGIC: &[u8; 8] = b"FJNANDS1";
/// Bytes per trailer entry: a big-endian physical page row, then its 16-byte tag.
const MANAGED_SPARE_ENTRY_BYTES: usize = 4 + 16;

/// True when a NAND input carries a managed-spare trailer after its main area.
fn managed_spare_trailer(bytes: &[u8], main_bytes: usize) -> bool {
    bytes.len() > main_bytes && bytes[main_bytes..].starts_with(MANAGED_SPARE_MAGIC)
}

/// Split a main-area NAND input into its image and its managed spare rows.
/// The trailer is `FJNANDS1`, a big-endian entry count, then the entries; each
/// row must be unique and inside the main area's page grid.
fn split_managed_spares(
    bytes: &[u8],
    main_bytes: usize,
) -> Result<(&[u8], Vec<(u32, [u8; 16])>), String> {
    if bytes.len() == main_bytes {
        return Ok((bytes, Vec::new()));
    }
    let header = main_bytes + MANAGED_SPARE_MAGIC.len();
    let count_bytes: [u8; 4] = bytes
        .get(header..header + 4)
        .and_then(|b| b.try_into().ok())
        .ok_or("NAND managed spare trailer has no count")?;
    let count = u32::from_be_bytes(count_bytes) as usize;
    let entries = &bytes[header + 4..];
    if entries.len() != count * MANAGED_SPARE_ENTRY_BYTES {
        return Err("NAND managed spare trailer length differs from its count".into());
    }
    let mut rows = std::collections::BTreeSet::new();
    let mut spares = Vec::with_capacity(count);
    for entry in entries.as_chunks::<MANAGED_SPARE_ENTRY_BYTES>().0 {
        let row = u32::from_be_bytes(entry[..4].try_into().map_err(|_| "NAND spare row")?);
        let tag: [u8; 16] = entry[4..].try_into().map_err(|_| "NAND spare tag")?;
        if !rows.insert(row) {
            return Err("NAND managed spare rows repeat".into());
        }
        spares.push((row, tag));
    }
    Ok((&bytes[..main_bytes], spares))
}

fn settings_packet(plan: &mut Value, mcu: Option<&Mcu>, bytes: &[u8]) -> Result<(), String> {
    packet(bytes)?;
    let mcu = mcu.ok_or("this body has no MCU packet input")?;
    let nodes = plan["nodes"]
        .as_array_mut()
        .ok_or("runtime nodes are absent")?;
    let node = nodes
        .iter_mut()
        .find(|n| n["name"] == mcu.node)
        .ok_or("MCU serial node is absent")?;
    let registers = node["model"]["routes"][mcu.route]["endpoint"]["options"]["registers"]
        .as_array_mut()
        .ok_or("MCU register endpoint is absent")?;
    for (offset, byte) in bytes.iter().enumerate() {
        let register = u64::from(mcu.first_register) + offset as u64;
        let row = registers
            .iter_mut()
            .find(|r| r[0].as_u64() == Some(register))
            .ok_or("MCU packet register is absent")?;
        row[1] = json!(byte);
    }
    Ok(())
}

/// Caller-supplied bytes for one prepared run. Every buffer is checked against
/// its prepared identity or declared bound before any CPU is constructed.
#[derive(Clone)]
pub struct InputBytes {
    /// Complete update container, verified against the prepared update identity.
    pub update: Vec<u8>,
    /// Boot ROM image, verified against the prepared boot-ROM identity.
    pub boot_rom: Vec<u8>,
    /// Committed NAND image, or a main-area image: exactly the geometry's main bytes,
    /// optionally followed by a managed-spare trailer (`FJNANDS1`, a big-endian count,
    /// then big-endian page row and 16-byte tag entries) for the spare areas it declares.
    pub nand: Option<Vec<u8>>,
    /// Sixteen-byte checksum-protected MCU persistent settings packet.
    pub settings: Option<Vec<u8>>,
    /// Real-time clock in seconds since 2000-01-01, or
    /// `DEFAULT_CLOCK_SECONDS_2000`.
    pub clock_seconds_2000: Option<u32>,
    /// Preferred BCP 47 language tags, in caller order.
    pub preferred_languages: Vec<String>,
    /// The camera's zstd-compressed NAND sections by section name.
    pub camera: BTreeMap<String, Vec<u8>>,
}

/// Largest NAND input accepted from the caller: a 256 MiB image.
const NAND_INPUT_MAX_BYTES: usize = 256 << 20;

fn preparation_difference(expected: &Value, provided: &Value, path: &str) -> String {
    match (expected, provided) {
        (Value::Object(expected), Value::Object(provided)) => {
            for (key, value) in expected {
                let path = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                match provided.get(key) {
                    Some(other) if value != other => {
                        return preparation_difference(value, other, &path);
                    }
                    None => return path,
                    _ => {}
                }
            }
            if let Some(key) = provided.keys().find(|key| !expected.contains_key(*key)) {
                return format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
            }
        }
        (Value::Array(expected), Value::Array(provided)) => {
            for (index, (value, other)) in expected.iter().zip(provided).enumerate() {
                if value != other {
                    return preparation_difference(value, other, &format!("{path}/{index}"));
                }
            }
            return format!("{path}/length ({} != {})", expected.len(), provided.len());
        }
        (Value::Number(expected), Value::Number(provided)) => {
            return format!("{path} ({expected} != {provided})");
        }
        _ => {}
    }
    path.into()
}

impl Prepared {
    /// Restore a prepared run from its `to_json` document. The embedded identity
    /// must equal the bundled catalog's resolution, and preparing it again must
    /// reproduce the document exactly, so a shipped document cannot describe a
    /// different run than this build would prepare.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let resolved = value
            .get("resolved")
            .cloned()
            .ok_or("prepared document has no resolved configuration")?;
        let resolved: ResolvedConfiguration = serde_json::from_value(resolved)
            .map_err(|error| format!("prepared document resolved configuration: {error}"))?;
        let expected = ResolvedConfiguration::with_overlays(
            &resolved.configuration.id,
            &resolved.configuration.overlays,
        )?;
        if expected != resolved {
            return Err("prepared document differs from the bundled catalog".into());
        }
        let prepared = prepared(resolved)?;
        let expected = prepared.to_json();
        if expected != *value {
            return Err(format!(
                "prepared document differs from its preparation at {}",
                preparation_difference(&expected, value, "")
            ));
        }
        Ok(prepared)
    }
}

/// Select whether undeclared accesses stop or are recorded for model discovery.
pub fn discovery_mode(discovery: bool) -> Mode {
    if discovery {
        Mode::Discovery
    } else {
        Mode::Strict
    }
}

/// Files and startup settings for the selected processor generation.
pub enum StartupInputs {
    Exr2 {
        options: Options,
        mode: Mode,
    },
    #[cfg(all(feature = "xp5-startup", not(target_arch = "wasm32")))]
    Xp5(xp5::Startup),
    #[cfg(all(feature = "xp5-startup", not(target_arch = "wasm32")))]
    Application(application::Startup),
}

/// One unexecuted camera session with the startup facts needed by its host.
pub struct Camera {
    pub session: Session,
    /// Maximum byte count a host may request from this camera's modeled RAM.
    pub read_limit: usize,
    /// Virtual guest ticks in one second.
    pub ticks_per_second: u64,
    /// Instructions per scheduler turn.
    pub quantum: usize,
    #[cfg(all(feature = "xp5-startup", not(target_arch = "wasm32")))]
    xp5: Option<xp5::RunFacts>,
}

impl Camera {
    /// Take the XP5 physical input handle when transferring a camera to a binding.
    #[cfg(all(feature = "xp5-startup", not(target_arch = "wasm32")))]
    pub fn take_xp5_inputs(
        &mut self,
    ) -> Result<fujimod_emulator_xp5_board::session::Inputs, String> {
        self.xp5
            .as_mut()
            .and_then(|facts| facts.inputs.take())
            .ok_or("camera has no XP5 physical input handle".into())
    }
}

fn graph_read_limit(session: &Session) -> Result<usize, String> {
    let execution: factory::Graph = serde_json::from_value(session.definition["execution"].clone())
        .map_err(|error| format!("session execution graph: {error}"))?;
    execution
        .ram
        .iter()
        .map(|region| region.size)
        .chain(
            execution
                .processors
                .iter()
                .filter_map(|processor| processor.rprofile.clone())
                .flat_map(|profile| [profile.tcm.atcm.bytes, profile.tcm.btcm.bytes]),
        )
        .try_fold(0usize, |sum, bytes| {
            sum.checked_add(bytes)
                .ok_or_else(|| "declared RAM extent overflows".to_owned())
        })
}

fn session_quantum(session: &Session) -> Result<usize, String> {
    session.definition["quantum"]
        .as_u64()
        .and_then(|quantum| usize::try_from(quantum).ok())
        .filter(|quantum| *quantum > 0)
        .ok_or_else(|| "configuration declares no positive scheduler quantum".into())
}

/// The files a host has for a camera. [`open_request`] uses the ones the
/// configuration's processor needs and refuses a missing one, so a host never
/// selects startup inputs by processor generation.
#[cfg(all(feature = "xp5-startup", not(target_arch = "wasm32")))]
#[derive(Clone, Default)]
pub struct Request {
    pub update: FirmwareFile,
    pub boot_rom: Option<FirmwareFile>,
    pub nand: Option<std::path::PathBuf>,
    pub settings: Option<std::path::PathBuf>,
    /// Directory of the camera's NAND sections, for a run whose settings name a camera.
    pub camera: Option<std::path::PathBuf>,
    /// The application startup plan of an X-Processor Pro body.
    pub application_plan: Option<std::path::PathBuf>,
    pub card_files: Vec<(String, std::path::PathBuf)>,
    pub discovery: bool,
    /// Guest ticks the modeled SD card host charges for each data transfer.
    /// Absence keeps the run's catalog value.
    pub sd_card_transfer_ticks: Option<u64>,
    /// FAT geometry for the SD card, in place of the run's catalog layout.
    pub card_layout: Option<CardLayout>,
    /// The modeled SD host's initialization fault, in place of none.
    pub card_initialization_fault: Option<serde_json::Value>,
}

/// Open any configuration `open` supports from a host's files.
#[cfg(all(feature = "xp5-startup", not(target_arch = "wasm32")))]
pub fn open_request(resolved: ResolvedConfiguration, request: Request) -> Result<Camera, String> {
    // EXR II cards are formatted after the board opens; other starts build
    // their loader card from the files.
    if (request.sd_card_transfer_ticks.is_some() || request.card_layout.is_some())
        && !is_exr2_run(&resolved)
    {
        return Err("SD card options apply only to EXR II runs".into());
    }
    let mut card_after_open = Vec::new();
    // A run whose body profile exports its native construction carries the
    // reset values as catalog data, so it may declare no startup inputs.
    let inputs = if is_exr2_run(&resolved) {
        StartupInputs::Exr2 {
            options: Options {
                update: request.update,
                boot_rom: request.boot_rom.ok_or("this camera needs its boot ROM")?,
                nand: request.nand,
                settings: request.settings,
                clock_seconds_2000: None,
                preferred_languages: Vec::new(),
                camera: request.camera,
                access_point: None,
                capability_reply: None,
                sd_card_transfer_ticks: request.sd_card_transfer_ticks,
                card_layout: request.card_layout.clone(),
                card_initialization_fault: request.card_initialization_fault.clone(),
            },
            mode: {
                card_after_open = request.card_files;
                discovery_mode(request.discovery)
            },
        }
    } else {
        match resolved.configuration.inputs.as_ref() {
            Some(RunInputs::Xp5(_)) => StartupInputs::Xp5(xp5::Startup {
                update: request.update,
                manifest: None,
                boot_rom: request.boot_rom,
                card_image: None,
                card_files: request.card_files,
                second_card_image: None,
                second_card_files: Vec::new(),
                scene: None,
                output: None,
                mode: discovery_mode(request.discovery),
                quantum: None,
                camera_settings: Default::default(),
                flash_seeds: Vec::new(),
            }),
            _ if resolved.reference.soc.id == "xpp" => {
                StartupInputs::Application(application::Startup {
                    update: request.update,
                    plan: request
                        .application_plan
                        .ok_or("this camera needs its application startup plan")?,
                    discovery: request.discovery,
                    quantum: None,
                    card_image: None,
                    card_files: request.card_files,
                    camera_settings: Default::default(),
                })
            }
            _ => {
                return Err(format!(
                    "{} cannot be opened as a camera",
                    resolved.configuration.id
                ));
            }
        }
    };
    let mut camera = open(resolved, inputs)?;
    media::attach(&mut camera.session, None, &card_after_open)?;
    Ok(camera)
}

/// Open a supported configured board from caller-selected files.
pub fn open(resolved: ResolvedConfiguration, inputs: StartupInputs) -> Result<Camera, String> {
    let exr2_run = is_exr2_run(&resolved);
    let exr2_ticks = match resolved.configuration.inputs.as_ref() {
        Some(RunInputs::Exr2(inputs)) => Some(inputs.mcu.rtc.ticks_per_second),
        None if exr2_run => Some(
            resolved
                .configuration
                .preset
                .as_ref()
                .and_then(|preset| preset.ticks_per_second)
                .ok_or("EXR II run without startup inputs declares no board timer rate")?,
        ),
        _ => None,
    };
    match (resolved.configuration.inputs.as_ref(), inputs) {
        (_, StartupInputs::Exr2 { options, mode }) if exr2_run => {
            let session = open_exr2(resolved, options, mode)?;
            let quantum = session_quantum(&session)?;
            let read_limit = graph_read_limit(&session)?;
            Ok(Camera {
                session,
                read_limit,
                ticks_per_second: exr2_ticks.ok_or("EXR II run declares no board timer rate")?,
                quantum,
                #[cfg(all(feature = "xp5-startup", not(target_arch = "wasm32")))]
                xp5: None,
            })
        }
        #[cfg(all(feature = "xp5-startup", not(target_arch = "wasm32")))]
        (Some(RunInputs::Xp5(_)), StartupInputs::Xp5(inputs)) => {
            xp5::open_startup(&resolved, inputs)
        }
        #[cfg(all(feature = "xp5-startup", not(target_arch = "wasm32")))]
        (Some(RunInputs::Xp4(_)) | None, StartupInputs::Application(inputs))
            if matches!(resolved.reference.soc.id.as_str(), "xp4" | "xpp") =>
        {
            let started = application::open(&resolved, inputs)?;
            let quantum = started.quantum;
            let ticks_per_second = started.ticks_per_second;
            let application::Board::Shared(session) = started.board;
            let read_limit = graph_read_limit(&session)?;
            Ok(Camera {
                session: *session,
                read_limit,
                ticks_per_second,
                quantum,
                xp5: None,
            })
        }
        _ => Err(format!(
            "startup inputs do not match configuration {}",
            resolved.configuration.id
        )),
    }
}

/// Whether a configuration runs on EXR II, either from declared startup inputs
/// or from a body construction that carries its reset values.
fn is_exr2_run(resolved: &ResolvedConfiguration) -> bool {
    matches!(
        resolved.configuration.inputs.as_ref(),
        Some(RunInputs::Exr2(_))
    ) || (resolved.configuration.inputs.is_none() && exr2::available(resolved))
}

/// Optional instruction-entry trace shared by native boot workflows.
pub struct TraceOutput {
    pub path: PathBuf,
    pub limit: usize,
}

impl TraceOutput {
    pub fn begin(
        &self,
        session: &Session,
    ) -> Result<fujimod_emulator::board::observation::PcTrace, String> {
        session.board.trace_pcs(self.limit)
    }

    pub fn write(
        &self,
        trace: fujimod_emulator::board::observation::PcTrace,
    ) -> Result<(), String> {
        let entries = trace.take();
        let file = File::create(&self.path)
            .map_err(|error| format!("{}: {error}", self.path.display()))?;
        let mut writer = std::io::BufWriter::new(file);
        serde_json::to_writer(&mut writer, &entries).map_err(|error| error.to_string())?;
        std::io::Write::flush(&mut writer).map_err(|error| error.to_string())
    }
}

/// Restore a portable snapshot into a session built from the same inputs.
pub fn load_snapshot(session: &mut Session, path: &Path) -> Result<(), String> {
    let mut restore = || -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        let point = session.import_snapshot(&bytes)?;
        session.restore(&point)
    };
    restore().map_err(|error| format!("{}: {error}", path.display()))
}

/// Write a portable snapshot of the current session boundary.
pub fn save_snapshot(session: &Session, path: &Path) -> Result<(), String> {
    let save = || -> Result<(), String> {
        let bytes = session.checkpoint(None)?.export()?;
        std::fs::write(path, bytes).map_err(|error| error.to_string())
    };
    save().map_err(|error| format!("{}: {error}", path.display()))
}

/// Construct an unexecuted strict board from explicitly selected files.
/// No directory search, downloaded image, captured default or firmware patch is used.
fn open_exr2(
    resolved: ResolvedConfiguration,
    options: Options,
    mode: Mode,
) -> Result<Session, String> {
    let mut prepared = prepared(resolved)?;
    if options.access_point.is_some() || options.capability_reply.is_some() {
        prepared.set_mt5931(options.access_point, options.capability_reply)?;
    }
    if let Some(ticks) = options.sd_card_transfer_ticks {
        prepared.set_sd_card_transfer_ticks(ticks)?;
    }
    if let Some(layout) = &options.card_layout {
        prepared.set_card_layout(layout)?;
    }
    if let Some(fault) = &options.card_initialization_fault {
        prepared.set_card_initialization_fault(fault)?;
    }
    let update = checked_update(options.update, &prepared.update)?;
    let boot_rom = checked_boot_rom(options.boot_rom, &prepared.boot_rom.content)?;
    let inputs = InputBytes {
        update,
        boot_rom,
        nand: options
            .nand
            .as_deref()
            .map(|path| read(path, NAND_INPUT_MAX_BYTES as u64))
            .transpose()?,
        settings: options
            .settings
            .as_deref()
            .map(|path| read(path, 16))
            .transpose()?,
        clock_seconds_2000: options.clock_seconds_2000,
        preferred_languages: options.preferred_languages,
        camera: options
            .camera
            .as_deref()
            .map(|directory| camera_sections(&prepared, directory))
            .transpose()?
            .unwrap_or_default(),
    };
    open_prepared_mode(prepared, inputs, mode)
}

impl Prepared {
    /// Format the modeled SD card with the caller's declared FAT geometry.
    fn set_card_layout(&mut self, layout: &CardLayout) -> Result<(), String> {
        layout.validate()?;
        let nodes = self
            .devices_value
            .get_mut("nodes")
            .and_then(Value::as_array_mut)
            .ok_or("prepared devices have no node list")?;
        let sd = nodes
            .iter_mut()
            .find(|node| node.get("name").and_then(Value::as_str) == Some("sd"))
            .ok_or("run has no modeled SD card host")?;
        let card = sd
            .pointer_mut("/model/card")
            .ok_or("run has no modeled SD card")?;
        if card.get("kind").and_then(Value::as_str) != Some("formatted") {
            return Err("run SD card is not a formatted FAT card".into());
        }
        let options = card
            .get_mut("options")
            .and_then(Value::as_object_mut)
            .ok_or("run SD card has no format options")?;
        options.insert("sectors".into(), json!(layout.sectors));
        options.insert("label".into(), json!(layout.label));
        options.insert("partition_start".into(), json!(layout.partition_start));
        options.insert(
            "sectors_per_cluster".into(),
            json!(layout.sectors_per_cluster),
        );
        Ok(())
    }
    /// Charge `ticks` of guest time for every data transfer on the modeled SD
    /// host, in place of the catalog's value.
    fn set_sd_card_transfer_ticks(&mut self, ticks: u64) -> Result<(), String> {
        if ticks == 0 {
            return Err("SD card transfer ticks must be positive".into());
        }
        let nodes = self
            .devices_value
            .get_mut("nodes")
            .and_then(Value::as_array_mut)
            .ok_or("prepared devices have no node list")?;
        let sd = nodes
            .iter_mut()
            .find(|node| node.get("name").and_then(Value::as_str) == Some("sd"))
            .ok_or("run has no modeled SD card host")?;
        let options = sd
            .pointer_mut("/model/options")
            .and_then(Value::as_object_mut)
            .ok_or("run SD card host has no options")?;
        if options.contains_key("latency") {
            return Err("run SD card host has a measured timing model, not transfer ticks".into());
        }
        options.insert("transfer_ticks".into(), json!(ticks));
        Ok(())
    }
    /// Model the SD host's initialization fault, as the catalog device model
    /// names it: an object such as
    /// {"reject_standby_cmd3_and_cmd8_once_after": 2}, or "reject_cmd8".
    fn set_card_initialization_fault(&mut self, fault: &Value) -> Result<(), String> {
        let nodes = self
            .devices_value
            .get_mut("nodes")
            .and_then(Value::as_array_mut)
            .ok_or("prepared devices have no node list")?;
        let sd = nodes
            .iter_mut()
            .find(|node| node.get("name").and_then(Value::as_str) == Some("sd"))
            .ok_or("run has no modeled SD card host")?;
        let options = sd
            .pointer_mut("/model/options")
            .and_then(Value::as_object_mut)
            .ok_or("run SD card host has no options")?;
        options.insert("initialization_fault".into(), fault.clone());
        Ok(())
    }
    fn set_mt5931(
        &mut self,
        access_point: Option<Value>,
        capability_reply: Option<[u8; 20]>,
    ) -> Result<(), String> {
        let nodes = self
            .devices_value
            .get_mut("nodes")
            .and_then(Value::as_array_mut)
            .ok_or("prepared devices have no node list")?;
        let sdio = nodes
            .iter_mut()
            .find(|node| node.get("name").and_then(Value::as_str) == Some("sdio"))
            .ok_or("run has no MT5931 endpoint")?;
        let module = sdio
            .pointer_mut("/model/module")
            .ok_or("run has no MT5931 module")?;
        if module.get("protocol").and_then(Value::as_str) != Some("mt5931") {
            return Err("run MT5931 module is not in endpoint mode".into());
        }
        let mt5931 = module
            .get_mut("mt5931")
            .and_then(Value::as_object_mut)
            .ok_or("run has no MT5931 options")?;
        if let Some(access_point) = access_point {
            mt5931.insert("access_point".into(), access_point);
        }
        if let Some(capability_reply) = capability_reply {
            mt5931.insert("capability_reply".into(), json!(capability_reply));
        }
        Ok(())
    }
}

/// The camera's compressed NAND sections in `directory`: one
/// `nand-<section>.bin.zst` per section of the firmware's settings layout that
/// the camera holds. A section the camera leaves empty has no file.
fn camera_sections(
    prepared: &Prepared,
    directory: &Path,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let layout = prepared
        .resolved
        .reference
        .settings
        .as_ref()
        .ok_or("selected firmware has no settings layout")?;
    let mut sections = BTreeMap::new();
    for section in &layout.sections {
        let path = directory.join(format!("nand-{}.bin.zst", section.name));
        if path.exists() {
            sections.insert(section.name.clone(), read(&path, CAMERA_SECTION_MAX_BYTES)?);
        }
    }
    if sections.is_empty() {
        return Err(format!(
            "{} holds no camera NAND sections",
            directory.display()
        ));
    }
    Ok(sections)
}

struct RestartBuilder {
    prepared: Prepared,
    inputs: InputBytes,
    mode: Mode,
}
impl fujimod_emulator_session::power_cycle::ColdBoot for RestartBuilder {
    fn build(&mut self, tick: u64, counts: Vec<u64>, packet: [u8; 16]) -> Result<Session, String> {
        open_prepared_epoch(
            self.prepared.duplicate()?,
            self.inputs.clone(),
            self.mode,
            Some((tick, counts, packet)),
        )
    }
}

/// Construct an unexecuted strict board from a prepared run and byte inputs.
/// A SoC that declares its power-on RAM contents starts each epoch with them.
pub fn open_prepared(prepared: Prepared, inputs: InputBytes) -> Result<Session, String> {
    open_prepared_mode(prepared, inputs, Mode::Strict)
}

fn open_prepared_mode(
    prepared: Prepared,
    inputs: InputBytes,
    mode: Mode,
) -> Result<Session, String> {
    let cycle = if prepared.resolved.reference.firmware.id == "x70-110" {
        Some(RestartBuilder {
            prepared: prepared.duplicate()?,
            inputs: inputs.clone(),
            mode,
        })
    } else {
        None
    };
    let mut session = open_prepared_epoch(prepared, inputs, mode, None)?;
    if let Some(builder) = cycle {
        let media = fujimod_emulator_session::durable::Spec {
            cards: session.contacts.keys().cloned().collect(),
            nands: vec!["nand".into()],
        };
        session.configure_power_cycle(
            fujimod_emulator_session::power_cycle::PowerCycle::new(
                "clocks".into(),
                media,
                Box::new(builder),
            )
            .with_logic_inputs(vec!["usb_cable".into()]),
        )?;
    }
    Ok(session)
}

fn power_on_ram_fill(prepared: &Prepared) -> Option<u8> {
    prepared.resolved.power_on_ram_fill()
}

fn open_prepared_epoch(
    prepared: Prepared,
    inputs: InputBytes,
    mode: Mode,
    epoch: Option<(u64, Vec<u64>, [u8; 16])>,
) -> Result<Session, String> {
    // A browser allocates Wasm pages as they are first written: filling all of
    // DRAM makes every page resident, past a phone tab's memory. Browser boots
    // start RAM zeroed; native boots keep the measured power-on fill.
    #[cfg(not(target_os = "emscripten"))]
    let ram_fill = power_on_ram_fill(&prepared);
    #[cfg(target_os = "emscripten")]
    let ram_fill: Option<u8> = None;
    prepared
        .update
        .check(&inputs.update)
        .map_err(|error| format!("update: {error}"))?;
    prepared
        .boot_rom
        .content
        .check(&inputs.boot_rom)
        .map_err(|error| format!("boot ROM: {error}"))?;
    if inputs
        .nand
        .as_ref()
        .is_some_and(|bytes| bytes.len() > NAND_INPUT_MAX_BYTES)
    {
        return Err("NAND image must be at most 256 MiB".into());
    }
    let mut session_definition = prepared.to_json();
    session_definition["mode"] = json!(mode.name());
    session_definition["inputs"]["nand_selected"] = json!(inputs.nand.is_some());
    session_definition["inputs"]["settings_selected"] = json!(inputs.settings.is_some());
    let InputBytes {
        update,
        boot_rom,
        nand,
        settings,
        clock_seconds_2000,
        preferred_languages,
        camera,
    } = inputs;
    session_definition["inputs"]["preferred_languages"] = json!(preferred_languages);
    session_definition["inputs"]["clock_seconds_2000"] = json!(clock_seconds_2000);
    if prepared.flash_offset >= update.len() {
        return Err("update flash offset exceeds input".into());
    }
    let flash: Vec<u8> = update[prepared.flash_offset..].iter().map(|b| !b).collect();
    let mut assets = BTreeMap::from([("nand_base".into(), flash)]);
    for input in &prepared.generated_assets {
        if input.pattern.is_empty()
            || input.size > 64 << 20
            || !input.size.is_multiple_of(input.pattern.len())
        {
            return Err("generated scene input has an invalid extent".into());
        }
        if assets
            .insert(
                input.name.clone(),
                input.pattern.repeat(input.size / input.pattern.len()),
            )
            .is_some()
        {
            return Err("duplicate scene asset".into());
        }
    }
    let mut devices = prepared.devices_value.clone();
    let mut scene = prepared.scene.clone();

    // Set real-time clock if provided
    if let Some(clock_seconds) = clock_seconds_2000 {
        apply_clock(&mut devices, clock_seconds)?;
    }

    if let Some(packet_bytes) = &settings {
        settings_packet(&mut devices, prepared.mcu.as_ref(), packet_bytes)?;
    }
    let declared = match prepared.resolved.configuration.inputs.as_ref() {
        Some(RunInputs::Exr2(inputs)) => inputs.settings.clone(),
        _ => prepared
            .resolved
            .configuration
            .preset
            .as_ref()
            .and_then(|preset| preset.settings.clone()),
    };
    let mut header_applied = false;
    match &declared {
        Some(declared) => {
            if nand.is_some() || settings.is_some() {
                return Err(
                    "a run that declares its settings takes no NAND image or settings packet"
                        .into(),
                );
            }
            let layout = prepared
                .resolved
                .reference
                .settings
                .as_ref()
                .ok_or("selected firmware has no settings layout")?;
            match (&declared.camera, camera.is_empty()) {
                (Some(id), true) => {
                    return Err(format!(
                        "this run boots the body's {id} camera: supply its NAND sections"
                    ));
                }
                (None, false) => {
                    return Err(
                        "this run boots no camera, but camera sections were supplied".into(),
                    );
                }
                _ => {}
            }
            // The power controller keeps the user profile's header; the
            // camera's sections only change the settings bank.
            let built =
                fujimod_settings::build(layout, &assets["nand_base"], &declared.requested, None)?;
            if let Some(profile) = built.profile {
                // The header is delivered through an MCU register endpoint; a
                // body without one keeps only the settings bank.
                if prepared.mcu.is_some() {
                    settings_packet(&mut devices, prepared.mcu.as_ref(), &profile.header)?;
                    header_applied = true;
                }
            }
            let mut names = BTreeMap::new();
            for (section, bytes) in camera {
                let name = format!("camera_{section}");
                if assets.insert(name.clone(), bytes).is_some() {
                    return Err("camera section asset repeats a run asset".into());
                }
                names.insert(section, name);
            }
            let nand = &mut scene["nands"]["nand"];
            // Without a camera's bank sections the bank can only be fabricated
            // from the update's defaults table over zero bytes, which stock
            // firmware reads as a valid bank. Leave the bank erased so the
            // firmware copies the defaults image the update installed instead.
            *nand = if names.is_empty() {
                json!({"kind": "profile", "options": nand["options"], "base": "nand_base",
                    "layout": layout, "requested": declared.requested})
            } else {
                json!({"kind": "settings", "options": nand["options"], "base": "nand_base",
                    "layout": layout, "requested": declared.requested, "camera": names})
            };
        }
        None if !camera.is_empty() => {
            return Err("this run declares no settings, but camera sections were supplied".into());
        }
        None => {}
    }
    if let Some(bytes) = nand {
        let nand = &mut scene["nands"]["nand"];
        let main_bytes = nand["options"]["bytes"]
            .as_u64()
            .and_then(|b| usize::try_from(b).ok())
            .ok_or("NAND geometry is absent")?;
        if bytes.len() == main_bytes || managed_spare_trailer(&bytes, main_bytes) {
            let (image, spares) = split_managed_spares(&bytes, main_bytes)?;
            if !image.starts_with(&assets["nand_base"]) {
                return Err(
                    "NAND main-area firmware prefix differs from the selected update".into(),
                );
            }
            let mut spare_rows = Vec::with_capacity(spares.len());
            for (row, tag) in spares {
                let name = format!("nand_spare_{row}");
                assets.insert(name.clone(), tag.to_vec());
                spare_rows.push(json!([row, name]));
            }
            nand["overlays"] = json!([[0, "nand_image"]]);
            nand["spare"] = json!(spare_rows);
            assets.insert("nand_image".into(), image.to_vec());
        } else {
            *nand = json!({"kind":"committed","options":nand["options"],"base":"nand_base","image":"nand_image"});
            assets.insert("nand_image".into(), bytes);
        }
    }
    if let Some((_, _, packet)) = &epoch {
        settings_packet(&mut devices, prepared.mcu.as_ref(), packet)?;
    }
    let graph = prepared.execution;
    graph.validate()?;
    let plan = if settings.is_some()
        || header_applied
        || clock_seconds_2000.is_some()
        || epoch.is_some()
    {
        Plan::parse(&serde_json::to_vec(&devices).map_err(|e| e.to_string())?)?
    } else {
        prepared.plan
    };
    plan.validate_reset_outputs(
        &graph
            .processors
            .iter()
            .map(|p| p.released)
            .collect::<Vec<_>>(),
    )?;
    plan.validate_memory_outputs(
        &graph
            .decoders
            .iter()
            .map(|d| MemoryChoices {
                initial: d.initial,
                choices: d.choices.len(),
            })
            .collect::<Vec<_>>(),
    )?;
    // Unit tests have no final crate to register the board builder and models.
    #[cfg(test)]
    fujimod_emulator_exr2_board::register_board_builder();
    #[cfg(test)]
    fujimod_emulator_models::register();
    let resources = serde_json::from_value::<preparation::Spec>(scene.clone())
        .map_err(|e| e.to_string())?
        .build(&assets)?;
    let built = plan.build(
        resources,
        epoch.as_ref().map_or(0, |e| e.0),
        &mut RefuseAdapters,
    )?;
    let bus = Bus::from_graph(mode, built.graph)?;
    let mut options = factory::Options::new(prepared.execution_policy.engine);
    if epoch.is_none()
        && let Some(byte) = ram_fill
    {
        options = options.with_ram_fill(byte);
    }
    if let Some((tick, counts, _)) = epoch {
        options = options.with_epoch(tick, counts);
    }
    let mut board = graph.build(options, bus, None)?;
    for controller in built.core_interrupts {
        board.configure_core_interrupts(
            controller.core,
            controller.device,
            Box::new(controller.inputs),
        )?;
    }
    board.write_physical(prepared.boot_rom.address, &boot_rom)?;
    for reset in prepared.reset_coprocessors {
        let cpu = board.cpu(reset.core)?;
        let old = cpu.coprocessor(reset.register, None)?;
        cpu.coprocessor(reset.register, Some(old | reset.set_bits))?;
    }
    Ok(Session {
        definition: session_definition,
        board,
        controls: None,
        contact_driver: None,
        resources: built.resources,
        nodes: built.node_ids,
        contacts: built.contacts,
        power_cycle: None,
        window_action: None,
        recording_frames: Default::default(),
    })
}

/// A stopped, passive boot observation. `rec` also identifies the first-start
/// recording role (`system_mode == 3`) after its menus close.
#[derive(Clone, Debug, Serialize, tsify::Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub struct RecReport {
    pub status: String,
    pub instructions: u64,
    pub virtual_ticks: u64,
    pub ui_mode: u32,
    pub system_mode: u32,
    pub menu_active: u32,
    pub menu_node: u32,
    pub pending_mode: u32,
    pub pending: bool,
    pub setup_presses: usize,
    /// Per processor translated-code cache allocated to each Unicorn engine.
    pub translation_cache_size_bytes: u32,
    /// First active scanout output presenting configured graphics, when present.
    pub scanout_instance: Option<usize>,
    pub failure: Option<String>,
    #[tsify(type = "unknown")]
    pub discovery_callee_stubs: Value,
}
fn word(session: &Session, address: u64) -> Result<u32, String> {
    let mut bytes = [0; 4];
    session.board.read_physical(address, &mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}
fn observe(session: &Session, spec: &Prepared, presses: usize) -> Result<RecReport, String> {
    let o = &spec.observation;
    let wide = |address| -> Result<u64, String> {
        let mut bytes = [0; 8];
        session.board.read_physical(address, &mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    };
    let r = &o.readiness;
    let one = wide(r.category_one)?;
    let two = wide(r.category_two)?;
    let pending = one & (1 << 9) != 0
        || r.input_pending
            .map(|a| word(session, a))
            .transpose()?
            .unwrap_or(0)
            != 0
        || two & !r.excluded_mask != 0
        || word(session, r.override_address)? != 1 && two & r.excluded_mask != 0
        || r.operation_alert_timer
            .map(|a| word(session, a))
            .transpose()?
            .unwrap_or(0)
            != 0;
    Ok(RecReport {
        status: "running".into(),
        instructions: session.board.instruction_counts().iter().sum(),
        virtual_ticks: session.board.now(),
        ui_mode: word(session, o.ui_mode)?,
        system_mode: word(session, o.system_mode)?,
        menu_active: word(session, o.menu_active)?,
        menu_node: word(session, o.menu_node)?,
        pending_mode: word(session, o.pending_mode)?,
        pending,
        setup_presses: presses,
        translation_cache_size_bytes: session.board.translation_cache_size_bytes(),
        scanout_instance: presented_instances(&session.scanout_state(&o.scanout)?)
            .into_iter()
            .next(),
        failure: None,
        discovery_callee_stubs: session.board.discovery_callee_stubs(),
    })
}

fn presented_instances(display: &Value) -> Vec<usize> {
    display["instances"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|instance| {
            instance["graphics"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|layer| layer["channels"].as_array().into_iter().flatten())
                .any(|channel| {
                    channel["enabled"] == true
                        && channel["configured"] == true
                        && channel["width"].as_u64().is_some_and(|n| n > 1)
                        && channel["height"].as_u64().is_some_and(|n| n > 1)
                })
        })
        .filter_map(|instance| instance["instance"].as_u64()?.try_into().ok())
        .collect()
}
type WindowAction<'a> = dyn FnMut(&mut Session) -> Result<Option<u64>, String> + 'a;

struct Driver<'a> {
    limits: host::Limits,
    action: &'a mut WindowAction<'a>,
    automatic_setup: bool,
}

impl Driver<'_> {
    fn advance(
        &mut self,
        session: &mut Session,
        spec: &Prepared,
        until: Option<u64>,
    ) -> Result<(), String> {
        if self.limits.reached(session) {
            return Err("run limit reached during first-start input".into());
        }
        let now: u64 = session.board.instruction_counts().iter().sum();
        let pause = now.saturating_add(1_000_000).min(self.limits.instructions);
        let until = until.into_iter().chain((self.action)(session)?).min();
        if !self
            .limits
            .step(session, spec.quantum, Some(pause), until)?
        {
            return Err("board is quiescent without an input or device deadline".into());
        }
        Ok(())
    }

    fn advance_ticks(
        &mut self,
        session: &mut Session,
        spec: &Prepared,
        ticks: u64,
    ) -> Result<(), String> {
        let until = session
            .board
            .now()
            .checked_add(ticks)
            .ok_or("input deadline overflows")?;
        while session.board.now() < until {
            self.advance(session, spec, Some(until))?;
        }
        Ok(())
    }
}

fn session_prepared(session: &Session) -> Result<Prepared, String> {
    let id = session.definition["configuration_id"]
        .as_str()
        .ok_or("session has no boot configuration")?;
    let overlays = session.definition["overlays"]
        .as_array()
        .ok_or("session has no selected overlays")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or("session overlay identity is not a string")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut prepared = prepare(id, &overlays)?;
    if let Some(quantum) = session.definition["quantum"].as_u64() {
        prepared.quantum = usize::try_from(quantum).map_err(|_| "boot quantum exceeds usize")?;
    }
    Ok(prepared)
}

fn press_setup_key(
    session: &mut Session,
    spec: &Prepared,
    s: &FirstStart,
    name: &str,
    driver: &mut Driver<'_>,
) -> Result<(), String> {
    let selection = &spec.resolved.reference;
    let key = |session: &mut Session, pressed| {
        fujimod_emulator_controls::set_key(
            session,
            selection,
            &s.located,
            name,
            pressed,
            EXR2_TICKS_PER_SECOND,
        )
    };
    key(session, true)?;
    let held = driver.advance_ticks(session, spec, s.hold_ticks);
    // Release even if the instruction budget or a modeled refusal stops the hold.
    key(session, false)?;
    held?;
    driver.advance_ticks(session, spec, s.settle_ticks)
}

fn setup_key(
    session: &mut Session,
    spec: &Prepared,
    key: &str,
    driver: &mut Driver<'_>,
    presses: &mut usize,
) -> Result<(), String> {
    let start = spec
        .first_start
        .as_ref()
        .ok_or("first-start policy is absent")?;
    *presses += 1;
    press_setup_key(session, spec, start, key, driver)
}

fn setup_calendar(
    session: &mut Session,
    spec: &Prepared,
    menu: &fujimod_catalog::first_start::FirstStartMenu,
    driver: &mut Driver<'_>,
    presses: &mut usize,
) -> Result<(), String> {
    let seconds = session.definition["inputs"]["clock_seconds_2000"]
        .as_u64()
        .map(u32::try_from)
        .transpose()
        .map_err(|_| "local clock exceeds u32")?
        .unwrap_or(fujimod_catalog_configuration::DEFAULT_CLOCK_SECONDS_2000);
    let target = first_start::calendar(seconds);
    let read = |session: &Session| -> Result<([u16; 5], u8), String> {
        let mut bytes = [0; 8];
        session.board.read_physical(menu.date_editor, &mut bytes)?;
        Ok((
            [
                u16::from_le_bytes([bytes[0], bytes[1]]),
                u16::from(bytes[2]),
                u16::from(bytes[3]),
                u16::from(bytes[4]),
                u16::from(bytes[5]),
            ],
            bytes[7],
        ))
    };
    for (field, wanted) in target.into_iter().enumerate() {
        for _ in 0..6 {
            if read(session)?.1 as usize == field {
                break;
            }
            setup_key(session, spec, "RIGHT", driver, presses)?;
        }
        if read(session)?.1 as usize != field {
            return Err("date editor did not select the requested field".into());
        }
        let (minimum, maximum) = match field {
            0 => (2000, 2099),
            1 => (1, 12),
            2 => (1, first_start::month_days(target[0], target[1])),
            3 => (0, 23),
            _ => (0, 59),
        };
        for _ in 0..=maximum - minimum {
            let current = read(session)?.0[field];
            if current == wanted {
                break;
            }
            if !(minimum..=maximum).contains(&current) {
                return Err("date editor field is outside its calendar range".into());
            }
            let increase = if field == 0 {
                current < wanted
            } else {
                let count = maximum - minimum + 1;
                (wanted + count - current) % count <= (current + count - wanted) % count
            };
            let expected = if increase {
                if current == maximum {
                    minimum
                } else {
                    current + 1
                }
            } else if current == minimum {
                maximum
            } else {
                current - 1
            };
            setup_key(
                session,
                spec,
                if increase { "UP" } else { "DOWN" },
                driver,
                presses,
            )?;
            let after = read(session)?;
            if after.0[field] != expected || after.1 as usize != field {
                return Err(format!(
                    "date editor field {field} did not change from {current} to {expected}: {}",
                    after.0[field]
                ));
            }
        }
        if read(session)?.0[field] != wanted {
            return Err("date editor exceeded its field navigation bound".into());
        }
    }
    if read(session)?.0 != target {
        return Err("date editor does not show the requested local time".into());
    }
    Ok(())
}

/// Run a named runtime to REC or an explicit stopped frontier. First-start menus
/// use calibrated UP/DOWN and MENU/OK ADC presses, never RAM writes.
pub fn to_rec(session: &mut Session, instruction_limit: u64) -> Result<RecReport, String> {
    to_rec_with(session, instruction_limit, None, |_session, _status| Ok(()))
}

/// Run EXR II's first-start and REC workflow with a guest-time ceiling and
/// an observational host callback between scheduler windows.
pub fn to_rec_with(
    session: &mut Session,
    instruction_limit: u64,
    duration: Option<std::time::Duration>,
    observe_window: impl FnMut(&mut Session, &RecReport) -> Result<(), String>,
) -> Result<RecReport, String> {
    to_rec_with_actions(
        session,
        instruction_limit,
        duration,
        true,
        |_| Ok(None),
        observe_window,
    )
}

/// Run with explicit scheduled host actions before each scheduler window.
pub fn to_rec_with_actions(
    session: &mut Session,
    instruction_limit: u64,
    duration: Option<std::time::Duration>,
    automatic_setup: bool,
    mut action: impl FnMut(&mut Session) -> Result<Option<u64>, String>,
    mut observe_window: impl FnMut(&mut Session, &RecReport) -> Result<(), String>,
) -> Result<RecReport, String> {
    let mut span = fujimod_perf::span(fujimod_perf::SpanKind::Boot);
    let start = boot_progress(session);
    let limits = host::Limits::new(
        session,
        Some(instruction_limit),
        duration,
        EXR2_TICKS_PER_SECOND,
    )?;
    let mut driver = Driver {
        limits,
        action: &mut action,
        automatic_setup,
    };
    let result = to_rec_inner(session, &mut driver, &mut observe_window);
    span.guest(start, boot_progress(session));
    result
}

fn boot_progress(session: &Session) -> fujimod_perf::GuestProgress {
    fujimod_perf::GuestProgress {
        instructions: session.board.instruction_counts().iter().sum(),
        ticks: session.board.now(),
    }
}

fn to_rec_inner(
    session: &mut Session,
    driver: &mut Driver<'_>,
    observe_window: &mut impl FnMut(&mut Session, &RecReport) -> Result<(), String>,
) -> Result<RecReport, String> {
    let spec = session_prepared(session)?;
    let limits = driver.limits;
    let mut presses = 0;
    let mut confirmations = 0;
    loop {
        let mut report = observe(session, &spec, presses)?;
        observe_window(session, &report)?;
        if limits.reached(session) {
            report.status = if report.instructions >= limits.instructions {
                "instruction-limit"
            } else {
                "time-limit"
            }
            .into();
            return Ok(report);
        }
        let presented = report.scanout_instance.is_some();
        if report.ui_mode == spec.observation.rec_mode
            && matches!(report.system_mode, 1 | 3)
            && report.menu_active == 0
            && report.pending_mode == 0
            && !report.pending
            && presented
        {
            report.status = "rec".into();
            return Ok(report);
        }
        let result = if driver.automatic_setup
            && report.ui_mode == spec.observation.setup_mode
            && report.system_mode == 3
            && report.menu_active != 0
            && report.pending_mode == 0
            && !report.pending
            && presented
            && spec
                .first_start
                .as_ref()
                .is_some_and(|first_start| confirmations < first_start.max_presses)
        {
            let start = spec.first_start.as_ref().expect("checked above");
            let inputs = match spec.resolved.configuration.inputs.as_ref() {
                Some(RunInputs::Exr2(inputs)) => inputs,
                _ => return Err("first start requires EXR II run inputs".into()),
            };
            let menu = spec
                .resolved
                .reference
                .first_start
                .as_ref()
                .ok_or("selected firmware has no first-start menu facts")?;

            if u64::from(report.menu_node) == menu.menu_node {
                let children = u64::from(word(session, menu.menu_node + menu.children_offset)?);
                for (row, language) in menu.languages.iter().enumerate() {
                    if word(session, children + row as u64 * menu.row_bytes)? != language.key {
                        return Err(
                            "START MENU language rows differ from their firmware facts".into()
                        );
                    }
                }
                let preferred: Vec<String> = session.definition["inputs"]["preferred_languages"]
                    .as_array()
                    .map(|tags| {
                        tags.iter()
                            .filter_map(|tag| tag.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let selected = first_start::select(
                    menu,
                    &preferred,
                    inputs
                        .first_start
                        .as_ref()
                        .and_then(|p| p.language.as_deref()),
                )?;
                let current = word(session, menu.cursor)? as usize;
                if current >= menu.languages.len() {
                    return Err("START MENU cursor is outside its language rows".into());
                }
                if current != selected {
                    presses += 1;
                    let key = if current > selected { "UP" } else { "DOWN" };
                    press_setup_key(session, &spec, start, key, driver).and_then(|()| {
                        let after = word(session, menu.cursor)? as usize;
                        let expected = if current > selected { current - 1 } else { current + 1 };
                        if after != expected { return Err(format!("START MENU cursor did not move from {current} to {expected}: {after}")); }
                        Ok(())
                    })
                } else {
                    presses += 1;
                    confirmations += 1;
                    press_setup_key(session, &spec, start, &start.key, driver)
                }
            } else {
                if word(session, u64::from(report.menu_node))? == menu.date_kind {
                    setup_calendar(session, &spec, menu, driver, &mut presses)?;
                }
                presses += 1;
                confirmations += 1;
                press_setup_key(session, &spec, start, &start.key, driver)
            }
        } else {
            driver.advance(session, &spec, None)
        };
        if let Err(error) = result {
            let mut report = observe(session, &spec, presses)?;
            report.status = if report.instructions >= limits.instructions {
                "instruction-limit"
            } else if limits.reached(session) {
                "time-limit"
            } else {
                "stopped"
            }
            .into();
            report.failure = Some(error);
            return Ok(report);
        }
    }
}

/// Guest tick frequency for the native EXR II timebase.
pub const EXR2_TICKS_PER_SECOND: u64 = 12_000_000;

/// Observe the boot progress words of a session built from a run configuration,
/// without advancing guest time. `status` reports `running` until the run
/// reaches REC.
pub fn status(session: &Session) -> Result<RecReport, String> {
    let spec = session_prepared(session)?;
    let mut report = observe(session, &spec, 0)?;
    let presented = report.scanout_instance.is_some();
    report.status = if report.ui_mode == spec.observation.rec_mode
        && matches!(report.system_mode, 1 | 3)
        && report.menu_active == 0
        && report.pending_mode == 0
        && !report.pending
        && presented
    {
        "rec"
    } else if report.failure.is_some() {
        "stopped"
    } else {
        "running"
    }
    .into();
    Ok(report)
}

/// The device node whose scanout presents the camera's display.
/// Resolving a configuration reads the whole catalog, so a session's display
/// node is resolved once per configuration and overlay set.
pub fn scanout_node(session: &Session) -> Result<String, String> {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};
    static NODES: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);
    let key = format!(
        "{}{}",
        session.definition["configuration_id"], session.definition["overlays"]
    );
    if let Some(node) = NODES.lock().map_err(|e| e.to_string())?.get(&key) {
        return Ok(node.clone());
    }
    let node = session_prepared(session)?.observation.scanout;
    NODES
        .lock()
        .map_err(|e| e.to_string())?
        .insert(key, node.clone());
    Ok(node)
}

/// Capture the first active native scanout output without advancing guest time.
pub fn screenshot(session: &Session) -> Result<Vec<u8>, String> {
    let spec = session_prepared(session)?;
    let display = session.scanout_state(&spec.observation.scanout)?;
    let instance = presented_instances(&display)
        .into_iter()
        .next()
        .ok_or("no active scanout output presents configured graphics")?;
    let frame = session
        .scanout(&spec.observation.scanout, instance, true, false)?
        .ok_or("active scanout output has no configured frame")?;
    let rgb = frame.rgb.ok_or("scanout did not return RGB")?;
    fujimod_recording::Frame::rgb(
        frame
            .width
            .try_into()
            .map_err(|_| "frame width exceeds u32")?,
        frame
            .height
            .try_into()
            .map_err(|_| "frame height exceeds u32")?,
        rgb,
    )?
    .png()
    .map(|p| p.to_vec())
}

/// The LCD images a session showed since the last call: the frames its
/// display devices published, or, for a display read by scanout, the image
/// it presents now. Hosts compare images; this never advances guest time.
pub fn lcd_frames(
    session: &mut Session,
) -> Result<Vec<fujimod_emulator_devices::graphics::scanout::Frame>, String> {
    let published = session.board.take_frames();
    if !published.is_empty() {
        return Ok(published);
    }
    let Ok(scanout) = scanout_node(session) else {
        return Ok(Vec::new());
    };
    let display = session.scanout_state(&scanout)?;
    let Some(instance) = presented_instances(&display).into_iter().next() else {
        return Ok(Vec::new());
    };
    let Some(frame) = session.scanout(&scanout, instance, true, false)? else {
        return Ok(Vec::new());
    };
    let rgb = frame.rgb.ok_or("scanout did not return RGB")?;
    let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
    for pixel in rgb.as_chunks::<3>().0 {
        rgba.extend_from_slice(pixel);
        rgba.push(255);
    }
    Ok(vec![fujimod_emulator_devices::graphics::scanout::Frame {
        output: instance,
        sequence: 0,
        width: frame
            .width
            .try_into()
            .map_err(|_| "frame width exceeds u32")?,
        height: frame
            .height
            .try_into()
            .map_err(|_| "frame height exceeds u32")?,
        rgba,
    }])
}

#[cfg(test)]
mod tests;
