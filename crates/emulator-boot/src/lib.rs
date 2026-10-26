//! Standalone boot from catalog facts and explicitly selected user files.
//!
//! Preparation runs before execution. Host readiness checks only observe RAM;
//! first-start acceptance drives the ordinary ADC input on the guest timeline.
mod first_start;
pub mod graph;
mod x70;

use fujimod_emulator::board::{
        bus::{Bus, Mode},
        factory,
    };
use fujimod_emulator_session::Session;
use fujimod_emulator_preparation as preparation;
use fujimod_catalog::reference::PartModel;
use fujimod_catalog_configuration::{
    Exr2RunInputs, RunEngine, RunExecution, RunExecutionMode, RunInputs, RunPurpose, RunScene,
    RunStart,
};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_catalog_assets::Content;
use fujimod_catalog_controls::{
    AdcInterval, ElectricalInput, InputTarget, facts::ConfigurationLocation,
};
use fujimod_emulator_cpu::arm::Coprocessor;
use fujimod_emulator_exr2_plan::{MemoryChoices, Plan, RefuseAdapters};
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
    pub configuration_id: String,
    pub overlays: Vec<String>,
    pub update: PathBuf,
    pub boot_rom: PathBuf,
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
    key_id: String,
    adc: String,
    channel: usize,
    calibration_pointer: u64,
    calibration_is_pointer: bool,
    lower_offset: u64,
    upper_offset: u64,
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
    mcu: Mcu,
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
    inputs: &Exr2RunInputs,
) -> Result<Option<FirstStart>, String> {
    let Some(policy) = &inputs.first_start else {
        return Ok(None);
    };
    first_start_key(resolved, inputs, &policy.key_id)
}

fn first_start_key(
    resolved: &ResolvedConfiguration,
    inputs: &Exr2RunInputs,
    key_id: &str,
) -> Result<Option<FirstStart>, String> {
    let Some(policy) = &inputs.first_start else {
        return Ok(None);
    };
    let facts = resolved
        .reference
        .firmware
        .generated_facts
        .as_ref()
        .ok_or("first-start input requires generated firmware control facts")?;
    let controls: fujimod_catalog_controls::FirmwareControlIds = serde_json::from_value(
        resolved
        .reference
        .control_ids
        .clone()
        .ok_or("first-start input requires firmware control IDs")?,
    )
    .map_err(|error| format!("firmware control IDs: {error}"))?;
    let mut matches = controls.inputs.iter().filter(|input| {
        input.debug_key.as_deref() == Some(key_id)
            && matches!(input.target, InputTarget::Control { .. })
            && input.electrical.is_some()
    });
    let input = matches
        .next()
        .ok_or_else(|| format!("firmware has no control predicate for key {}", key_id))?;
    if matches.next().is_some() {
        return Err(format!(
            "firmware key {} has ambiguous physical control predicates",
            key_id
        ));
    }
    let Some(ElectricalInput::Adc {
        connection_id,
        intervals,
        callback: None,
        ..
    }) = &input.electrical
    else {
        return Err(format!(
            "firmware key {} has no direct ADC predicate",
            key_id
        ));
    };
    let [AdcInterval::ConfigurationU16 { lower, upper }] = intervals.as_slice() else {
        return Err(format!(
            "firmware key {} requires one ADC calibration interval",
            key_id
        ));
    };
    let connection = resolved
        .reference
        .body
        .connections
        .iter()
        .find(|connection| connection.id == *connection_id)
        .ok_or("first-start ADC connection is absent from the body")?;
    let channel = connection
        .signals
        .iter()
        .filter_map(|signal| match signal {
            fujimod_catalog::reference::ConnectionSignal::Adc { channel, .. } => {
                Some(usize::from(*channel))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if channel.len() != 1 || inputs.adc.block_id != "adc" {
        return Err("first-start ADC target is ambiguous or unsupported".into());
    }
    let cfgdata: ConfigurationLocation = serde_json::from_value(
        facts
            .cfgdata_location
            .clone()
            .ok_or("first-start calibration has no firmware configuration-block location")?,
    )
    .map_err(|error| format!("firmware configuration-block location: {error}"))?;
    let (calibration_pointer, calibration_is_pointer) = match cfgdata {
        ConfigurationLocation::Pointer { address } => (address, true),
        ConfigurationLocation::Direct { address } => (address, false),
    };
    Ok(Some(FirstStart {
        key_id: key_id.to_owned(),
        adc: inputs.adc.block_id.clone(),
        channel: channel[0],
        calibration_pointer,
        calibration_is_pointer,
        lower_offset: u64::from(*lower),
        upper_offset: u64::from(*upper),
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
    }
    let inputs = match configuration.inputs.as_ref() {
        Some(RunInputs::Exr2(inputs)) => inputs,
        Some(RunInputs::Xp4(_)) => {
            return Err("native X70 boot does not support XP4 startup inputs".into());
        }
        Some(RunInputs::Xp5(_)) => {
            return Err("native X70 boot does not support XP5 startup inputs".into());
        }
        None => return Err("native X70 boot requires declared startup inputs".into()),
    };
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
    let execution = graph::assemble(
        &resolved.reference,
        execution_policy.instructions_per_tick,
        board_ticks_per_second,
    )?;
    let execution_value = graph::value(
        &resolved.reference,
        &execution,
        execution_policy.instructions_per_tick,
    );
    let (plan, devices_value, scene) =
        x70::assemble(&resolved.reference, configuration, inputs, scene_input)?;
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
                size: usize::try_from(asset.size)
                    .map_err(|_| format!("scene asset {} exceeds host address space", asset.id))?,
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
    let mcu_attachment = resolved
        .reference
        .body
        .microcontroller
        .as_ref()
        .ok_or("X70 run has no attached MCU")?;
    let mcu_part = resolved
        .reference
        .parts
        .iter()
        .find(|part| part.id == mcu_attachment.part_id)
        .ok_or("X70 MCU part is unresolved")?;
    let PartModel::Microcontroller {
        packet_registers, ..
    } = &mcu_part.model
    else {
        return Err("X70 MCU attachment is not a microcontroller part".into());
    };
    let node = x70::node_for_block(&mcu_attachment.controller_id)?;
    let mcu = Mcu {
        node,
        route: usize::from(mcu_attachment.channel),
        first_register: u8::try_from(packet_registers.first)
            .map_err(|_| "X70 MCU packet register exceeds one byte")?,
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
    /// Produce the stable JSON projection used by the native Python adapter.
    pub fn to_json(&self) -> Value {
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
                "mcu": {"node":self.mcu.node,"route":self.mcu.route,"first_register":self.mcu.first_register},
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
                    "readiness": {
                        "category_one":self.observation.readiness.category_one,
                        "category_two":self.observation.readiness.category_two,
                        "input_pending":self.observation.readiness.input_pending,
                        "override_address":self.observation.readiness.override_address,
                        "excluded_mask":self.observation.readiness.excluded_mask,
                        "operation_alert_timer":self.observation.readiness.operation_alert_timer,
                    }
                },
                "first_start": self.first_start.as_ref().map(|start| json!({
                    "key_id":start.key_id,
                    "adc":start.adc,"channel":start.channel,
                    "calibration_pointer":start.calibration_pointer,
                    "calibration_is_pointer":start.calibration_is_pointer,
                    "lower_offset":start.lower_offset,"upper_offset":start.upper_offset,
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
fn checked(path: &Path, content: &Content) -> Result<Vec<u8>, String> {
    let bytes = read(path, content.size)?;
    content
        .check(&bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
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
            if let Some(clock_obj) = map.get_mut("clock") {
                if let Value::Object(clock_map) = clock_obj {
                    if clock_map.contains_key("initial_seconds") {
                        clock_map.insert("initial_seconds".to_string(), json!(clock_seconds));
                        *count += 1;
                    }
                }
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
    for entry in entries.chunks_exact(MANAGED_SPARE_ENTRY_BYTES) {
        let row = u32::from_be_bytes(entry[..4].try_into().map_err(|_| "NAND spare row")?);
        let tag: [u8; 16] = entry[4..].try_into().map_err(|_| "NAND spare tag")?;
        if !rows.insert(row) {
            return Err("NAND managed spare rows repeat".into());
        }
        spares.push((row, tag));
    }
    Ok((&bytes[..main_bytes], spares))
}

fn settings_packet(plan: &mut Value, mcu: &Mcu, bytes: &[u8]) -> Result<(), String> {
    packet(bytes)?;
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

/// Construct an unexecuted strict board from explicitly selected files.
/// No directory search, downloaded image, captured default or firmware patch is used.
pub fn open(options: Options) -> Result<Session, String> {
    let prepared = prepare(&options.configuration_id, &options.overlays)?;
    let update = checked(&options.update, &prepared.update)?;
    let boot_rom = checked(&options.boot_rom, &prepared.boot_rom.content)?;
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
    open_prepared(prepared, inputs)
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

/// Construct an unexecuted strict board from a prepared run and byte inputs.
/// The engine is linked into the crate. A prepared run opens one board; prepare
/// again for another.
pub fn open_prepared(prepared: Prepared, inputs: InputBytes) -> Result<Session, String> {
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
        settings_packet(&mut devices, &prepared.mcu, packet_bytes)?;
    }
    let declared = match prepared.resolved.configuration.inputs.as_ref() {
        Some(RunInputs::Exr2(inputs)) => inputs.settings.clone(),
        _ => None,
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
                settings_packet(&mut devices, &prepared.mcu, &profile.header)?;
                header_applied = true;
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
            *nand = json!({"kind": "settings", "options": nand["options"], "base": "nand_base",
                "layout": layout, "requested": declared.requested, "camera": names});
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
    let graph = prepared.execution;
    graph.validate()?;
    let plan = if settings.is_some() || header_applied || clock_seconds_2000.is_some() {
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
    let built = plan.build(resources, 0, &mut RefuseAdapters)?;
    let bus_mode = match prepared.execution_policy.mode {
        RunExecutionMode::Strict => Mode::Strict,
    };
    let engine = match prepared.execution_policy.engine {
        RunEngine::Unicorn => factory::Engine::Unicorn,
        RunEngine::Hypervisor => factory::Engine::Hypervisor,
    };
    let bus = Bus::from_graph(bus_mode, built.graph)?;
    let mut board = graph.build(
        factory::Options::new(engine),
        bus,
        None,
    )?;
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
        recording_frames: Default::default(),
    })
}

/// A stopped, passive boot observation. `rec` also identifies the first-start
/// recording role (`system_mode == 3`) after its menus close.
#[derive(Clone, Debug, Serialize)]
#[derive(tsify::Tsify)]
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
}
fn word(session: &Session, address: u64) -> Result<u32, String> {
    let mut bytes = [0; 4];
    session.board.read_physical(address, &mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}
fn observe(session: &Session, spec: &Prepared, presses: usize) -> Result<RecReport, String> {
    let o = &spec.observation;
    let r = &o.readiness;
    let wide = |address| -> Result<u64, String> {
        let mut bytes = [0; 8];
        session.board.read_physical(address, &mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    };
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
fn advance(
    session: &mut Session,
    spec: &Prepared,
    end: u64,
    until: Option<u64>,
) -> Result<(), String> {
    let now: u64 = session.board.instruction_counts().iter().sum();
    if now >= end {
        return Err("instruction limit reached during first-start input".into());
    }
    let tick = session.board.now();
    session.board.run_with_limits(
        end - now,
        spec.quantum,
        Some(now.saturating_add(1_000_000).min(end)),
        until,
    )?;
    if session.board.now() == tick && session.board.instruction_counts().iter().sum::<u64>() == now
    {
        return Err("board is quiescent without an input or device deadline".into());
    }
    Ok(())
}
fn advance_ticks(
    session: &mut Session,
    spec: &Prepared,
    end: u64,
    ticks: u64,
) -> Result<(), String> {
    let until = session
        .board
        .now()
        .checked_add(ticks)
        .ok_or("input deadline overflows")?;
    while session.board.now() < until {
        advance(session, spec, end, Some(until))?;
    }
    Ok(())
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
    prepare(id, &overlays)
}

fn press_setup_key(
    session: &mut Session,
    spec: &Prepared,
    s: &FirstStart,
    end: u64,
) -> Result<(), String> {
    let base = if s.calibration_is_pointer {
        u64::from(word(session, s.calibration_pointer)?)
    } else {
        s.calibration_pointer
    };
    let half = |offset| -> Result<u16, String> {
        let mut bytes = [0; 2];
        session.board.read_physical(base + offset, &mut bytes)?;
        Ok(u16::from_le_bytes(bytes))
    };
    let lower = half(s.lower_offset)?;
    let upper = half(s.upper_offset)?;
    if lower >= upper || upper >= 1023 {
        return Err("first-start key calibration does not describe a pressed ADC interval".into());
    }
    let original = session.adc_samples(&s.adc)?;
    let mut pressed = original.clone();
    *pressed
        .get_mut(s.channel)
        .ok_or("first-start key ADC channel is absent")? = (lower + upper) / 2;
    session.set_adc_samples(&s.adc, &pressed)?;
    let held = advance_ticks(session, spec, end, s.hold_ticks);
    // Release even if the instruction budget or a modeled refusal stops the hold.
    session.set_adc_samples(&s.adc, &original)?;
    held?;
    advance_ticks(session, spec, end, s.settle_ticks)
}

fn setup_key(
    session: &mut Session,
    spec: &Prepared,
    inputs: &Exr2RunInputs,
    key: &str,
    end: u64,
    presses: &mut usize,
) -> Result<(), String> {
    let key =
        first_start_key(&spec.resolved, inputs, key)?.ok_or("first-start policy is absent")?;
    *presses += 1;
    press_setup_key(session, spec, &key, end)
}

fn setup_calendar(
    session: &mut Session,
    spec: &Prepared,
    inputs: &Exr2RunInputs,
    menu: &fujimod_catalog::first_start::FirstStartMenu,
    end: u64,
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
            setup_key(session, spec, inputs, "RIGHT", end, presses)?;
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
                inputs,
                if increase { "UP" } else { "DOWN" },
                end,
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
    if instruction_limit == 0 {
        return Err("instruction limit must be positive".into());
    }
    let spec = session_prepared(session)?;
    let end = session
        .board
        .instruction_counts()
        .iter()
        .sum::<u64>()
        .checked_add(instruction_limit)
        .ok_or("instruction limit overflows")?;
    let mut presses = 0;
    let mut confirmations = 0;
    loop {
        let mut report = observe(session, &spec, presses)?;
        if report.instructions >= end {
            report.status = "instruction-limit".into();
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
        let result = if report.ui_mode == spec.observation.setup_mode
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
            let result = if u64::from(report.menu_node) == menu.menu_node {
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
                    let key = first_start_key(&spec.resolved, inputs, key)?
                        .ok_or("first-start policy is absent")?;
                    press_setup_key(session, &spec, &key, end).and_then(|()| {
                        let after = word(session, menu.cursor)? as usize;
                        let expected = if current > selected { current - 1 } else { current + 1 };
                        if after != expected { return Err(format!("START MENU cursor did not move from {current} to {expected}: {after}")); }
                        Ok(())
                    })
                } else {
                    presses += 1;
                    confirmations += 1;
                    press_setup_key(session, &spec, start, end)
                }
            } else {
                if word(session, u64::from(report.menu_node))? == menu.date_kind {
                    setup_calendar(session, &spec, inputs, &menu, end, &mut presses)?;
                }
                presses += 1;
                confirmations += 1;
                press_setup_key(session, &spec, start, end)
            };
            result
        } else {
            advance(session, &spec, end, None)
        };
        if let Err(error) = result {
            let mut report = observe(session, &spec, presses)?;
            report.status = if report.instructions >= end {
                "instruction-limit"
            } else {
                "stopped"
            }
            .into();
            report.failure = Some(error);
            return Ok(report);
        }
    }
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

#[cfg(test)]
mod tests;
