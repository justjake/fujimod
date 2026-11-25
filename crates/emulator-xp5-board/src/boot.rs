//! XP5 device topology resolved from the selected SoC profile.
//!
//! The profile owns device addresses and options. Run inputs own the external
//! values supplied to those devices; neither the caller nor a board runner
//! needs a second copy of the register-window map.

use fujimod_catalog_configuration::RunEngine;
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
    reference::{AProfileState, ConnectionSignal, ProcessorModel, ResolvedSelection, SocBlockKind},
    soc::BlockDevice,
};
use fujimod_emulator_cpu::{Register, arm64};
use fujimod_emulator_devices::{graph::{DeviceGraph, DeviceWindow}, parts::gpio::Feedback};
use fujimod_emulator::board::aprofile::generic_timer;
use fujimod_emulator_ip_arm::gicv3::Gic;
use crate::board as xp5_board;
use serde_json::Value;

use std::collections::BTreeMap;

/// Generic CPU and interrupt-controller state declared by a caller-selected
/// firmware startup profile. The profile is data; opening the board does not
/// inspect firmware instructions or select a profile by firmware identity.
pub struct FirmwareStartup {
    pub processors: Vec<fujimod_emulator_board_graph::ProcessorStart>,
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
    let affinities = soc
        .processors
        .iter()
        .filter_map(|processor| match &processor.aprofile {
            Some(AProfileState::Armv8a { affinity, .. }) => {
                Some((processor.id.clone(), *affinity))
            }
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let cpus = profile
        .cpu
        .as_ref()
        .ok_or("selected firmware profile has no CPU startup state")?;
    // The experimental X-H2S reset profile releases every A53 at the common reset state
    // (assumption); the profile's own `running` flags only gate the generic path.
    let experiment = profile.rom_basis.as_deref() == Some("xh2s-reset-experiment");
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
            Ok(fujimod_emulator_board_graph::ProcessorStart {
                processor_id: processor.id.clone(),
                released: cpu.running || experiment,
                // Assumption (boot-ROM wait): the experimental profile's secondaries
                // wait in WFI at the common reset state until an SGI is pending for them.
                parked_until_sgi: experiment && cpu.id != 0,
                registers,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let interrupts = match soc.blocks.iter().find_map(|block| match &block.device {
        Some(BlockDevice::GicV3 {
            redistributor_block_id,
            redistributor_stride,
            lines,
            ..
        }) => Some((block, redistributor_block_id, redistributor_stride, lines)),
        _ => None,
    }) {
        Some((distributor, redistributor_block_id, redistributor_stride, lines)) => {
            let redistributor = soc
                .blocks
                .iter()
                .find(|block| block.id == *redistributor_block_id)
                .map(|block| block.base)
                .ok_or_else(|| format!("GIC names absent SoC block {redistributor_block_id}"))?;
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
                    distributor: distributor.base,
                    redistributor,
                    stride: *redistributor_stride,
                },
                cores: affinity_values.len(),
            })
        }
        None => None,
    };

    let mut wiring = None;
    for processor in &soc.processors {
        let Some(AProfileState::Armv8a {
            counter_clock_id,
            pmu_clock_id,
            physical_timer_irq,
            virtual_timer_irq,
            ..
        }) = &processor.aprofile
        else {
            continue;
        };
        let value = (
            counter_clock_id,
            pmu_clock_id,
            *physical_timer_irq,
            *virtual_timer_irq,
        );
        if wiring.is_some_and(|prior| prior != value) {
            return Err("XP5 SoC processors declare different architectural timer wiring".into());
        }
        wiring = Some(value);
    }
    let Some((counter_clock_id, pmu_clock_id, physical_timer_irq, virtual_timer_irq)) = wiring
    else {
        return Err("XP5 SoC has no ARMv8-A application processors".into());
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
        physical_irq: physical_timer_irq,
        virtual_irq: virtual_timer_irq,
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
    let drains = selection
        .body
        .hardware
        .as_ref()
        .map(|hardware| hardware.bus_drain.as_slice())
        .filter(|drains| !drains.is_empty())
        .ok_or_else(|| {
            format!(
                "body {} declares no bus-drain address the XP5 firmware reads",
                selection.body.id
            )
        })?;
    let mut devices = windows
        .iter()
        .map(|window| device_window(selection, window))
        .collect::<Result<Vec<_>, _>>()?;
    devices.extend(drains.iter().map(|drain| DeviceWindow {
        base: drain.address,
        size: 4,
        kind: "unmodeled-bus-drain".into(),
        options: Default::default(),
    }));
    let feedback = gpio_feedback(selection, None)?;
    if !feedback.is_empty() {
        let gpio = devices
            .iter_mut()
            .find(|window| window.kind == "gpio")
            .ok_or_else(|| format!("SoC profile {profile_id} has no GPIO window for body feedback"))?;
        if gpio.options.contains_key("feedback") {
            return Err(format!(
                "SoC profile {profile_id} GPIO window and body {} both declare feedback",
                selection.body.id
            ));
        }
        gpio.options.insert(
            "feedback".into(),
            toml::Value::try_from(feedback).map_err(|error| error.to_string())?,
        );
    }
    Ok(devices)
}

/// The XP5 device graph built from the catalog's block records.
///
/// Firmware, body and run facts are joined by block identity: a block keeps the
/// hardware window and model, the body supplies wiring and parts, and the run
/// supplies ADC samples, GPIO levels and flash contents.
pub fn windows_from_blocks(
    resolved: &fujimod_catalog_resolution::ResolvedConfiguration,
    dram_limit: u64,
) -> Result<Vec<DeviceWindow>, String> {
    use fujimod_catalog_configuration::application_inputs::ApplicationBlockInput;
    use fujimod_catalog::reference::SocBlock;

    let selection = &resolved.reference;
    let inputs = resolved
        .configuration
        .inputs
        .as_ref()
        .and_then(|inputs| match inputs {
            fujimod_catalog_configuration::RunInputs::Xp5(inputs) => Some(inputs.blocks.as_slice()),
            _ => None,
        })
        .unwrap_or_default();
    let flash_input = inputs.iter().find_map(|input| match input {
        ApplicationBlockInput::Flash {
            block_id,
            part_id,
            erased_tail,
            seeds,
            ..
        } if block_id == "flash-aperture" => Some((part_id.as_str(), erased_tail, seeds)),
        _ => None,
    });
    let part_inputs = resolved
        .configuration
        .inputs
        .as_ref()
        .and_then(|inputs| match inputs {
            fujimod_catalog_configuration::RunInputs::Xp5(inputs) => Some(inputs.parts.as_slice()),
            _ => None,
        })
        .unwrap_or_default();
    let presented_jedec = inputs.iter().find_map(|input| match input {
        ApplicationBlockInput::Flash { presented_jedec_id, .. } => *presented_jedec_id,
        _ => None,
    });
    let samples = inputs.iter().find_map(|input| match input {
        ApplicationBlockInput::Adc { samples, .. } => Some(samples.clone()),
        _ => None,
    });
    let levels = inputs.iter().find(|input| matches!(input, ApplicationBlockInput::Gpio { .. }));
    let profile = resolved
        .configuration
        .preset
        .as_ref()
        .and_then(|preset| preset.firmware_profile_id.as_ref())
        .and_then(|id| selection.firmware.layouts.as_ref()?.profiles.get(id));
    let flash_image = profile.and_then(|profile| profile.flash_image.as_ref());
    let drains = selection
        .body
        .hardware
        .as_ref()
        .map(|hardware| hardware.bus_drain.as_slice())
        .unwrap_or_default();
    let emulation = resolved.processor_emulation()?;
    let ticks_per_second = emulation
        .board_ticks_per_second
        .ok_or("XP5 processor emulation declares no board time base")?;
    for id in emulation
        .retained_word_blocks
        .iter()
        .chain(emulation.completion_latencies.iter().map(|latency| &latency.block_id))
    {
        if !selection.soc.blocks.iter().any(|block| &block.id == id) {
            return Err(format!("XP5 processor emulation names unknown block {id}"));
        }
    }
    let mut devices = Vec::new();
    for block in &selection.soc.blocks {
        let Some(device) = &block.device else {
            continue;
        };
        if let Some((kind, options)) = block_window(
            block, device, selection, flash_input, flash_image, samples.as_deref(), levels,
            &resolved.configuration, ticks_per_second, presented_jedec, part_inputs,
        )? {
            let mut options = options;
            if !options.contains_key("registers") && block.registers.iter().any(|register| register.reads.is_some()) {
                options.insert("registers".into(), stub_registers(block));
            }
            if emulation.retained_word_blocks.contains(&block.id) {
                options.insert("retain_words".into(), toml::Value::Boolean(true));
            }
            if let Some(latency) = emulation.completion_latencies.iter().find(|latency| latency.block_id == block.id) {
                let option = match kind {
                    "completion-stub" | "imaging-tnr" | "imaging-fcnv" | "imaging-filter-scaler"
                    | "imaging-rzl2" | "imaging-resz" => "delay_us",
                    "imaging-rifp-io" => "queue_completion_us",
                    "imaging-buffer" => "completion_us",
                    "xp5-codec" => "run_ticks",
                    _ => return Err(format!("block {}: {kind} takes no completion latency", block.id)),
                };
                let value = if option == "run_ticks" {
                    u128::from(latency.ticks)
                } else {
                    let micros = u128::from(latency.ticks) * 1_000_000;
                    if micros % u128::from(ticks_per_second) != 0 {
                        return Err(format!("block {}: latency is not a whole number of microseconds", block.id));
                    }
                    micros / u128::from(ticks_per_second)
                };
                options.insert(option.into(), toml::Value::Integer(i64::try_from(value).map_err(|_| format!("block {}: latency overflows", block.id))?));
            }
            devices.push(DeviceWindow {
                base: block.base,
                size: block.size,
                kind: kind.to_owned(),
                options,
            });
        }
    }
    // The firmware drains the bus by reading one word of each port and discarding it; a port
    // the run's DRAM covers is RAM instead.
    let ports = selection.soc.blocks.iter()
        .filter(|block| matches!(block.device, Some(BlockDevice::UnmodeledBusDrain { .. })))
        .map(|block| block.base)
        .chain(drains.iter().map(|drain| drain.address))
        .filter(|&address| address >= dram_limit)
        .collect::<std::collections::BTreeSet<_>>();
    for address in ports {
        devices.push(DeviceWindow {
            base: address,
            size: 4,
            kind: "discarded-read".into(),
            options: Default::default(),
        });
    }
    Ok(devices)
}

type FlashInput<'a> = (&'a str, &'a Option<u8>, &'a Vec<fujimod_catalog_configuration::application_inputs::FlashSeed>);

/// The bring-up register file answers of a block's registers that do not read back their last write.
fn stub_registers(block: &fujimod_catalog::reference::SocBlock) -> toml::Value {
    use fujimod_catalog::ip::RegisterReads;
    let integer = |value: u64| toml::Value::Integer(value as i64);
    toml::Value::Array(block.registers.iter().filter_map(|register| {
        let (mode, value) = match register.reads? {
            RegisterReads::Fixed { value } => ("constant", value),
            RegisterReads::Alternates { value } => ("toggle", value),
            RegisterReads::SelfClearing => ("command", 0),
            RegisterReads::AfterWrite { value } => ("after_write", value),
        };
        let mut row = toml::Table::new();
        row.insert("offset".into(), integer(register.offset));
        row.insert("mode".into(), toml::Value::String(mode.into()));
        if !matches!(register.reads, Some(RegisterReads::SelfClearing)) { row.insert("value".into(), integer(value)); }
        Some(toml::Value::Table(row))
    }).collect())
}

/// The completion-stub options of a start, completion and acknowledgement handshake.
fn handshake_options(
    options: &mut toml::Table,
    starts: &[fujimod_catalog::soc::CompletionStart],
    completion: &[fujimod_catalog::soc::CompletionWrite],
    ack: Option<&fujimod_catalog::soc::CompletionAck>,
    irq_mask: Option<&fujimod_catalog::soc::CompletionIrqMask>,
) {
    let integer = |value: u64| toml::Value::Integer(i64::try_from(value).unwrap_or(i64::MAX));
    if !starts.is_empty() {
    options.insert("starts".into(), toml::Value::Array(starts.iter().map(|start| {
        let mut row = toml::Table::new();
        row.insert("offset".into(), integer(start.offset));
        row.insert("mask".into(), integer(u64::from(start.mask)));
        toml::Value::Table(row)
    }).collect()));
    }
    if !completion.is_empty() {
    options.insert("completion".into(), toml::Value::Array(completion.iter().map(|write| {
        let mut row = toml::Table::new();
        row.insert("offset".into(), integer(write.offset));
        if let Some(set) = write.set { row.insert("set".into(), integer(u64::from(set))); }
        if let Some(clear) = write.clear { row.insert("clear".into(), integer(u64::from(clear))); }
        toml::Value::Table(row)
    }).collect()));
    }
    if let Some(ack) = ack {
    let mut row = toml::Table::new();
    row.insert("offset".into(), integer(ack.offset));
    row.insert("mode".into(), toml::Value::String("w1c".into()));
    if let Some(offset) = ack.status_offset { row.insert("status_offset".into(), integer(offset)); }
    if let Some(bits) = ack.bits { row.insert("bits".into(), integer(u64::from(bits))); }
    options.insert("ack".into(), toml::Value::Table(row));
    }
    if let Some(mask) = irq_mask {
    options.insert("irq_mask_offset".into(), integer(mask.offset));
    options.insert("irq_mask_bit".into(), integer(u64::from(mask.bit)));
    options.insert("irq_mask_set_masks".into(), toml::Value::Boolean(mask.set_masks));
    }
}

/// A stage's handshake and interrupt line as completion-stub options.
fn stage_handshake(options: &mut toml::Table, handshake: Option<&fujimod_catalog::soc::CompletionHandshake>, irq: Option<u32>) {
    if let Some(handshake) = handshake {
        handshake_options(options, &handshake.starts, &handshake.completion, handshake.ack.as_ref(), handshake.irq_mask.as_ref());
    }
    if let Some(line) = irq { options.insert("irq".into(), toml::Value::Integer(i64::from(line))); }
}

/// The device a serial port is emulated as when its one wired part is a whole-port output.
/// The chip-select pad of a display panel the body attaches to `block`.
fn panel_select(selection: &ResolvedSelection, block: &fujimod_catalog::reference::SocBlock) -> Result<Option<(u8, u8)>, String> {
    let attachments = selection.body.hardware.iter().flat_map(|hardware| &hardware.serial_attachments);
    for attachment in attachments.filter(|attachment| attachment.controller_id == block.id) {
        let panel = selection.parts.iter().any(|part| part.id == attachment.part_id && matches!(
            &part.model,
            fujimod_catalog::reference::PartModel::Peripheral { interface: fujimod_catalog::assembly::PeripheralPart::DisplayPanelSerial }
        ));
        if !panel {
            continue;
        }
        let select = attachment.select_connection_id.as_ref()
            .ok_or_else(|| format!("display panel on {} has no chip-select connection", block.id))?;
        let connection = selection.body.connections.iter().find(|connection| &connection.id == select)
            .ok_or_else(|| format!("display panel on {}: no connection {select}", block.id))?;
        let [ConnectionSignal::Gpio { port, pin, .. }] = connection.signals.as_slice() else {
            return Err(format!("connection {select} is not one GPIO line"));
        };
        return Ok(Some((*port, *pin)));
    }
    Ok(None)
}

fn port_device(selection: &ResolvedSelection, block: &fujimod_catalog::reference::SocBlock) -> Result<Option<&'static str>, String> {
    use fujimod_catalog::assembly::PeripheralPart;
    let mut kinds = selection.body.wiring.iter()
        .filter(|wire| wire.source_id == block.id && wire.signal == fujimod_catalog::reference::WireSignal::SerialData)
        .map(|wire| {
            let part = selection.parts.iter().find(|part| part.id == wire.target_id)
                .ok_or_else(|| format!("wire {} -> {}: no such part", wire.source_id, wire.target_id))?;
            Ok(match &part.model {
                fujimod_catalog::reference::PartModel::Peripheral { interface } => match interface {
                    PeripheralPart::FocusActuator => Some("focus-actuator"),
                    PeripheralPart::IrisActuator => Some("iris-actuator"),
                    PeripheralPart::AudioSerialOutput => Some("audio-spi-sink"),
                    PeripheralPart::ImagingSerialOutput => Some("imaging-serial-sink"),
                    _ => None,
                },
                _ => None,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    match kinds.as_slice() {
        [Some(_)] => Ok(kinds.pop().flatten()),
        many if many.iter().any(Option::is_some) => Err(format!("block {}: a whole-port output shares its port", block.id)),
        _ => Ok(None),
    }
}

/// The wire of a port whose one attached part has no chip select.
fn dedicated_wire(
    selection: &ResolvedSelection,
    block: &fujimod_catalog::reference::SocBlock,
) -> Result<Option<toml::Table>, String> {
    use fujimod_catalog::assembly::PeripheralPart;
    let attachments: Vec<_> = selection.body.hardware.iter()
        .flat_map(|hardware| &hardware.serial_attachments)
        .filter(|attachment| attachment.controller_id == block.id && attachment.select_connection_id.is_none())
        .collect();
    let [attachment] = attachments.as_slice() else {
        return if attachments.is_empty() { Ok(None) } else { Err(format!("block {}: several parts share a port without chip selects", block.id)) };
    };
    let part = selection.parts.iter().find(|part| part.id == attachment.part_id)
        .ok_or_else(|| format!("serial attachment {}: no such part", attachment.part_id))?;
    let fujimod_catalog::reference::PartModel::Peripheral { interface } = &part.model else {
        return Err(format!("part {} is not a serial peripheral", part.id));
    };
    let mut wire = toml::Table::new();
    match interface {
        PeripheralPart::FramedCommandPeer { initial_frame } => {
            wire.insert("kind".into(), toml::Value::String("dedicated".into()));
            let mut peer = toml::Table::new();
            peer.insert("kind".into(), toml::Value::String("framed-command-peer".into()));
            peer.insert("initial_frame".into(), toml::Value::Integer(i64::from(*initial_frame)));
            wire.insert("part".into(), toml::Value::Table(peer));
        }
        // A driver that never answers leaves its input at the idle level.
        PeripheralPart::UnidentifiedSerialDevice { miso_high } => {
            wire.insert("kind".into(), toml::Value::String("input-level".into()));
            wire.insert("high".into(), toml::Value::Boolean(*miso_high));
        }
        // Whole-port outputs are their own windows.
        PeripheralPart::FocusActuator | PeripheralPart::IrisActuator
        | PeripheralPart::AudioSerialOutput | PeripheralPart::ImagingSerialOutput => return Ok(None),
        other => return Err(format!("part {}: {other:?} cannot own a port", part.id)),
    }
    Ok(Some(wire))
}

/// The X-mount lens wire of the body's lens mount on `block`, with its mounted lens.
fn lens_wire(
    selection: &ResolvedSelection,
    block: &fujimod_catalog::reference::SocBlock,
) -> Result<Option<toml::Table>, String> {
    let Some(mount) = selection.body.hardware.as_ref().and_then(|hardware| hardware.lens_mount.as_ref()) else {
        return Ok(None);
    };
    if mount.controller_id != block.id {
        return Ok(None);
    }
    let pin = |id: &str| -> Result<toml::Value, String> {
        let connection = selection.body.connections.iter().find(|connection| connection.id == id)
            .ok_or_else(|| format!("lens mount: no connection {id}"))?;
        connection.signals.iter().find_map(|signal| match signal {
            ConnectionSignal::Gpio { port, pin, .. } => {
                let mut row = toml::Table::new();
                row.insert("port".into(), toml::Value::Integer(i64::from(*port)));
                row.insert("pin".into(), toml::Value::Integer(i64::from(*pin)));
                Some(toml::Value::Table(row))
            }
            _ => None,
        }).ok_or_else(|| format!("lens mount connection {id} carries no GPIO pad"))
    };
    let integer = |value: u64| toml::Value::Integer(i64::try_from(value).unwrap_or(i64::MAX));
    let mut wire = toml::Table::new();
    wire.insert("kind".into(), toml::Value::String("x-mount-lens".into()));
    wire.insert("detect".into(), pin(&mount.detect_connection_id)?);
    wire.insert("sense".into(), toml::Value::Array(mount.sense_connection_ids.iter().map(|id| pin(id)).collect::<Result<_, _>>()?));
    wire.insert("supplies".into(), toml::Value::Array(mount.supply_connection_ids.iter().map(|id| pin(id)).collect::<Result<_, _>>()?));
    wire.insert("ready".into(), pin(&mount.ready_connection_id)?);
    if let Some(function) = mount.ready_function { wire.insert("ready_mux".into(), integer(u64::from(function))); }
    wire.insert("ready_low_line".into(), integer(u64::from(mount.ready_low_line)));
    wire.insert("ready_high_line".into(), integer(u64::from(mount.ready_high_line)));
    let Some(lens_id) = &mount.lens_part_id else {
        return Err(format!("body {}: an empty lens mount is unmodeled", selection.body.id));
    };
    let lens = selection.parts.iter().find(|part| &part.id == lens_id).ok_or_else(|| format!("no lens part {lens_id}"))?;
    let fujimod_catalog::reference::PartModel::Peripheral {
        interface: fujimod_catalog::assembly::PeripheralPart::XMountLens { preset, sense_unpowered, startup_us, busy_us, operation_us },
    } = &lens.model else {
        return Err(format!("part {lens_id} is not an X-mount lens"));
    };
    wire.insert("lens".into(), toml::Value::String(preset.clone()));
    wire.insert("sense_unpowered".into(), toml::Value::Array(sense_unpowered.iter().map(|level| toml::Value::Boolean(*level)).collect()));
    wire.insert("startup_us".into(), integer(*startup_us));
    wire.insert("busy_us".into(), integer(*busy_us));
    wire.insert("operation_us".into(), integer(*operation_us));
    Ok(Some(wire))
}

/// The chip-selected endpoints body wiring attaches to a serial port, with their run inputs.
fn serial_endpoints(
    selection: &ResolvedSelection,
    block: &fujimod_catalog::reference::SocBlock,
    part_inputs: &[fujimod_catalog_configuration::application_inputs::ApplicationPartInput],
) -> Result<Vec<toml::Value>, String> {
    use fujimod_catalog::assembly::PeripheralPart;
    use fujimod_catalog_configuration::application_inputs::ApplicationPartInput as Input;
    let integer = |value: i64| toml::Value::Integer(value);
    let mut endpoints = Vec::new();
    let attachments = selection.body.hardware.iter().flat_map(|hardware| &hardware.serial_attachments);
    for attachment in attachments.filter(|attachment| {
        attachment.controller_id == block.id && attachment.select_connection_id.is_some()
    }) {
        if selection.parts.iter().any(|part| part.id == attachment.part_id && matches!(
            &part.model,
            fujimod_catalog::reference::PartModel::Peripheral { interface: PeripheralPart::DisplayPanelSerial }
        )) {
            continue;
        }
        let Some(select_id) = &attachment.select_connection_id else {
            return Err(format!("serial attachment {} on {} has no chip-select connection", attachment.part_id, block.id));
        };
        let connection = selection.body.connections.iter().find(|connection| &connection.id == select_id)
            .ok_or_else(|| format!("serial attachment {}: no connection {select_id}", attachment.part_id))?;
        let [ConnectionSignal::Gpio { port, pin, active_level }] = connection.signals.as_slice() else {
            return Err(format!("connection {select_id} is not one GPIO line"));
        };
        let part = selection.parts.iter().find(|part| part.id == attachment.part_id)
            .ok_or_else(|| format!("serial attachment {}: no such part", attachment.part_id))?;
        let fujimod_catalog::reference::PartModel::Peripheral { interface } = &part.model else {
            return Err(format!("part {} is not a serial peripheral", part.id));
        };
        let inputs: Vec<_> = part_inputs.iter().filter(|input| input.part_id() == part.id).collect();
        let missing = |what: &str| format!("part {}: the run supplies no {what}", part.id);
        let mut row = toml::Table::new();
        let kind = |row: &mut toml::Table, kind: &str| { row.insert("kind".into(), toml::Value::String(kind.into())); };
        match interface {
            PeripheralPart::PowerController { .. } => {
                kind(&mut row, "power-controller");
                let Some(Input::Clock { seconds, valid, registers, .. }) =
                    inputs.iter().find(|input| matches!(input, Input::Clock { .. })).copied()
                else {
                    return Err(missing("clock"));
                };
                row.insert("rtc_seconds".into(), integer(i64::from(*seconds)));
                row.insert("rtc_valid".into(), toml::Value::Boolean(*valid));
                if !registers.is_empty() {
                    row.insert("registers".into(), toml::Value::Table(registers.iter().map(|word| {
                        (format!("{:x}", word.register), integer(i64::from(word.value)))
                    }).collect()));
                }
            }
            PeripheralPart::SerialCalibrationRegisters => {
                kind(&mut row, "serial-calibration-registers");
                let high = inputs.iter().find_map(|input| match input {
                    Input::IdleLevel { high, .. } => Some(*high),
                    _ => None,
                }).ok_or_else(|| missing("idle level"))?;
                row.insert("miso_high".into(), toml::Value::Boolean(high));
            }
            PeripheralPart::BisConfigurationRegisters => {
                kind(&mut row, "bis-configuration-registers");
                let Some(Input::BisRevision { revision, samples, block, idle_high, .. }) =
                    inputs.iter().find(|input| matches!(input, Input::BisRevision { .. })).copied()
                else {
                    return Err(missing("BIS revision"));
                };
                let array = |values: Vec<i64>| toml::Value::Array(values.into_iter().map(integer).collect());
                row.insert("revision".into(), array(revision.iter().map(|v| i64::from(*v)).collect()));
                row.insert("samples".into(), array(samples.iter().map(|v| i64::from(*v)).collect()));
                row.insert("idle_high".into(), toml::Value::Boolean(*idle_high));
                row.insert("block".into(), array(block.iter().map(|v| i64::from(*v)).collect()));
            }
            PeripheralPart::Lsm6dsConfigurationRegisters => {
                kind(&mut row, "lsm6ds-configuration-registers");
                let Some(Input::Motion { temperature, angular_rate, acceleration, .. }) =
                    inputs.iter().find(|input| matches!(input, Input::Motion { .. })).copied()
                else {
                    return Err(missing("motion"));
                };
                let array = |values: &[i16; 3]| toml::Value::Array(values.iter().map(|v| integer(i64::from(*v))).collect());
                row.insert("temperature".into(), integer(i64::from(*temperature)));
                let mut motion = toml::Table::new();
                motion.insert("angular_rate".into(), array(angular_rate));
                motion.insert("acceleration".into(), array(acceleration));
                row.insert("motion".into(), toml::Value::Table(motion));
            }
            PeripheralPart::SerialCommandRegisters { bits, mode, read_flag, data_words, miso_high } => {
                kind(&mut row, "serial-command-registers");
                row.insert("bits".into(), integer(i64::from(*bits)));
                row.insert("mode".into(), integer(i64::from(*mode)));
                if let Some(flag) = read_flag { row.insert("read_flag".into(), integer(i64::from(*flag))); }
                if let Some(words) = data_words { row.insert("data_words".into(), integer(i64::from(*words))); }
                row.insert("miso_high".into(), toml::Value::Boolean(*miso_high));
            }
            PeripheralPart::UnidentifiedSerialDevice { miso_high } => {
                kind(&mut row, "generic-sink");
                row.insert("miso_high".into(), toml::Value::Boolean(*miso_high));
            }
            other => return Err(format!("part {}: {other:?} is not a chip-selected serial endpoint", part.id)),
        }
        let mut select = toml::Table::new();
        select.insert("port".into(), integer(i64::from(*port)));
        select.insert("pin".into(), integer(i64::from(*pin)));
        select.insert("active_high".into(), toml::Value::Boolean(*active_level));
        let mut endpoint = toml::Table::new();
        endpoint.insert("select".into(), toml::Value::Table(select));
        endpoint.insert("part".into(), toml::Value::Table(row));
        endpoints.push(toml::Value::Table(endpoint));
    }
    Ok(endpoints)
}

fn block_window(
    block: &fujimod_catalog::reference::SocBlock,
    device: &BlockDevice,
    selection: &ResolvedSelection,
    flash: Option<FlashInput<'_>>,
    flash_image: Option<&fujimod_catalog::firmware::FlashImage>,
    samples: Option<&[u16]>,
    gpio_input: Option<&fujimod_catalog_configuration::application_inputs::ApplicationBlockInput>,
    run: &fujimod_catalog_configuration::RunConfiguration,
    ticks_per_second: u64,
    presented_jedec: Option<u32>,
    part_inputs: &[fujimod_catalog_configuration::application_inputs::ApplicationPartInput],
) -> Result<Option<(&'static str, toml::Table)>, String> {
    let mut options = toml::Table::new();
    let integer = |value: u64| toml::Value::Integer(i64::try_from(value).unwrap_or(i64::MAX));
    let irq = |name: &str| block.interrupts.iter().find(|row| row.output == name).map(|row| row.line);
    let kind = match device {
        BlockDevice::Xp5Timer { focus_capture, .. } => {
            if *focus_capture { "timer-focus-capture" } else { "timer" }
        }
        BlockDevice::Xp5TimerExtended { .. } => "timer-extended",
        BlockDevice::SerialFlashAperture { .. } => {
            options.insert("dat_origin".into(), integer(flash_image.map(|image| image.dat_origin).unwrap_or(0)));
            options.insert("xor".into(), integer(flash_image.map(|image| u64::from(image.xor)).unwrap_or(255)));
            options.insert("erased_tail".into(), integer(flash.and_then(|item| *item.1).map(u64::from).unwrap_or(255)));
            if let Some((part_id, _, seeds)) = flash {
                let part = selection.parts.iter().find(|part| part.id == part_id);
                if let Some(part) = part {
                    if let fujimod_catalog::reference::PartModel::Peripheral { interface } = &part.model {
                        if matches!(interface, fujimod_catalog::assembly::PeripheralPart::SerialNandFlash { .. }) {
                            options.insert("medium".into(), toml::Value::String("serial-nand".into()));
                        }
                    }
                }
                if !seeds.is_empty() {
                    options.insert("seed".into(), toml::Value::Array(seeds.iter().map(|seed| {
                        let mut row = toml::Table::new();
                        row.insert("offset".into(), integer(seed.offset));
                        row.insert("value".into(), integer(u64::from(seed.value)));
                        toml::Value::Table(row)
                    }).collect()));
                }
            }
            "flash-storage"
        }
        BlockDevice::SerialFlashController { .. } => {
            let jedec = presented_jedec.or_else(|| flash.and_then(|(part_id, _, _)| selection.parts.iter().find(|part| part.id == part_id))
                .and_then(|part| match &part.model {
                    fujimod_catalog::reference::PartModel::Peripheral { interface } => match interface {
                        fujimod_catalog::assembly::PeripheralPart::SerialNorFlash { jedec_id, .. }
                        | fujimod_catalog::assembly::PeripheralPart::SerialNandFlash { jedec_id, .. } => *jedec_id,
                        _ => None,
                    },
                    _ => None,
                })
                ).ok_or("flash controller needs a body flash part with a JEDEC identity")?;
            options.insert("jedec_id".into(), integer(u64::from(jedec)));
            "flash-storage-controller"
        }
        BlockDevice::FlashStorageDma { .. } => "flash-storage-dma",
        BlockDevice::Gates { configuration_word, .. } => {
            if let Some(word) = configuration_word {
                options.insert("configuration_word".into(), integer(*word));
            }
            if block.id == "clock-gates" { "clock-gates" } else { "power-gates" }
        }
        BlockDevice::ClockBanks { .. } => "clock-banks",
        BlockDevice::PanelPll { .. } => "panel-pll",
        BlockDevice::GeneralDma { companion_irq_base, a53_irq_base, .. } => {
            if let Some(base) = companion_irq_base { options.insert("companion_irq_base".into(), integer(u64::from(*base))); }
            if let Some(base) = a53_irq_base { options.insert("a53_irq_base".into(), integer(u64::from(*base))); }
            "general-dma"
        }
        BlockDevice::AudioDma { setup_ticks, bytes_per_tick, .. } => {
            options.insert("irq_base".into(), integer(u64::from(irq("irq").unwrap_or(0))));
            options.insert("setup_ticks".into(), integer(*setup_ticks));
            options.insert("bytes_per_tick".into(), integer(*bytes_per_tick));
            "audio-dma"
        }
        BlockDevice::Xp5Gpio { port: None, pad_model, .. } => {
            use fujimod_catalog_configuration::application_inputs::ApplicationBlockInput;
            let table = |pairs: Vec<(&str, toml::Value)>| {
                toml::Value::Table(pairs.into_iter().map(|(key, value)| (key.to_string(), value)).collect())
            };
            if let Some(fujimod_catalog::soc::GpioPadModel::DataLatch) = pad_model {
                options.insert("pad_model".into(), toml::Value::String("data-latch".into()));
            }
            // A pin's interrupt is wired when the body connects something to that pin; the
            // companion core (core4) takes its pins on its own controller.
            let mut irqs = [("input_irqs", Vec::new()), ("companion_input_irqs", Vec::new())];
            for interrupt in &block.interrupts {
                let Some((port, pin)) = interrupt.output.strip_prefix("port").and_then(|rest| rest.split_once("-pin")) else {
                    continue;
                };
                let (Ok(port), Ok(pin)) = (port.parse::<u8>(), pin.parse::<u8>()) else {
                    return Err(format!("block {}: interrupt output {} is not a pin", block.id, interrupt.output));
                };
                let Some(active) = selection.body.connections.iter().flat_map(|connection| &connection.signals).find_map(|signal| match signal {
                    ConnectionSignal::Gpio { port: p, pin: n, active_level } if *p == port && *n == pin => Some(*active_level),
                    _ => None,
                }) else {
                    continue;
                };
                let slot = usize::from(interrupt.core.as_deref() == Some("core4"));
                irqs[slot].1.push(table(vec![
                    ("port", integer(u64::from(port))),
                    ("pin", integer(u64::from(pin))),
                    ("line", integer(u64::from(interrupt.line))),
                    ("active_high", toml::Value::Boolean(active)),
                ]));
            }
            for (key, rows) in irqs {
                if !rows.is_empty() { options.insert(key.into(), toml::Value::Array(rows)); }
            }
            let feedback = gpio_feedback(selection, Some(ticks_per_second))?;
            if !feedback.is_empty() {
                options.insert("feedback".into(), toml::Value::try_from(feedback).map_err(|error| error.to_string())?);
            }
            if let Some(ApplicationBlockInput::Gpio { outputs, pins, registers, .. }) = gpio_input {
                if !outputs.is_empty() {
                    options.insert("initial_outputs".into(), toml::Value::Array(outputs.iter().map(|word| table(vec![
                        ("port", integer(u64::from(word.port))),
                        ("levels", integer(u64::from(word.levels))),
                    ])).collect()));
                }
                if !pins.is_empty() {
                    options.insert("initial_inputs".into(), toml::Value::Array(pins.iter().map(|level| table(vec![
                        ("port", integer(u64::from(level.port))),
                        ("pin", integer(u64::from(level.pin))),
                        ("high", toml::Value::Boolean(level.high)),
                    ])).collect()));
                }
                if !registers.is_empty() {
                    options.insert("initial_registers".into(), toml::Value::Array(registers.iter().map(|word| table(vec![
                        ("port", integer(u64::from(word.port))),
                        ("register", integer(u64::from(word.register))),
                        ("value", integer(u64::from(word.value))),
                    ])).collect()));
                }
            }
            "gpio"
        }
        BlockDevice::Xp5Gpio { port: Some(3), .. } => "gpio-port3",
        BlockDevice::Xp5Gpio { port: Some(port), .. } => return Err(format!("block {}: no XP5 single-port GPIO view of port {port}", block.id)),
        BlockDevice::CableStatus { connected_level, .. } => match block.kind {
            SocBlockKind::UsbController if *connected_level == 1 => "usb-connection-status",
            SocBlockKind::DisplayController => {
                options.insert("connected_level".into(), integer(u64::from(*connected_level)));
                "hdmi-hot-plug-status"
            }
            _ => return Err(format!("block {}: no XP5 cable status model", block.id)),
        },
        BlockDevice::Xp5Adc { .. } => {
            if let Some(samples) = samples {
                options.insert("samples".into(), toml::Value::Array(samples.iter().map(|value| integer(u64::from(*value))).collect()));
            }
            "adc"
        }
        BlockDevice::Temperature { .. } => "soc-temperature",
        BlockDevice::InterprocessorMailbox { .. } => "interprocessor-mailbox",
        BlockDevice::AudioSerial { reset_offset, .. } => {
            match run.audio_signals.iter().find(|signal| signal.block_id == block.id) {
                Some(signal) => {
                    options.insert("sample_rate_hz".into(), integer(signal.sample_rate_hz));
                    options.insert("tone_hz".into(), integer(signal.tone_hz));
                    options.insert("tone_amplitude".into(), toml::Value::Float(f64::from(signal.tone_amplitude_thousandths) / 1000.0));
                    "audio-serial"
                }
                None => {
                    options.insert("trigger_offset".into(), integer(*reset_offset));
                    "reset-handshake"
                }
            }
        }
        BlockDevice::MemoryConfiguration { commit_ticks, .. } => {
            options.insert("commit_ticks".into(), integer(*commit_ticks));
            "memory-configuration"
        }
        BlockDevice::DdrPhy { .. } => { options.insert("registers".into(), stub_registers(block)); "bringup-stub" }
        BlockDevice::SdInterface { .. } => "sd-interface",
        BlockDevice::SdhciUhs2 { clock_ticks, .. } => {
            options.insert("slot".into(), integer(block.base.saturating_sub(0x2000)));
            options.insert("capabilities".into(), integer(0x8_0000_0000));
            options.insert("clock_ticks".into(), integer(*clock_ticks));
            "sdhci-uhs2"
        }
        BlockDevice::DisplayOutputLink { .. } => "display-output-link",
        BlockDevice::Renderer { .. } => "renderer",
        BlockDevice::ImagingPhy { .. } => "imaging-phy",
        BlockDevice::ImagingInput { .. } => {
            if run.idle_status_blocks.contains(&block.id) {
                options.insert("idle_status".into(), toml::Value::Boolean(true));
            }
            "imaging-input"
        }
        BlockDevice::ImagingResize { .. } => {
            options.insert("completion_us".into(), integer(1000));
            "imaging-resize"
        }
        BlockDevice::ImagingPaf { .. } => "imaging-paf",
        BlockDevice::ImagingPreview { focus_statistics, .. } => {
            if *focus_statistics {
                options.insert("focus_filters".into(), toml::Value::Boolean(true));
                options.insert("focus_window".into(), toml::Value::Boolean(true));
            }
            "imaging-preview"
        }
        BlockDevice::ImagingRifpIo { .. } => "imaging-rifp-io",
        BlockDevice::ImagingRifp { .. } => {
            options.insert("completion_us".into(), integer(1000));
            "imaging-rifp"
        }
        BlockDevice::ImagingTnr { handshake, .. } => {
            stage_handshake(&mut options, handshake.as_ref(), irq("irq"));
            "imaging-tnr"
        }
        BlockDevice::ImagingFcnv { handshake, .. } => {
            stage_handshake(&mut options, handshake.as_ref(), irq("irq"));
            "imaging-fcnv"
        }
        BlockDevice::ImagingFilterScaler { handshake, .. } => {
            stage_handshake(&mut options, handshake.as_ref(), irq("irq"));
            "imaging-filter-scaler"
        }
        BlockDevice::ImagingRzl2 { handshake, .. } => {
            stage_handshake(&mut options, handshake.as_ref(), irq("irq"));
            "imaging-rzl2"
        }
        BlockDevice::ImagingResz { handshake, .. } => {
            stage_handshake(&mut options, handshake.as_ref(), irq("irq"));
            "imaging-resz"
        }
        BlockDevice::InputCapture { transmit_requests, .. } => {
            if !transmit_requests.is_empty() {
                options.insert("dma_transmit".into(), toml::Value::Array(transmit_requests.iter().map(|wire| {
                    let mut row = toml::Table::new();
                    row.insert("channel".into(), integer(u64::from(wire.channel)));
                    row.insert("request".into(), integer(u64::from(wire.request)));
                    toml::Value::Table(row)
                }).collect()));
            }
            if let Some(stage) = selection.body.hardware.as_ref().and_then(|hardware| hardware.stabilizer.as_ref())
                .filter(|stage| stage.capture_block_id == block.id)
            {
                let part = selection.parts.iter().find(|part| part.id == stage.part_id)
                    .ok_or_else(|| format!("no stabilizer part {}", stage.part_id))?;
                let fujimod_catalog::reference::PartModel::Peripheral {
                    interface: fujimod_catalog::assembly::PeripheralPart::StabilizerStage { latency_ns, center, rest, lag_shift, drive_gain },
                } = &part.model else {
                    return Err(format!("part {} is not a stabilizer stage", part.id));
                };
                let output = format!("channel{}", stage.capture_channel);
                let line = block.interrupts.iter()
                    .find(|row| row.output == output && row.core.as_deref() == Some("core4"))
                    .map(|row| row.line)
                    .ok_or_else(|| format!("block {}: capture {output} has no companion interrupt", block.id))?;
                let array = |values: Vec<i64>| toml::Value::Array(values.into_iter().map(toml::Value::Integer).collect());
                let mut sequencer = toml::Table::new();
                sequencer.insert("channel".into(), integer(u64::from(stage.capture_channel)));
                sequencer.insert("companion_irq_line".into(), integer(u64::from(line)));
                sequencer.insert("latency_ns".into(), integer(*latency_ns));
                sequencer.insert("center".into(), array(center.iter().map(|v| i64::from(*v)).collect()));
                sequencer.insert("rest".into(), array(rest.iter().map(|v| i64::from(*v)).collect()));
                sequencer.insert("lag_shift".into(), integer(u64::from(*lag_shift)));
                sequencer.insert("drive_channels".into(), array(stage.drive_channels.iter().map(|v| i64::from(*v)).collect()));
                sequencer.insert("drive_gain".into(), array(drive_gain.iter().map(|v| i64::from(*v)).collect()));
                options.insert("sequencer".into(), toml::Value::Table(sequencer));
            }
            "input-capture"
        }
        BlockDevice::Pl011 { .. } => {
            options.insert("irq".into(), integer(u64::from(irq("irq").unwrap_or(0))));
            let mut rl78 = toml::Table::new();
            rl78.insert("flash_bytes".into(), integer(49152));
            options.insert("rl78".into(), toml::Value::Table(rl78));
            "pl011"
        }
        BlockDevice::Pl022 { source_hz, .. } if panel_select(selection, block)?.is_some() => {
            let (port, pin) = panel_select(selection, block)?.expect("checked above");
            options.insert("select_port".into(), integer(u64::from(port)));
            options.insert("select_pin".into(), integer(u64::from(pin)));
            options.insert("source_hz".into(), integer(*source_hz));
            "panel-spi"
        }
        BlockDevice::Pl022 { .. } if port_device(selection, block)?.is_some() => {
            port_device(selection, block)?.expect("checked above")
        }
        BlockDevice::Pl022 { source_hz, .. } if run.polled_serial_blocks.contains(&block.id) => {
            options.insert("source_hz".into(), integer(*source_hz));
            let endpoints = serial_endpoints(selection, block, part_inputs)?;
            if !endpoints.is_empty() { options.insert("endpoints".into(), toml::Value::Array(endpoints)); }
            "spi-controller"
        }
        BlockDevice::Pl022 { source_hz, ticks_per_second, receive_request, transmit_request, companion_irq_line, .. } => {
            options.insert("source_hz".into(), integer(*source_hz));
            options.insert("ticks_per_second".into(), integer(*ticks_per_second));
            if let Some(value) = receive_request { options.insert("receive_request".into(), integer(u64::from(*value))); }
            if let Some(value) = transmit_request { options.insert("transmit_request".into(), integer(u64::from(*value))); }
            if let Some(value) = companion_irq_line { options.insert("companion_irq_line".into(), integer(u64::from(*value))); }
            if let Some(line) = irq("irq") { options.insert("gic_line".into(), integer(u64::from(line))); }
            if run.unobserved_interrupt_blocks.contains(&block.id) {
                options.insert("companion_irq_unconnected".into(), toml::Value::Boolean(true));
            }
            let endpoints = serial_endpoints(selection, block, part_inputs)?;
            let mut wire = match lens_wire(selection, block)? {
                Some(wire) => wire,
                None => dedicated_wire(selection, block)?.unwrap_or_default(),
            };
            if !endpoints.is_empty() {
                if !wire.is_empty() {
                    return Err(format!("block {}: a lens mount shares its port with chip-selected endpoints", block.id));
                }
                wire.insert("kind".into(), toml::Value::String("gpio-selected".into()));
                wire.insert("endpoints".into(), toml::Value::Array(endpoints));
            }
            options.insert("wire".into(), toml::Value::Table(wire));
            "pl022"
        }
        BlockDevice::Completion { fixed_words, starts, completion, ack, irq_mask, delay_ticks, .. } => {
            if !fixed_words.is_empty() {
                options.insert("registers".into(), toml::Value::Array(fixed_words.iter().map(|word| {
                    let mut row = toml::Table::new();
                    row.insert("offset".into(), integer(word.offset));
                    row.insert("mode".into(), toml::Value::String("constant".into()));
                    row.insert("value".into(), integer(u64::from(word.value)));
                    toml::Value::Table(row)
                }).collect()));
            }
            handshake_options(&mut options, starts, completion, ack.as_ref(), irq_mask.as_ref());
            options.insert("delay_us".into(), integer(delay_ticks / 1000));
            if let Some(line) = irq("irq") { options.insert("irq".into(), integer(u64::from(line))); }
            "completion-stub"
        }
        BlockDevice::VideoCodec { .. } => {
            options.insert("irq".into(), integer(u64::from(irq("irq").ok_or_else(|| format!("block {} declares no interrupt", block.id))?)));
            "xp5-codec"
        }
        BlockDevice::ImagingBuffer { .. } => "imaging-buffer",
        BlockDevice::Xp5ImagingStatistics { .. } => "imaging-statistics",
        // A loader start is the direct-flash boot; a ROM start replaces the straps with its own.
        BlockDevice::BootControl { direct_flash_straps, .. } => {
            options.insert("source_straps".into(), integer(u64::from(*direct_flash_straps)));
            options.insert("initial_control".into(), integer(0));
            options.insert("initial_primary_control".into(), integer(0));
            "boot-control"
        }
        BlockDevice::ImagingDsp { handshake, .. } => {
            stage_handshake(&mut options, handshake.as_ref(), irq("irq"));
            "completion-stub"
        }
        BlockDevice::RegisterBlock { idle_bits, fixed_words, reads_back, .. } => {
            if !idle_bits.is_empty() || !fixed_words.is_empty() {
                options.insert("registers".into(), toml::Value::Array(fixed_words.iter().map(|word| {
                    let mut row = toml::Table::new();
                    row.insert("offset".into(), integer(word.offset));
                    row.insert("mode".into(), toml::Value::String("constant".into()));
                    row.insert("value".into(), integer(u64::from(word.value)));
                    toml::Value::Table(row)
                }).collect()));
            }
            if block.id == "dma-routes" { "dma-request-routing" }
            else if block.id == "dma-address-high" { "dma-address-high" }
            else if *reads_back || block.registers.iter().any(|register| register.reads.is_some()) {
                options.insert("registers".into(), stub_registers(block));
                "bringup-stub"
            } else { "write-only-registers" }
        }
        BlockDevice::SdHost { transfer_ticks, clock_ticks, dma_windows, address_high, .. } => {
            let mut controller = toml::Table::new();
            controller.insert("base".into(), integer(block.base));
            controller.insert("protocol".into(), toml::Value::String("sdhc".into()));
            controller.insert("transfer_ticks".into(), integer(*transfer_ticks));
            controller.insert("clock_ticks".into(), integer(*clock_ticks));
            controller.insert("rca".into(), integer(1));
            if !dma_windows.is_empty() {
                let (low, banks): (Vec<&fujimod_catalog::soc::SdDmaWindow>, Vec<_>) = dma_windows.iter().partition(|window| window.base == 0);
                let [low] = low.as_slice() else {
                    return Err(format!("block {}: SD DMA needs one range from address 0", block.id));
                };
                controller.insert("ram_bytes".into(), integer(low.size));
                if !banks.is_empty() {
                    controller.insert("ram_banks".into(), toml::Value::Array(banks.iter().map(|bank| {
                        toml::Value::Array(vec![integer(bank.base), integer(bank.size)])
                    }).collect()));
                }
                controller.insert("dram_alias".into(), toml::Value::Boolean(false));
            }
            if let Some(address) = address_high { controller.insert("address_high".into(), integer(*address)); }
            if let Some(steps) = run.sd_clock_steps.iter().find(|steps| steps.block_id == block.id) {
                let mut dividers = toml::Table::new();
                dividers.insert("interface".into(), integer(u64::from(steps.interface)));
                dividers.insert("default_speed".into(), integer(u64::from(steps.default_speed)));
                dividers.insert("high_speed".into(), integer(u64::from(steps.high_speed)));
                controller.insert("speed_dividers".into(), toml::Value::Table(dividers));
            }
            options.insert("controller".into(), toml::Value::Table(controller));
            "sdhci"
        }
        BlockDevice::Decompressor { .. } => "lz",
        BlockDevice::Movdc { .. } => {
            "movdc"
        }
        BlockDevice::SensorCommand { .. } => {
            options.insert("command_latency_us".into(), integer(1));
            let sensors: Vec<_> = selection.parts.iter().filter_map(|part| match &part.model {
                fujimod_catalog::reference::PartModel::Peripheral {
                    interface: fujimod_catalog::assembly::PeripheralPart::TimedReadoutSensor { fast_readout, line_ns },
                } => Some((*fast_readout, *line_ns)),
                _ => None,
            }).collect();
            match sensors.as_slice() {
                [] => {}
                [(readout, line_ns)] => {
                    options.insert("frame_timing_readout".into(), integer(u64::from(*readout)));
                    options.insert("frame_timing_line_ns".into(), integer(*line_ns));
                }
                _ => return Err(format!("body {} installs more than one timed-readout sensor", selection.body.id)),
            }
            "imaging-sensor"
        }
        BlockDevice::ImagingReceiver { .. } => {
            "imaging-receiver"
        }
        BlockDevice::Display { .. } => "display",
        BlockDevice::IoGroups { .. } => "pin-control",
        BlockDevice::Usb { .. } => {
            options.insert("interrupt".into(), integer(u64::from(irq("irq").unwrap_or(0))));
            options.insert("speed".into(), integer(0));
            options.insert("command_ticks".into(), integer(1000));
            options.insert("transaction_ticks".into(), integer(125000));
            "usb-device-controller"
        }
        BlockDevice::UsbPhy { .. } => "usb-phy",
        BlockDevice::JpegCodec { run_ticks, direction, .. } => {
            let line = irq("irq").ok_or_else(|| format!("block {}: JPEG codec has no interrupt", block.id))?;
            options.insert("irq".into(), integer(u64::from(line)));
            options.insert("run_ticks".into(), integer(*run_ticks));
            match direction {
                Some(fujimod_catalog::soc::JpegDirection::Decode) => "jpeg-decoder",
                Some(fujimod_catalog::soc::JpegDirection::Encode) => "jpeg-encoder",
                None => return Err(format!("block {}: XP5 JPEG blocks declare a direction", block.id)),
            }
        }
        BlockDevice::I2c { fifo_depth, clock_hz, tick_hz, .. } => {
            let _ = (fifo_depth, clock_hz, tick_hz);
            if let Some(line) = irq("interrupt") { options.insert("interrupt".into(), integer(u64::from(line))); }
            let mut targets = Vec::new();
            for wire in selection.body.wiring.iter().filter(|wire| wire.source_id == block.id) {
                let address = wire.address.ok_or_else(|| format!("wire {} -> {} has no bus address", wire.source_id, wire.target_id))?;
                let part = selection.parts.iter().find(|part| part.id == wire.target_id)
                    .ok_or_else(|| format!("wire {} -> {}: no such part", wire.source_id, wire.target_id))?;
                let fujimod_catalog::reference::PartModel::Peripheral { interface } = &part.model else {
                    return Err(format!("part {} is not a control-bus peripheral", part.id));
                };
                let interface = toml::Value::try_from(interface).map_err(|error| error.to_string())?;
                let mut row = toml::Table::new();
                row.insert("address".into(), integer(u64::from(address)));
                row.insert("part".into(), interface);
                targets.push(toml::Value::Table(row));
            }
            if !targets.is_empty() { options.insert("targets".into(), toml::Value::Array(targets)); }
            if let Some(time) = run.bus_byte_times.iter().find(|time| time.block_id == block.id) {
                options.insert("byte_ns".into(), integer(time.ticks * 1_000_000_000 / ticks_per_second));
            }
            "i2c-controller"
        }
        BlockDevice::ResetGates { .. } => "imaging-reset",
        BlockDevice::GicV2 { .. } | BlockDevice::GicV3 { .. } => return Ok(None),
        BlockDevice::UnmodeledBusDrain { .. } => return Ok(None),
        BlockDevice::DramController { .. } => return Ok(None),
        other => return Err(format!("block {} has no XP5 emulator window for {other:?}", block.id)),
    };
    Ok(Some((kind, options)))
}

/// The body's GPIO feedback wires: each drives an input pad from the output
/// latch bit of the same pin number (register 0, the port's drive field).
fn gpio_feedback(selection: &ResolvedSelection, ticks_per_second: Option<u64>) -> Result<Vec<Feedback>, String> {
    let body = &selection.body;
    let Some(hardware) = body.hardware.as_ref() else {
        return Ok(vec![]);
    };
    let pin = |id: &str| -> Result<(usize, usize, bool), String> {
        let connection = body
            .connections
            .iter()
            .find(|connection| connection.id == id)
            .ok_or_else(|| format!("body {} feedback refers to missing connection {id}", body.id))?;
        match connection.signals.as_slice() {
            [ConnectionSignal::Gpio { port, pin, active_level }] => {
                Ok((usize::from(*port), usize::from(*pin), *active_level))
            }
            _ => Err(format!(
                "body {} feedback connection {id} must be exactly one GPIO pin",
                body.id
            )),
        }
    };
    hardware
        .gpio_feedback
        .iter()
        .map(|wire| {
            let (port, bit, output_level) = pin(&wire.output_connection_id)?;
            let (input_port, input_pin, input_level) = pin(&wire.input_connection_id)?;
            if output_level != input_level {
                return Err(format!(
                    "body {} inverting GPIO feedback {} -> {} is unmodeled",
                    body.id, wire.output_connection_id, wire.input_connection_id
                ));
            }
            let rise_delay = match (wire.rise_ns, ticks_per_second) {
                (None, _) => 0,
                (Some(ns), Some(rate)) => u64::try_from(u128::from(ns) * u128::from(rate) / 1_000_000_000)
                    .map_err(|_| format!("body {} feedback rise time overflows", body.id))?,
                (Some(_), None) => return Err(format!("body {} delayed GPIO feedback needs a run tick rate", body.id)),
            };
            Ok(Feedback { port, register: 0, bit, input_port, input_pin, rise_delay, latch_high: wire.latch_high })
        })
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
/// The declared key model is supplied by the caller so its electrical
/// layout and timing remain application inputs rather than emulator policy.
/// A command dial's two quadrature pads, driven by an encoder device.
struct DialPads(fujimod_emulator_devices::xp5::pins::SharedPins);
impl fujimod_emulator_devices::parts::pins::Pins for DialPads {
    fn output_level(
        &self,
        _: fujimod_emulator_devices::parts::pins::Pin,
    ) -> Result<Option<bool>, String> {
        Ok(None)
    }
    fn input(
        &mut self,
        _: &mut fujimod_emulator_devices::Cx<'_>,
        pin: fujimod_emulator_devices::parts::pins::Pin,
        level: bool,
    ) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|_| "XP5 GPIO pins poisoned")?
            .set_input(usize::from(pin.port), usize::from(pin.pin), level);
        Ok(())
    }
}

/// Session node names of the command dials' encoders.
pub const REAR_DIAL_NODE: &str = "rear-dial";
pub const FRONT_DIAL_NODE: &str = "front-dial";

/// The encoders of a body's quadrature command dials.
#[derive(Default)]
pub struct Dials {
    pub rear: Option<fujimod_emulator_parts_controls::encoder::Options>,
    pub front: Option<fujimod_emulator_parts_controls::encoder::Options>,
}

pub fn assemble_devices(
    windows: Vec<DeviceWindow>,
    dials: Dials,
    mut resources: xp5_board::Resources,
    mode: Mode,
) -> Result<DeviceAssembly, String> {
    let contacts = resources.sd.iter().map(|(base, media)| (format!("{base:#x}"), media.contacts.clone())).collect();
    let mut graph = DeviceGraph::new();
    let mut unknown = |window: &fujimod_emulator_devices::graph::DeviceWindow| {
        Err(format!(
            "unknown XP5 device {} at {:#x}",
            window.kind, window.base
        ))
    };
    xp5_board::register_all(&mut graph, &windows, &mut resources, &mut unknown)?;
    xp5_board::sd::require_hosts(&resources.sd)?;
    if !resources.displays.is_empty() {
        return Err("XP5 profile has unused display configurations".into());
    }
    let mut encoders = Vec::new();
    for (name, options) in [(REAR_DIAL_NODE, dials.rear), (FRONT_DIAL_NODE, dials.front)] {
        let Some(options) = options else { continue };
        let encoder = fujimod_emulator_parts_controls::encoder::Encoder::new(
            options,
            fujimod_emulator_parts_controls::encoder::Wiring {
                outputs: fujimod_emulator_parts_controls::encoder::OutputWiring::Gpio(Box::new(
                    DialPads(resources.pins.clone()),
                )),
                counter: None,
            },
        )?;
        encoders.push((name, graph.attach(Box::new(encoder))));
    }
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
        causes.extend(encoders.iter().map(|(_, node)| *node));
        graph.depend_on(device, causes)?;
    }
    for (name, node) in encoders {
        nodes.insert(name.into(), node);
    }
    let usb = windows
        .iter()
        .find(|window| window.kind == "usb-device-controller")
        .map(|controller| -> Result<_, String> {
            let at = |kind: &str| {
                windows
                    .iter()
                    .filter(|window| window.kind == kind)
                    .map(|window| graph.at(window.base))
                    .collect::<Result<Vec<_>, _>>()
            };
            Ok(crate::session::UsbWiring {
                controller: graph.at(controller.base)?,
                status: at("usb-connection-status")?.first().copied(),
                serial: at("spi-controller")?,
            })
        })
        .transpose()?;
    let bus = Bus::from_graph(mode, graph)?;
    Ok(DeviceAssembly {
        contacts,
        bus,
        inputs: crate::session::Inputs::new(resources.pins, gpio_devices, usb),
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
    /// The encoders of the body's quadrature command dials.
    pub dials: Dials,
    pub resources: xp5_board::Resources,
    pub engine: RunEngine,
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
        options.dials,
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
        power_cycle: None,
        window_action: None,
        recording_frames: Default::default(),
    };
    Ok((session, assembly.inputs))
}

#[cfg(test)]
mod bus_drain_tests {
    use super::windows;

    fn selection(body: &str, firmware: &str) -> fujimod_catalog::ResolvedSelection {
        fujimod_catalog::ReferenceCatalog::bundled()
            .unwrap()
            .resolve(body, firmware)
            .unwrap()
    }

    fn drains(selection: &fujimod_catalog::ResolvedSelection) -> Vec<u64> {
        windows(selection, "xp5")
            .unwrap()
            .iter()
            .filter(|window| window.kind == "unmodeled-bus-drain")
            .map(|window| window.base)
            .collect()
    }

    #[test]
    fn each_body_gets_its_own_drain_beside_the_soc_ones() {
        let x100vi = drains(&selection("x100vi", "x100vi-131"));
        let xt5 = drains(&selection("xt5", "xt5-431"));
        assert!(x100vi.contains(&0x6800_0000) && !x100vi.contains(&0x6000_0000));
        assert!(xt5.contains(&0x6000_0000) && !xt5.contains(&0x6800_0000));
        for drains in [&x100vi, &xt5] {
            assert!(drains.contains(&0x8000_0000) && drains.contains(&0x1_c000_0000));
        }
    }

    #[test]
    fn a_body_without_a_declared_drain_is_refused() {
        let mut selection = selection("xt5", "xt5-431");
        selection.body.hardware.as_mut().unwrap().bus_drain.clear();
        let error = windows(&selection, "xp5").err().unwrap();
        assert!(error.contains("no bus-drain address"), "{error}");
    }
}

#[cfg(test)]
mod gpio_feedback_tests {
    use super::windows;
    use fujimod_catalog::reference::ConnectionSignal;
    use serde_json::json;

    fn selection(body: &str, firmware: &str) -> fujimod_catalog::ResolvedSelection {
        fujimod_catalog::ReferenceCatalog::bundled()
            .unwrap()
            .resolve(body, firmware)
            .unwrap()
    }

    fn feedback(selection: &fujimod_catalog::ResolvedSelection) -> serde_json::Value {
        let windows = windows(selection, "xp5").unwrap();
        let gpio = windows.iter().find(|window| window.kind == "gpio").unwrap();
        serde_json::to_value(gpio.options.get("feedback")).unwrap()
    }

    #[test]
    fn declared_bodies_feed_the_sensor_power_enable_back_to_its_pad() {
        let wire = json!([{"port": 4, "register": 0, "bit": 4, "input_port": 4, "input_pin": 4}]);
        assert_eq!(feedback(&selection("x100vi", "x100vi-131")), wire);
        assert_eq!(feedback(&selection("xt5", "xt5-431")), wire);
    }

    #[test]
    fn a_body_without_feedback_leaves_the_gpio_window_unchanged() {
        let mut selection = selection("xt5", "xt5-431");
        selection.body.hardware.as_mut().unwrap().gpio_feedback.clear();
        assert_eq!(feedback(&selection), serde_json::Value::Null);
    }

    #[test]
    fn inverting_and_non_gpio_feedback_wires_are_refused() {
        let mut inverted = selection("xt5", "xt5-431");
        let input = inverted.body.hardware.as_ref().unwrap().gpio_feedback[0]
            .input_connection_id
            .clone();
        let connection = inverted.body.connections.iter_mut().find(|c| c.id == input).unwrap();
        connection.signals = vec![ConnectionSignal::Gpio { port: 4, pin: 4, active_level: false }];
        let error = windows(&inverted, "xp5").err().unwrap();
        assert!(error.contains("inverting GPIO feedback"), "{error}");

        let mut analog = selection("xt5", "xt5-431");
        let connection = analog.body.connections.iter_mut().find(|c| c.id == input).unwrap();
        connection.signals = vec![ConnectionSignal::Adc { channel: 0, threshold: None }];
        let error = windows(&analog, "xp5").err().unwrap();
        assert!(error.contains("exactly one GPIO pin"), "{error}");
    }
}
