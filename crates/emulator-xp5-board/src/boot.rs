//! XP5 device topology resolved from the selected SoC profile.
//!
//! The profile owns device addresses and options. Run inputs own the external
//! values supplied to those devices; neither the caller nor a board runner
//! needs a second copy of the register-window map.

use fujimod_emulator::board::{
    InterruptMap, aprofile,
    bus::{Bus, Mode},
    factory,
};
use fujimod_catalog_configuration::application_inputs::ApplicationRunScene;
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_catalog::{
    firmware::FirmwareProfile,
    profiles::RegisterWindow,
    reference::{ProcessorModel, ResolvedSelection, SocBlockKind},
    soc::{ApplicationHardware, InterruptControllerHardware},
};
use fujimod_emulator_cpu::{Register, arm64};
use fujimod_emulator_devices::{graph::{DeviceGraph, DeviceWindow}, parts::{controls::{Controls, Source}, pins::DriveInput}};
use fujimod_emulator_parts_controls::resistor_keys::{Options as ResistorKeysOptions, ResistorKeys};
use fujimod_emulator::board::aprofile::generic_timer;
use fujimod_emulator_ip_arm::gicv3::Gic;
use crate::board as xp5_board;
use serde_json::Value;
use std::time::Duration;
use std::{collections::BTreeMap, sync::Arc};

/// Generic CPU and interrupt-controller state declared by a caller-selected
/// firmware startup profile. The profile is data; opening the board does not
/// inspect firmware instructions or select a profile by firmware identity.
pub struct FirmwareStartup {
    pub processors: Vec<fujimod_emulator_boot::graph::ProcessorStart>,
    pub interrupts: Option<InterruptMap>,
    pub aprofile: Vec<(usize, aprofile::Config)>,
}

fn insert_start_register(
    registers: &mut Vec<(Register, u64)>,
    name: &str,
    value: u64,
) -> Result<(), String> {
    let register = match name {
        "PC" => Register::Pc,
        "PSTATE" => Register::Pstate,
        "SP" => Register::Sp,
        _ if name.starts_with('x') => {
            let index = name[1..]
                .parse::<u8>()
                .map_err(|error| format!("invalid AArch64 register {name}: {error}"))?;
            if index > 30 {
                return Err(format!("invalid AArch64 register {name}"));
            }
            Register::X(index)
        }
        _ => Register::Arm64System(arm64::system_register(name)?),
    };
    registers.push((register, value));
    Ok(())
}

fn is_gic_system_register(register: u16) -> bool {
    register == 0xc230
        || (register >> 14 == 3
            && (register >> 11) & 7 == 0
            && (register >> 7) & 15 == 12
            && (8..=15).contains(&((register >> 3) & 15)))
}

/// Translate caller-supplied catalog startup facts into generic XP5 runtime
/// processor, GIC, and architectural timer configuration.
pub fn startup_from_profile(
    resolved: &ResolvedConfiguration,
    profile: &FirmwareProfile,
    ticks_per_second: u64,
) -> Result<FirmwareStartup, String> {
    let soc = &resolved.reference.soc;
    let hardware = soc
        .hardware
        .as_ref()
        .ok_or("selected SoC has no hardware description")?;
    let ApplicationHardware::Armv8a { affinities, .. } = hardware
        .application
        .as_ref()
        .ok_or("XP5 SoC has no A-profile processor wiring")?
    else {
        return Err("XP5 SoC application processors are not ARMv8-A".into());
    };
    let cpus = profile
        .cpu
        .as_ref()
        .ok_or("selected firmware profile has no CPU startup state")?;
    let processors = cpus
        .iter()
        .map(|cpu| {
            let index = usize::try_from(cpu.id).map_err(|_| "CPU id exceeds host address space")?;
            let processor = soc.processors.get(index).ok_or_else(|| {
                format!("firmware CPU {} is absent from the selected SoC", cpu.id)
            })?;
            let affinity = affinities
                .get(&processor.id)
                .ok_or_else(|| format!("processor {} has no MPIDR affinity", processor.id))?;
            let mut registers = vec![(Register::Pc, cpu.pc), (Register::Pstate, cpu.cpsr)];
            for (name, value) in [
                ("x19", cpu.regs.x19),
                ("x20", cpu.regs.x20),
                ("x21", cpu.regs.x21),
            ] {
                insert_start_register(&mut registers, name, value)?;
            }
            for (name, value) in serde_json::to_value(&cpu.sysregs)
                .map_err(|error| error.to_string())?
                .as_object()
                .ok_or("firmware sysreg record is not an object")?
            {
                let register = arm64::system_register(name)?.encoding()?;
                if !is_gic_system_register(register) {
                    insert_start_register(
                        &mut registers,
                        name,
                        value
                            .as_u64()
                            .ok_or("firmware sysreg value is not an integer")?,
                    )?;
                }
            }
            insert_start_register(&mut registers, "MPIDR_EL1", *affinity)?;
            Ok(fujimod_emulator_boot::graph::ProcessorStart {
                processor_id: processor.id.clone(),
                released: cpu.running,
                registers,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let interrupts = match hardware.interrupt_controller.as_ref() {
        Some(InterruptControllerHardware::GicV3 {
            distributor_block_id,
            redistributor_block_id,
            redistributor_stride,
            lines,
        }) => {
            let block_base = |id: &str| {
                soc.blocks
                    .iter()
                    .find(|block| block.id == id)
                    .map(|block| block.base)
                    .ok_or_else(|| format!("GIC names absent SoC block {id}"))
            };
            let affinity_values = soc
                .processors
                .iter()
                .filter(|processor| processor.model == ProcessorModel::CortexA53)
                .map(|processor| {
                    affinities
                        .get(&processor.id)
                        .ok_or_else(|| format!("processor {} has no MPIDR affinity", processor.id))
                        .and_then(|value| {
                            u32::try_from(*value)
                                .map_err(|_| "GIC affinity exceeds 32 bits".to_string())
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut gic = Gic::new(affinity_values.clone(), *lines as usize)?;
            if let Some(cpus) = &profile.cpu {
                for cpu in cpus {
                    let core =
                        usize::try_from(cpu.id).map_err(|_| "CPU id exceeds host address space")?;
                    for (name, value) in serde_json::to_value(&cpu.sysregs)
                        .map_err(|error| error.to_string())?
                        .as_object()
                        .ok_or("firmware sysreg record is not an object")?
                    {
                        let register = arm64::system_register(name)?.encoding()?;
                        if is_gic_system_register(register) {
                            gic.system(
                                core,
                                register,
                                Some(
                                    value
                                        .as_u64()
                                        .ok_or("firmware sysreg value is not an integer")?,
                                ),
                            )?;
                        }
                    }
                }
            }
            if let Some(initial) = &profile.gic_initial {
                for (offset, value) in initial {
                    let offset = u64::from_str_radix(offset.trim_start_matches("0x"), 16).map_err(
                        |error| format!("invalid GIC distributor offset {offset}: {error}"),
                    )?;
                    gic.distributor(offset, 4, Some(*value))?;
                }
            }
            if let Some(initial) = &profile.gic_redistributor_initial {
                for register in initial {
                    let core = usize::try_from(register.core)
                        .map_err(|_| "GIC redistributor core exceeds host address space")?;
                    let size = u8::try_from(register.size)
                        .map_err(|_| "GIC register width exceeds 8 bits")?;
                    gic.redistributor(core, register.offset, size, Some(register.value))?;
                }
            }
            Some(InterruptMap {
                gic: Box::new(gic),
                registers: fujimod_emulator::board::InterruptRegisters::GicV3 {
                    distributor: block_base(distributor_block_id)?,
                    redistributor: block_base(redistributor_block_id)?,
                    stride: *redistributor_stride,
                },
                cores: affinity_values.len(),
            })
        }
        _ => None,
    };

    let ApplicationHardware::Armv8a {
        counter_clock_id,
        pmu_clock_id,
        physical_timer_irq,
        virtual_timer_irq,
        ..
    } = hardware
        .application
        .as_ref()
        .ok_or("XP5 SoC has no A-profile timer wiring")?
    else {
        return Err("XP5 SoC application processors are not ARMv8-A".into());
    };
    let clock_frequency = |id: &str| {
        soc.clocks
            .iter()
            .find(|clock| clock.id == id)
            .and_then(|clock| clock.frequency_hz)
            .ok_or_else(|| format!("SoC clock {id} has no declared frequency"))
    };
    let distributor = match interrupts.as_ref().map(|gic| &gic.registers) {
        Some(fujimod_emulator::board::InterruptRegisters::GicV3 { distributor, .. }) => *distributor,
        None => 0,
        _ => return Err("XP5 requires GICv3 interrupt registers".into()),
    };
    let timer_config = aprofile::Config {
        clock: generic_timer::Clock {
            counter_hz: clock_frequency(counter_clock_id)?,
            ticks_per_second,
        },
        physical_irq: *physical_timer_irq,
        virtual_irq: *virtual_timer_irq,
        pmu_hz: clock_frequency(pmu_clock_id)?,
        implementation: Some(aprofile::CortexA53 {
            cores: affinities.len(),
            peripheral_base: distributor,
        }),
    };
    let aprofile = soc
        .processors
        .iter()
        .enumerate()
        .filter(|(_, processor)| processor.model == ProcessorModel::CortexA53)
        .map(|(core, _)| (core, timer_config))
        .collect();
    Ok(FirmwareStartup {
        processors,
        interrupts,
        aprofile,
    })
}

fn ticks_as_microseconds(ticks: u64, ticks_per_second: u64) -> Result<i64, String> {
    let scaled = u128::from(ticks)
        .checked_mul(1_000_000)
        .ok_or("scene duration overflows microseconds")?;
    let rate = u128::from(ticks_per_second);
    if rate == 0 || scaled % rate != 0 {
        return Err("XP5 scene duration is not a whole number of microseconds".into());
    }
    i64::try_from(scaled / rate).map_err(|_| "scene duration exceeds device option range".into())
}

fn scene_window<'a>(
    resolved: &ResolvedConfiguration,
    windows: &'a mut [DeviceWindow],
    block_id: &str,
) -> Result<&'a mut DeviceWindow, String> {
    let block = resolved
        .reference
        .soc
        .blocks
        .iter()
        .find(|block| block.id == block_id)
        .ok_or_else(|| format!("XP5 scene names absent SoC block {block_id}"))?;
    windows
        .iter_mut()
        .find(|window| window.base == block.base)
        .ok_or_else(|| format!("XP5 scene block {block_id} has no caller device window"))
}

/// Apply catalog-declared frame and command-image inputs to caller-owned XP5
/// device windows. This maps scene facts to device options without inspecting
/// guest code or loading firmware bytes.
pub fn apply_run_scene(
    resolved: &ResolvedConfiguration,
    windows: &mut [DeviceWindow],
    scene: &ApplicationRunScene,
    ticks_per_second: u64,
) -> Result<(), String> {
    for frame in &scene.frames {
        let period =
            toml::Value::Integer(ticks_as_microseconds(frame.period_ticks, ticks_per_second)?);
        let active = frame
            .active_ticks
            .map(|ticks| ticks_as_microseconds(ticks, ticks_per_second))
            .transpose()?
            .map(toml::Value::Integer);
        for block_id in &frame.block_ids {
            let window = scene_window(resolved, windows, block_id)?;
            match window.kind.as_str() {
                "imaging-receiver" => {
                    if frame.luma.is_some() {
                        return Err(format!(
                            "XP5 scene luma is unsupported for receiver {block_id}"
                        ));
                    }
                    window
                        .options
                        .insert("frame_period_us".into(), period.clone());
                    match &active {
                        Some(active) => {
                            window
                                .options
                                .insert("frame_active_us".into(), active.clone());
                        }
                        None => {
                            window.options.remove("frame_active_us");
                        }
                    }
                }
                "movdc" | "imaging-preview" => {
                    window
                        .options
                        .insert("frame_period_us".into(), period.clone());
                    if let Some(luma) = frame.luma {
                        window
                            .options
                            .insert("source_luma".into(), toml::Value::Integer(i64::from(luma)));
                    }
                }
                _ => {
                    return Err(format!(
                        "XP5 frame scene is unsupported for {} at {:#x}",
                        window.kind, window.base
                    ));
                }
            }
        }
    }
    if !scene.statistics.is_empty() {
        return Err("XP5 statistics scenes are not supported by the selected devices".into());
    }
    for command in &scene.command_images {
        for block_id in &command.block_ids {
            let window = scene_window(resolved, windows, block_id)?;
            if !matches!(window.kind.as_str(), "imaging-rifp" | "imaging-rifp-io") {
                return Err(format!(
                    "XP5 command image is unsupported for {} at {:#x}",
                    window.kind, window.base
                ));
            }
            let luma = u8::try_from(command.luma)
                .map_err(|_| "XP5 command image luma exceeds the device's 8-bit input")?;
            window
                .options
                .insert("source_luma".into(), toml::Value::Integer(i64::from(luma)));
        }
    }
    Ok(())
}

fn attachment_kind(
    selection: &ResolvedSelection,
    window: &RegisterWindow,
) -> Result<&'static str, String> {
    let attachment = window.attachment.as_deref().ok_or_else(|| {
        format!(
            "XP5 window {} has no device kind or attachment",
            window.name
        )
    })?;
    let block = selection
        .soc
        .blocks
        .iter()
        .find(|block| block.id == attachment)
        .ok_or_else(|| {
            format!(
                "XP5 window {} attaches to missing SoC block {attachment}",
                window.name
            )
        })?;
    match block.kind {
        SocBlockKind::Timer => Ok("timer"),
        SocBlockKind::SerialController => Ok("spi-controller"),
        kind => Err(format!(
            "XP5 window {} cannot attach SoC block {} of kind {:?}",
            window.name, block.id, kind
        )),
    }
}

/// Convert one catalog window into the device graph's checked register window.
fn device_window(
    selection: &ResolvedSelection,
    window: &RegisterWindow,
) -> Result<DeviceWindow, String> {
    let kind = window
        .kind
        .as_deref()
        .map(str::to_owned)
        .map(Ok)
        .unwrap_or_else(|| attachment_kind(selection, window).map(str::to_owned))?;
    let options = window
        .options
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| format!("XP5 window {} options: {error}", window.name))?
        .map(|value| {
            toml::Value::try_from(value)
                .map_err(|error| format!("XP5 window {} options: {error}", window.name))
        })
        .transpose()?
        .and_then(|value| value.as_table().cloned())
        .unwrap_or_default();
    Ok(DeviceWindow {
        base: window.base,
        size: window.size,
        kind,
        options,
    })
}

/// Resolve the selected XP5 profile's explicit device windows.
pub fn windows(
    selection: &ResolvedSelection,
    profile_id: &str,
) -> Result<Vec<DeviceWindow>, String> {
    let profile = selection
        .soc
        .hardware
        .as_ref()
        .and_then(|hardware| hardware.profiles.get(profile_id))
        .ok_or_else(|| {
            format!(
                "SoC {} has no hardware profile {profile_id}",
                selection.soc.id
            )
        })?;
    let windows = profile
        .windows
        .as_ref()
        .ok_or_else(|| format!("SoC profile {profile_id} declares no register windows"))?;
    windows
        .iter()
        .map(|window| device_window(selection, window))
        .collect()
}

/// An XP5 device bus assembled from the selected SoC profile.
pub struct DeviceAssembly {
    pub contacts: BTreeMap<String, fujimod_emulator_devices::parts::card_contacts::Contacts>,
    pub bus: Bus,
    pub inputs: crate::session::Inputs,
    pub nodes: BTreeMap<String, usize>,
}

/// Construct XP5 devices and retain their shared external input state.
///
/// The resistor-key controller is supplied by the caller so its electrical
/// layout and timing remain application inputs rather than emulator policy.
pub fn assemble_devices(
    windows: Vec<DeviceWindow>,
    controls_options: ResistorKeysOptions,
    mut resources: xp5_board::Resources,
    mode: Mode,
) -> Result<DeviceAssembly, String> {
    let contacts = resources.sd.iter().map(|(base, media)| (format!("{base:#x}"), media.contacts.clone())).collect();
    let input_driver: Arc<dyn DriveInput> = resources.pins.clone();
    let controls = Arc::new(ResistorKeys::new(input_driver, controls_options)?);
    controls.start(Duration::ZERO);
    let control_interface: Arc<dyn Controls> = controls.clone();
    resources.controls = control_interface;
    let mut graph = DeviceGraph::new();
    let mut unknown = |window: &fujimod_emulator_devices::graph::DeviceWindow| {
        Err(format!(
            "unknown XP5 device {} at {:#x}",
            window.kind, window.base
        ))
    };
    xp5_board::register_all(&mut graph, &windows, &mut resources, &mut unknown)?;
    if !resources.sd.is_empty() { return Err("XP5 profile has unused card media".into()); }
    if !resources.displays.is_empty() {
        return Err("XP5 profile has unused display configurations".into());
    }
    let control_source = graph.attach(Box::new(Source::new(controls.clone())));
    let gpio_devices = windows
        .iter()
        .filter(|window| matches!(window.kind.as_str(), "gpio" | "gpio-port3"))
        .map(|window| graph.at(window.base))
        .collect::<Result<Vec<_>, _>>()?;
    let mut nodes = BTreeMap::new();
    for window in &windows {
        match graph.at(window.base) {
            Ok(node) => {
                nodes.insert(format!("{:#x}", window.base), node);
            }
            // Clock banks are a sparse envelope: its base is not itself a
            // register device, and holes in the envelope must remain unmapped.
            Err(_) if window.kind == "clock-banks" => {}
            Err(error) => return Err(error),
        }
    }
    // Expose the actual mapped nodes inside composite envelopes for callers
    // that need to inspect a specific clock bank.
    for node in &graph.nodes {
        for mapping in &node.mappings {
            nodes.insert(format!("{:#x}", mapping.base), graph.at(mapping.base)?);
        }
    }
    for &device in &gpio_devices {
        let mut causes = graph.nodes[device].native_dependencies.clone();
        causes.push(control_source);
        graph.depend_on(device, causes)?;
    }
    let bus = Bus::from_graph(mode, graph)?;
    Ok(DeviceAssembly {
        contacts,
        bus,
        inputs: crate::session::Inputs::new(
            resources.pins,
            controls,
            gpio_devices,
            control_source,
        ),
        nodes,
    })
}

/// One caller-supplied initial memory region for an XP5 run.
pub struct MemoryImage {
    pub address: u64,
    pub bytes: Vec<u8>,
}

/// Complete caller-owned inputs for opening an XP5 board on the shared factory.
///
/// Firmware bytes are supplied as memory images and storage backing. This API
/// does not compare them with a bundled firmware identity or apply patches.
pub struct OpenOptions {
    pub definition: Value,
    /// Refuse unsupported accesses, or continue with the board's discovery ledger.
    pub mode: Mode,
    pub processors: factory::Graph,
    pub windows: Vec<DeviceWindow>,
    pub controls: ResistorKeysOptions,
    pub resources: xp5_board::Resources,
    pub engine: factory::Engine,
    pub interrupts: Option<InterruptMap>,
    pub aprofile: Vec<(usize, aprofile::Config)>,
    pub images: Vec<MemoryImage>,
    pub scene: Option<Value>,
}

fn validate_images(graph: &factory::Graph, images: &[MemoryImage]) -> Result<(), String> {
    let mut ranges = Vec::with_capacity(images.len());
    let mut total = 0usize;
    for image in images {
        if image.bytes.is_empty() {
            return Err("XP5 memory image is empty".into());
        }
        total = total
            .checked_add(image.bytes.len())
            .ok_or("XP5 memory image extent overflows")?;
        if total > 512 << 20 {
            return Err("XP5 initial memory images exceed 512 MiB".into());
        }
        let end = image
            .address
            .checked_add(image.bytes.len() as u64)
            .ok_or("XP5 memory image address overflows")?;
        if !graph.ram.iter().any(|ram| {
            let ram_end = ram.base.checked_add(ram.size as u64);
            image.address >= ram.base && ram_end.is_some_and(|ram_end| end <= ram_end)
        }) {
            return Err("XP5 memory image escapes declared physical RAM".into());
        }
        ranges.push((image.address, end));
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err("XP5 initial memory images overlap".into());
    }
    Ok(())
}

/// Open a caller-described XP5 board using FujiMod's standard bus, CPU factory
/// and session, with the inputs that drive its keys and GPIO pins. Firmware
/// selection and byte identity policy belong to caller.
pub fn open(
    options: OpenOptions,
) -> Result<(fujimod_emulator_session::Session, crate::session::Inputs), String> {
    validate_images(&options.processors, &options.images)?;
    options.processors.validate()?;
    let assembly = assemble_devices(
        options.windows,
        options.controls,
        options.resources,
        options.mode,
    )?;
    let mut board = options.processors.build(
        factory::Options::new(options.engine),
        assembly.bus,
        options.interrupts,
    )?;
    for (core, profile) in options.aprofile {
        board.configure_aprofile(core, profile)?;
    }
    for image in options.images {
        board.write_physical(image.address, &image.bytes)?;
    }
    if let Some(scene) = options.scene.as_ref() {
        board.set_scene(scene)?;
    }
    let session = fujimod_emulator_session::Session {
        definition: options.definition,
        board,
        controls: None,
        contact_driver: None,
        resources: Default::default(),
        nodes: assembly.nodes,
        contacts: assembly.contacts,
        recording_frames: Default::default(),
    };
    Ok((session, assembly.inputs))
}
