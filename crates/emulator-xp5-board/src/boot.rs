//! XP5 device topology resolved from the selected SoC profile.
//!
//! The profile owns device addresses and options. Run inputs own the external
//! values supplied to those devices; neither the caller nor a board runner
//! needs a second copy of the register-window map.

use crate::board as xp5_board;
use fujimod_catalog::{
    firmware::{FirmwareProfile, GicSecurityStates},
    reference::{AProfileState, ConnectionSignal, ProcessorModel, ResolvedSelection, SocBlockKind},
    soc::BlockDevice,
};
use fujimod_catalog_configuration::RunEngine;
use fujimod_catalog_configuration::application::ApplicationRunScene;
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator::board::aprofile::generic_timer;
use fujimod_emulator::board::{
    InterruptMap, aprofile,
    bus::{Bus, Mode},
    factory,
};
use fujimod_emulator_cpu::{Register, arm64};
use fujimod_emulator_devices::{
    gic::SecurityStates,
    graph::{DeviceGraph, DeviceWindow},
    parts::gpio::Feedback,
};
use fujimod_emulator_ip_arm::gicv3::Gic;
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
            Some(AProfileState::Armv8a { affinity, .. }) => Some((processor.id.clone(), *affinity)),
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
            cpu.validate()?;
            let index = usize::try_from(cpu.id).map_err(|_| "CPU id exceeds host address space")?;
            let processor = soc.processors.get(index).ok_or_else(|| {
                format!("firmware CPU {} is absent from the selected SoC", cpu.id)
            })?;
            let affinity = affinities
                .get(&processor.id)
                .ok_or_else(|| format!("processor {} has no MPIDR affinity", processor.id))?;
            let pstate = cpu
                .entry
                .as_ref()
                .map(|entry| entry.pstate)
                .or(cpu.cpsr)
                .ok_or("firmware CPU declares neither an entry descriptor nor a legacy handoff")?;
            let mut registers = vec![(Register::Pc, cpu.pc), (Register::Pstate, pstate)];
            if let Some(entry) = &cpu.entry {
                for (name, value) in &entry.regs {
                    insert_start_register(&mut registers, name, *value)?;
                }
            } else if let Some(regs) = &cpu.regs {
                for (name, value) in [("x19", regs.x19), ("x20", regs.x20), ("x21", regs.x21)] {
                    insert_start_register(&mut registers, name, value)?;
                }
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
            // Profile ICC values below are written in the reset Security
            // state (EL3, SCR_EL3.NS clear), so they address Secure banks.
            let security_states = match profile.gic_security_states {
                None | Some(GicSecurityStates::One) => SecurityStates::One,
                Some(GicSecurityStates::Two) => SecurityStates::Two,
            };
            let mut gic = Gic::new(affinity_values.clone(), *lines as usize, security_states)?;
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
        Some(fujimod_emulator::board::InterruptRegisters::GicV3 { distributor, .. }) => {
            *distributor
        }
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
                // The input engine's frame-start and frame-end events after each completed run.
                "imaging-input" => {
                    if frame.luma.is_some() {
                        return Err(format!(
                            "XP5 scene luma is unsupported for input engine {block_id}"
                        ));
                    }
                    let active = active.clone().ok_or_else(|| {
                        format!("XP5 scene for input engine {block_id} needs an active interval")
                    })?;
                    window
                        .options
                        .insert("frame_period_us".into(), period.clone());
                    window.options.insert("frame_active_us".into(), active);
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

/// One RAM region a run declares (an XP5 boot manifest's `[[ram]]`), and whether it is DRAM
/// rather than companion SRAM, PHY register space or boot SRAM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunRam {
    pub base: u64,
    pub size: u64,
    pub dram: bool,
}

/// A run's declared RAM, checked: regions are nonempty, and DRAM regions do not overlap.
/// Adjacent DRAM regions form one span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunMemory {
    ram: Vec<(u64, u64)>,
    dram: Vec<(u64, u64)>,
}

impl RunMemory {
    pub fn new(regions: impl IntoIterator<Item = RunRam>) -> Result<Self, String> {
        let mut ram = Vec::new();
        let mut dram: Vec<(u64, u64)> = Vec::new();
        for region in regions {
            let end = region
                .base
                .checked_add(region.size)
                .filter(|_| region.size != 0)
                .ok_or_else(|| {
                    format!(
                        "RAM region {:#x}+{:#x} is empty or overflows",
                        region.base, region.size
                    )
                })?;
            ram.push((region.base, end));
            if region.dram {
                dram.push((region.base, end));
            }
        }
        dram.sort_unstable();
        if let Some(pair) = dram.windows(2).find(|pair| pair[0].1 > pair[1].0) {
            return Err(format!(
                "DRAM regions {:#x}..{:#x} and {:#x}..{:#x} overlap",
                pair[0].0, pair[0].1, pair[1].0, pair[1].1
            ));
        }
        let mut spans: Vec<(u64, u64)> = Vec::new();
        for (base, end) in dram {
            match spans.last_mut() {
                Some(last) if last.1 == base => last.1 = end,
                _ => spans.push((base, end)),
            }
        }
        Ok(Self { ram, dram: spans })
    }

    /// The DRAM spans as `(base, end)`, adjacent regions joined, in address order.
    pub fn dram(&self) -> &[(u64, u64)] {
        &self.dram
    }

    /// The exclusive end of the DRAM span that starts at `base`.
    pub fn dram_end_from(&self, base: u64) -> Result<u64, String> {
        self.dram
            .iter()
            .find(|span| span.0 == base)
            .map(|span| span.1)
            .ok_or_else(|| format!("the run declares no DRAM from {base:#x}"))
    }

    fn within(ranges: &[(u64, u64)], base: u64, size: u64) -> bool {
        base.checked_add(size).is_some_and(|end| {
            ranges
                .iter()
                .any(|&(start, stop)| start <= base && end <= stop)
        })
    }

    /// Whether `base..base+size` lies in declared DRAM.
    pub fn in_dram(&self, base: u64, size: u64) -> bool {
        Self::within(&self.dram, base, size)
    }

    /// Whether `base..base+size` lies in one declared RAM region.
    pub fn in_ram(&self, base: u64, size: u64) -> bool {
        Self::within(&self.ram, base, size)
    }
}

/// The run's XP5 device windows, given the RAM the run declares.
pub fn device_windows(
    resolved: &fujimod_catalog_resolution::ResolvedConfiguration,
    memory: &RunMemory,
) -> Result<Vec<DeviceWindow>, String> {
    windows_from_blocks(resolved, memory)
}

/// DRAM from 0 to the catalog's DRAM address-space limit, for device tests that build windows
/// without a boot manifest.
#[cfg(test)]
pub(crate) fn catalog_dram(
    resolved: &fujimod_catalog_resolution::ResolvedConfiguration,
) -> RunMemory {
    RunMemory::new([RunRam {
        base: 0,
        size: resolved.dram_address_space_limit_bytes().unwrap(),
        dram: true,
    }])
    .unwrap()
}

/// The X-T5 4.31 windows of `kinds`, for device tests.
#[cfg(test)]
pub(crate) fn xt5_windows(kinds: &[&str]) -> Vec<DeviceWindow> {
    let resolved = fujimod_catalog_resolution::ResolvedConfiguration::bundled("xt5-431").unwrap();
    let mut windows = device_windows(&resolved, &catalog_dram(&resolved)).unwrap();
    windows.retain(|window| kinds.contains(&window.kind.as_str()));
    windows
}

/// The XP5 device graph built from the catalog's block records.
///
/// Firmware, body and run facts are joined by block identity: a block keeps the
/// hardware window and model, the body supplies wiring and parts, and the run
/// supplies ADC samples, GPIO levels and flash contents.
pub fn windows_from_blocks(
    resolved: &fujimod_catalog_resolution::ResolvedConfiguration,
    memory: &RunMemory,
) -> Result<Vec<DeviceWindow>, String> {
    use fujimod_catalog_configuration::application::ApplicationBlockInput;

    let selection = &resolved.reference;
    let inputs = resolved
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
        .inputs
        .as_ref()
        .and_then(|inputs| match inputs {
            fujimod_catalog_configuration::RunInputs::Xp5(inputs) => Some(inputs.parts.as_slice()),
            _ => None,
        })
        .unwrap_or_default();
    let presented_jedec = inputs.iter().find_map(|input| match input {
        ApplicationBlockInput::Flash {
            presented_jedec_id, ..
        } => *presented_jedec_id,
        _ => None,
    });
    let samples = inputs.iter().find_map(|input| match input {
        ApplicationBlockInput::Adc { samples, .. } => Some(samples.clone()),
        _ => None,
    });
    let levels = inputs
        .iter()
        .find(|input| matches!(input, ApplicationBlockInput::Gpio { .. }));
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
    for id in emulation.retained_word_blocks.iter().chain(
        emulation
            .completion_latencies
            .iter()
            .map(|latency| &latency.block_id),
    ) {
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
            block,
            device,
            selection,
            flash_input,
            flash_image,
            samples.as_deref(),
            levels,
            &resolved.configuration,
            ticks_per_second,
            presented_jedec,
            part_inputs,
            memory,
        )? {
            let mut options = options;
            if !options.contains_key("registers")
                && block
                    .registers
                    .iter()
                    .any(|register| register.reads.is_some())
            {
                options.insert("registers".into(), stub_registers(block));
            }
            if emulation.retained_word_blocks.contains(&block.id) {
                options.insert("retain_words".into(), toml::Value::Boolean(true));
            }
            if let Some(latency) = emulation
                .completion_latencies
                .iter()
                .find(|latency| latency.block_id == block.id)
            {
                let option = match kind {
                    "completion-stub"
                    | "imaging-tnr"
                    | "imaging-fcnv"
                    | "imaging-filter-scaler"
                    | "imaging-rzl2"
                    | "imaging-resz" => "delay_us",
                    "imaging-rifp-io" => "queue_completion_us",
                    "imaging-buffer" => "completion_us",
                    "xp5-codec" => "run_ticks",
                    _ => {
                        return Err(format!(
                            "block {}: {kind} takes no completion latency",
                            block.id
                        ));
                    }
                };
                let value = if option == "run_ticks" {
                    u128::from(latency.ticks)
                } else {
                    let micros = u128::from(latency.ticks) * 1_000_000;
                    if micros % u128::from(ticks_per_second) != 0 {
                        return Err(format!(
                            "block {}: latency is not a whole number of microseconds",
                            block.id
                        ));
                    }
                    micros / u128::from(ticks_per_second)
                };
                options.insert(
                    option.into(),
                    toml::Value::Integer(
                        i64::try_from(value)
                            .map_err(|_| format!("block {}: latency overflows", block.id))?,
                    ),
                );
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
    // in the run's declared DRAM is RAM instead.
    let ports = selection
        .soc
        .blocks
        .iter()
        .filter(|block| matches!(block.device, Some(BlockDevice::UnmodeledBusDrain { .. })))
        .map(|block| block.base)
        .chain(drains.iter().map(|drain| drain.address))
        .filter(|&address| !memory.in_dram(address, 4))
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

type FlashInput<'a> = (
    &'a str,
    &'a Option<u8>,
    &'a Vec<fujimod_catalog_configuration::application::FlashSeed>,
);

/// The body's hardware facts beyond its parts and wiring.
fn body_hardware(selection: &ResolvedSelection) -> Option<&fujimod_catalog::assembly::BodyHardware> {
    selection.body.hardware.as_ref()
}

/// The bring-up register file answers of a block's registers that do not read back their last write.
fn stub_registers(block: &fujimod_catalog::reference::SocBlock) -> toml::Value {
    use fujimod_catalog::ip::RegisterReads;
    let integer = |value: u64| toml::Value::Integer(value as i64);
    toml::Value::Array(
        block
            .registers
            .iter()
            .filter_map(|register| {
                let (mode, value) = match register.reads? {
                    RegisterReads::Fixed { value } => ("constant", value),
                    RegisterReads::Alternates { value } => ("toggle", value),
                    RegisterReads::SelfClearing => ("command", 0),
                    RegisterReads::AfterWrite { value } => ("after_write", value),
                };
                let mut row = toml::Table::new();
                row.insert("offset".into(), integer(register.offset));
                row.insert("mode".into(), toml::Value::String(mode.into()));
                if !matches!(register.reads, Some(RegisterReads::SelfClearing)) {
                    row.insert("value".into(), integer(value));
                }
                Some(toml::Value::Table(row))
            })
            .collect(),
    )
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
        options.insert(
            "starts".into(),
            toml::Value::Array(
                starts
                    .iter()
                    .map(|start| {
                        let mut row = toml::Table::new();
                        row.insert("offset".into(), integer(start.offset));
                        row.insert("mask".into(), integer(u64::from(start.mask)));
                        toml::Value::Table(row)
                    })
                    .collect(),
            ),
        );
    }
    if !completion.is_empty() {
        options.insert(
            "completion".into(),
            toml::Value::Array(
                completion
                    .iter()
                    .map(|write| {
                        let mut row = toml::Table::new();
                        row.insert("offset".into(), integer(write.offset));
                        if let Some(set) = write.set {
                            row.insert("set".into(), integer(u64::from(set)));
                        }
                        if let Some(clear) = write.clear {
                            row.insert("clear".into(), integer(u64::from(clear)));
                        }
                        toml::Value::Table(row)
                    })
                    .collect(),
            ),
        );
    }
    if let Some(ack) = ack {
        let mut row = toml::Table::new();
        row.insert("offset".into(), integer(ack.offset));
        row.insert("mode".into(), toml::Value::String("w1c".into()));
        if let Some(offset) = ack.status_offset {
            row.insert("status_offset".into(), integer(offset));
        }
        if let Some(bits) = ack.bits {
            row.insert("bits".into(), integer(u64::from(bits)));
        }
        options.insert("ack".into(), toml::Value::Table(row));
    }
    if let Some(mask) = irq_mask {
        options.insert("irq_mask_offset".into(), integer(mask.offset));
        options.insert("irq_mask_bit".into(), integer(u64::from(mask.bit)));
        options.insert(
            "irq_mask_set_masks".into(),
            toml::Value::Boolean(mask.set_masks),
        );
    }
}

/// A stage's handshake and interrupt line as completion-stub options.
fn stage_handshake(
    options: &mut toml::Table,
    handshake: Option<&fujimod_catalog::soc::CompletionHandshake>,
    irq: Option<u32>,
) {
    if let Some(handshake) = handshake {
        handshake_options(
            options,
            &handshake.starts,
            &handshake.completion,
            handshake.ack.as_ref(),
            handshake.irq_mask.as_ref(),
        );
    }
    if let Some(line) = irq {
        options.insert("irq".into(), toml::Value::Integer(i64::from(line)));
    }
}

/// The device a serial port is emulated as when its one wired part is a whole-port output.
/// The chip-select pad of a display panel the body attaches to `block`.
fn panel_select(
    selection: &ResolvedSelection,
    block: &fujimod_catalog::reference::SocBlock,
) -> Result<Option<(u8, u8)>, String> {
    let attachments = selection
        .body
        .hardware
        .iter()
        .flat_map(|hardware| &hardware.serial_attachments);
    for attachment in attachments.filter(|attachment| attachment.controller_id == block.id) {
        let panel = selection.parts.iter().any(|part| {
            part.id == attachment.part_id
                && matches!(
                    &part.model,
                    fujimod_catalog::reference::PartModel::Peripheral {
                        interface: fujimod_catalog::assembly::PeripheralPart::DisplayPanelSerial
                    }
                )
        });
        if !panel {
            continue;
        }
        let select = attachment.select_connection_id.as_ref().ok_or_else(|| {
            format!(
                "display panel on {} has no chip-select connection",
                block.id
            )
        })?;
        let connection = selection
            .body
            .connections
            .iter()
            .find(|connection| &connection.id == select)
            .ok_or_else(|| format!("display panel on {}: no connection {select}", block.id))?;
        let [ConnectionSignal::Gpio { port, pin, .. }] = connection.signals.as_slice() else {
            return Err(format!("connection {select} is not one GPIO line"));
        };
        return Ok(Some((*port, *pin)));
    }
    Ok(None)
}

fn port_device(
    selection: &ResolvedSelection,
    block: &fujimod_catalog::reference::SocBlock,
) -> Result<Option<&'static str>, String> {
    use fujimod_catalog::assembly::PeripheralPart;
    let mut kinds = selection
        .body
        .wiring
        .iter()
        .filter(|wire| {
            wire.source_id == block.id
                && wire.signal == fujimod_catalog::reference::WireSignal::SerialData
        })
        .map(|wire| {
            let part = selection
                .parts
                .iter()
                .find(|part| part.id == wire.target_id)
                .ok_or_else(|| {
                    format!(
                        "wire {} -> {}: no such part",
                        wire.source_id, wire.target_id
                    )
                })?;
            Ok(match &part.model {
                fujimod_catalog::reference::PartModel::Peripheral { interface } => {
                    match interface {
                        PeripheralPart::FocusActuator => Some("focus-actuator"),
                        PeripheralPart::IrisActuator => Some("iris-actuator"),
                        PeripheralPart::AudioSerialOutput => Some("audio-spi-sink"),
                        PeripheralPart::ImagingSerialOutput => Some("imaging-serial-sink"),
                        _ => None,
                    }
                }
                _ => None,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    match kinds.as_slice() {
        [Some(_)] => Ok(kinds.pop().flatten()),
        many if many.iter().any(Option::is_some) => Err(format!(
            "block {}: a whole-port output shares its port",
            block.id
        )),
        _ => Ok(None),
    }
}

/// The wire of a port whose one attached part has no chip select.
fn dedicated_wire(
    selection: &ResolvedSelection,
    block: &fujimod_catalog::reference::SocBlock,
) -> Result<Option<toml::Table>, String> {
    use fujimod_catalog::assembly::PeripheralPart;
    let attachments: Vec<_> = selection
        .body
        .hardware
        .iter()
        .flat_map(|hardware| &hardware.serial_attachments)
        .filter(|attachment| {
            attachment.controller_id == block.id && attachment.select_connection_id.is_none()
        })
        .collect();
    let [attachment] = attachments.as_slice() else {
        return if attachments.is_empty() {
            Ok(None)
        } else {
            Err(format!(
                "block {}: several parts share a port without chip selects",
                block.id
            ))
        };
    };
    let part = selection
        .parts
        .iter()
        .find(|part| part.id == attachment.part_id)
        .ok_or_else(|| format!("serial attachment {}: no such part", attachment.part_id))?;
    let fujimod_catalog::reference::PartModel::Peripheral { interface } = &part.model else {
        return Err(format!("part {} is not a serial peripheral", part.id));
    };
    let mut wire = toml::Table::new();
    match interface {
        PeripheralPart::FramedCommandPeer { initial_frame } => {
            wire.insert("kind".into(), toml::Value::String("dedicated".into()));
            let mut peer = toml::Table::new();
            peer.insert(
                "kind".into(),
                toml::Value::String("framed-command-peer".into()),
            );
            peer.insert(
                "initial_frame".into(),
                toml::Value::Integer(i64::from(*initial_frame)),
            );
            wire.insert("part".into(), toml::Value::Table(peer));
        }
        // A driver that never answers leaves its input at the idle level.
        PeripheralPart::UnidentifiedSerialDevice { miso_high } => {
            wire.insert("kind".into(), toml::Value::String("input-level".into()));
            wire.insert("high".into(), toml::Value::Boolean(*miso_high));
        }
        // Whole-port outputs are their own windows.
        PeripheralPart::FocusActuator
        | PeripheralPart::IrisActuator
        | PeripheralPart::AudioSerialOutput
        | PeripheralPart::ImagingSerialOutput => return Ok(None),
        other => return Err(format!("part {}: {other:?} cannot own a port", part.id)),
    }
    Ok(Some(wire))
}

/// The X-mount lens wire of the body's lens mount on `block`, with its mounted lens.
fn lens_wire(
    selection: &ResolvedSelection,
    block: &fujimod_catalog::reference::SocBlock,
) -> Result<Option<toml::Table>, String> {
    let Some(mount) = selection
        .body
        .hardware
        .as_ref()
        .and_then(|hardware| hardware.lens_mount.as_ref())
    else {
        return Ok(None);
    };
    if mount.controller_id != block.id {
        return Ok(None);
    }
    let pin = |id: &str| -> Result<toml::Value, String> {
        let connection = selection
            .body
            .connections
            .iter()
            .find(|connection| connection.id == id)
            .ok_or_else(|| format!("lens mount: no connection {id}"))?;
        connection
            .signals
            .iter()
            .find_map(|signal| match signal {
                ConnectionSignal::Gpio { port, pin, .. } => {
                    let mut row = toml::Table::new();
                    row.insert("port".into(), toml::Value::Integer(i64::from(*port)));
                    row.insert("pin".into(), toml::Value::Integer(i64::from(*pin)));
                    Some(toml::Value::Table(row))
                }
                _ => None,
            })
            .ok_or_else(|| format!("lens mount connection {id} carries no GPIO pad"))
    };
    let integer = |value: u64| toml::Value::Integer(i64::try_from(value).unwrap_or(i64::MAX));
    let mut wire = toml::Table::new();
    wire.insert("kind".into(), toml::Value::String("x-mount-lens".into()));
    wire.insert("detect".into(), pin(&mount.detect_connection_id)?);
    wire.insert(
        "sense".into(),
        toml::Value::Array(
            mount
                .sense_connection_ids
                .iter()
                .map(|id| pin(id))
                .collect::<Result<_, _>>()?,
        ),
    );
    wire.insert(
        "supplies".into(),
        toml::Value::Array(
            mount
                .supply_connection_ids
                .iter()
                .map(|id| pin(id))
                .collect::<Result<_, _>>()?,
        ),
    );
    wire.insert("ready".into(), pin(&mount.ready_connection_id)?);
    if let Some(function) = mount.ready_function {
        wire.insert("ready_mux".into(), integer(u64::from(function)));
    }
    wire.insert(
        "ready_low_line".into(),
        integer(u64::from(mount.ready_low_line)),
    );
    wire.insert(
        "ready_high_line".into(),
        integer(u64::from(mount.ready_high_line)),
    );
    let Some(lens_id) = &mount.lens_part_id else {
        return Err(format!(
            "body {}: an empty lens mount is unmodeled",
            selection.body.id
        ));
    };
    let lens = selection
        .parts
        .iter()
        .find(|part| &part.id == lens_id)
        .ok_or_else(|| format!("no lens part {lens_id}"))?;
    let fujimod_catalog::reference::PartModel::Peripheral {
        interface:
            fujimod_catalog::assembly::PeripheralPart::XMountLens {
                preset,
                sense_unpowered,
                startup_us,
                busy_us,
                operation_us,
            },
    } = &lens.model
    else {
        return Err(format!("part {lens_id} is not an X-mount lens"));
    };
    wire.insert("lens".into(), toml::Value::String(preset.clone()));
    wire.insert(
        "sense_unpowered".into(),
        toml::Value::Array(
            sense_unpowered
                .iter()
                .map(|level| toml::Value::Boolean(*level))
                .collect(),
        ),
    );
    wire.insert("startup_us".into(), integer(*startup_us));
    wire.insert("busy_us".into(), integer(*busy_us));
    wire.insert("operation_us".into(), integer(*operation_us));
    Ok(Some(wire))
}

/// The chip-selected endpoints body wiring attaches to a serial port, with their run inputs.
fn serial_endpoints(
    selection: &ResolvedSelection,
    block: &fujimod_catalog::reference::SocBlock,
    part_inputs: &[fujimod_catalog_configuration::application::ApplicationPartInput],
) -> Result<Vec<toml::Value>, String> {
    use fujimod_catalog::assembly::PeripheralPart;
    use fujimod_catalog_configuration::application::ApplicationPartInput as Input;
    let integer = |value: i64| toml::Value::Integer(value);
    let mut endpoints = Vec::new();
    let attachments = selection
        .body
        .hardware
        .iter()
        .flat_map(|hardware| &hardware.serial_attachments);
    for attachment in attachments.filter(|attachment| {
        attachment.controller_id == block.id && attachment.select_connection_id.is_some()
    }) {
        if selection.parts.iter().any(|part| {
            part.id == attachment.part_id
                && matches!(
                    &part.model,
                    fujimod_catalog::reference::PartModel::Peripheral {
                        interface: PeripheralPart::DisplayPanelSerial
                    }
                )
        }) {
            continue;
        }
        let Some(select_id) = &attachment.select_connection_id else {
            return Err(format!(
                "serial attachment {} on {} has no chip-select connection",
                attachment.part_id, block.id
            ));
        };
        let connection = selection
            .body
            .connections
            .iter()
            .find(|connection| &connection.id == select_id)
            .ok_or_else(|| {
                format!(
                    "serial attachment {}: no connection {select_id}",
                    attachment.part_id
                )
            })?;
        let [
            ConnectionSignal::Gpio {
                port,
                pin,
                active_level,
            },
        ] = connection.signals.as_slice()
        else {
            return Err(format!("connection {select_id} is not one GPIO line"));
        };
        let part = selection
            .parts
            .iter()
            .find(|part| part.id == attachment.part_id)
            .ok_or_else(|| format!("serial attachment {}: no such part", attachment.part_id))?;
        let fujimod_catalog::reference::PartModel::Peripheral { interface } = &part.model else {
            return Err(format!("part {} is not a serial peripheral", part.id));
        };
        let inputs: Vec<_> = part_inputs
            .iter()
            .filter(|input| input.part_id() == part.id)
            .collect();
        let missing = |what: &str| format!("part {}: the run supplies no {what}", part.id);
        let mut row = toml::Table::new();
        let kind = |row: &mut toml::Table, kind: &str| {
            row.insert("kind".into(), toml::Value::String(kind.into()));
        };
        match interface {
            PeripheralPart::PowerController { .. } => {
                kind(&mut row, "power-controller");
                let Some(Input::Clock {
                    seconds,
                    valid,
                    registers,
                    ..
                }) = inputs
                    .iter()
                    .find(|input| matches!(input, Input::Clock { .. }))
                    .copied()
                else {
                    return Err(missing("clock"));
                };
                row.insert("rtc_seconds".into(), integer(i64::from(*seconds)));
                row.insert("rtc_valid".into(), toml::Value::Boolean(*valid));
                if !registers.is_empty() {
                    row.insert(
                        "registers".into(),
                        toml::Value::Table(
                            registers
                                .iter()
                                .map(|word| {
                                    (
                                        format!("{:x}", word.register),
                                        integer(i64::from(word.value)),
                                    )
                                })
                                .collect(),
                        ),
                    );
                }
                if let (Some(mode), Some(flash_bytes)) = (
                    &attachment.programming_mode_connection_id,
                    attachment.programming_flash_bytes,
                ) {
                    let mut programmer = toml::Table::new();
                    programmer.insert("mode".into(), gpio_line(selection, mode)?);
                    programmer.insert(
                        "flash_bytes".into(),
                        integer(i64::try_from(flash_bytes).map_err(|_| {
                            format!("part {}: programming flash size overflows", part.id)
                        })?),
                    );
                    row.insert("programmer".into(), toml::Value::Table(programmer));
                }
            }
            _ if attachment.programming_mode_connection_id.is_some() => {
                return Err(format!(
                    "part {}: only a power controller has a programming mode",
                    part.id
                ));
            }
            PeripheralPart::SerialCalibrationRegisters => {
                kind(&mut row, "serial-calibration-registers");
                let high = inputs
                    .iter()
                    .find_map(|input| match input {
                        Input::IdleLevel { high, .. } => Some(*high),
                        _ => None,
                    })
                    .ok_or_else(|| missing("idle level"))?;
                row.insert("miso_high".into(), toml::Value::Boolean(high));
            }
            PeripheralPart::BisConfigurationRegisters => {
                kind(&mut row, "bis-configuration-registers");
                let Some(Input::BisRevision {
                    revision,
                    samples,
                    block,
                    idle_high,
                    ..
                }) = inputs
                    .iter()
                    .find(|input| matches!(input, Input::BisRevision { .. }))
                    .copied()
                else {
                    return Err(missing("BIS revision"));
                };
                let array = |values: Vec<i64>| {
                    toml::Value::Array(values.into_iter().map(integer).collect())
                };
                row.insert(
                    "revision".into(),
                    array(revision.iter().map(|v| i64::from(*v)).collect()),
                );
                row.insert(
                    "samples".into(),
                    array(samples.iter().map(|v| i64::from(*v)).collect()),
                );
                row.insert("idle_high".into(), toml::Value::Boolean(*idle_high));
                row.insert(
                    "block".into(),
                    array(block.iter().map(|v| i64::from(*v)).collect()),
                );
            }
            PeripheralPart::Lsm6dsConfigurationRegisters => {
                kind(&mut row, "lsm6ds-configuration-registers");
                let Some(Input::Motion {
                    temperature,
                    angular_rate,
                    acceleration,
                    ..
                }) = inputs
                    .iter()
                    .find(|input| matches!(input, Input::Motion { .. }))
                    .copied()
                else {
                    return Err(missing("motion"));
                };
                let array = |values: &[i16; 3]| {
                    toml::Value::Array(values.iter().map(|v| integer(i64::from(*v))).collect())
                };
                row.insert("temperature".into(), integer(i64::from(*temperature)));
                let mut motion = toml::Table::new();
                motion.insert("angular_rate".into(), array(angular_rate));
                motion.insert("acceleration".into(), array(acceleration));
                row.insert("motion".into(), toml::Value::Table(motion));
            }
            PeripheralPart::SerialCommandRegisters {
                bits,
                mode,
                read_flag,
                data_words,
                miso_high,
            } => {
                kind(&mut row, "serial-command-registers");
                row.insert("bits".into(), integer(i64::from(*bits)));
                row.insert("mode".into(), integer(i64::from(*mode)));
                if let Some(flag) = read_flag {
                    row.insert("read_flag".into(), integer(i64::from(*flag)));
                }
                if let Some(words) = data_words {
                    row.insert("data_words".into(), integer(i64::from(*words)));
                }
                row.insert("miso_high".into(), toml::Value::Boolean(*miso_high));
            }
            PeripheralPart::UnidentifiedSerialDevice { miso_high } => {
                kind(&mut row, "generic-sink");
                row.insert("miso_high".into(), toml::Value::Boolean(*miso_high));
            }
            other => {
                return Err(format!(
                    "part {}: {other:?} is not a chip-selected serial endpoint",
                    part.id
                ));
            }
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

/// The `{port, pin, active_high}` table of a body connection that is exactly one GPIO line.
fn gpio_line(selection: &ResolvedSelection, id: &str) -> Result<toml::Value, String> {
    let connection = selection
        .body
        .connections
        .iter()
        .find(|connection| connection.id == id)
        .ok_or_else(|| format!("body {}: no connection {id}", selection.body.id))?;
    let [
        ConnectionSignal::Gpio {
            port,
            pin,
            active_level,
        },
    ] = connection.signals.as_slice()
    else {
        return Err(format!("connection {id} is not one GPIO line"));
    };
    let mut line = toml::Table::new();
    line.insert("port".into(), toml::Value::Integer(i64::from(*port)));
    line.insert("pin".into(), toml::Value::Integer(i64::from(*pin)));
    line.insert("active_high".into(), toml::Value::Boolean(*active_level));
    Ok(toml::Value::Table(line))
}

fn block_window(
    block: &fujimod_catalog::reference::SocBlock,
    device: &BlockDevice,
    selection: &ResolvedSelection,
    flash: Option<FlashInput<'_>>,
    flash_image: Option<&fujimod_catalog::firmware::FlashImage>,
    samples: Option<&[u16]>,
    gpio_input: Option<&fujimod_catalog_configuration::application::ApplicationBlockInput>,
    run: &fujimod_catalog_configuration::RunConfiguration,
    ticks_per_second: u64,
    presented_jedec: Option<u32>,
    part_inputs: &[fujimod_catalog_configuration::application::ApplicationPartInput],
    memory: &RunMemory,
) -> Result<Option<(&'static str, toml::Table)>, String> {
    let mut options = toml::Table::new();
    let integer = |value: u64| toml::Value::Integer(i64::try_from(value).unwrap_or(i64::MAX));
    let irq = |name: &str| {
        block
            .interrupts
            .iter()
            .find(|row| row.output == name)
            .map(|row| row.line)
    };
    let kind = match device {
        BlockDevice::Xp5Timer { focus_capture, .. } => {
            if *focus_capture {
                "timer-focus-capture"
            } else {
                "timer"
            }
        }
        BlockDevice::Xp5TimerExtended { .. } => "timer-extended",
        BlockDevice::SerialFlashAperture { .. } => {
            options.insert(
                "dat_origin".into(),
                integer(flash_image.map(|image| image.dat_origin).unwrap_or(0)),
            );
            options.insert(
                "xor".into(),
                integer(flash_image.map(|image| image.xor).unwrap_or(255)),
            );
            options.insert(
                "erased_tail".into(),
                integer(flash.and_then(|item| *item.1).map(u64::from).unwrap_or(255)),
            );
            if let Some((part_id, _, seeds)) = flash {
                let part = selection.parts.iter().find(|part| part.id == part_id);
                if let Some(part) = part
                    && let fujimod_catalog::reference::PartModel::Peripheral { interface } =
                        &part.model
                    && matches!(
                        interface,
                        fujimod_catalog::assembly::PeripheralPart::SerialNandFlash { .. }
                    )
                {
                    options.insert("medium".into(), toml::Value::String("serial-nand".into()));
                }
                if !seeds.is_empty() {
                    options.insert(
                        "seed".into(),
                        toml::Value::Array(
                            seeds
                                .iter()
                                .map(|seed| {
                                    let mut row = toml::Table::new();
                                    row.insert("offset".into(), integer(seed.offset));
                                    row.insert("value".into(), integer(u64::from(seed.value)));
                                    toml::Value::Table(row)
                                })
                                .collect(),
                        ),
                    );
                }
            }
            "flash-storage"
        }
        BlockDevice::SerialFlashController { .. } => {
            let jedec = presented_jedec
                .or_else(|| {
                    flash
                        .and_then(|(part_id, _, _)| {
                            selection.parts.iter().find(|part| part.id == part_id)
                        })
                        .and_then(|part| match &part.model {
                            fujimod_catalog::reference::PartModel::Peripheral {
                                interface:
                                    fujimod_catalog::assembly::PeripheralPart::SerialNorFlash {
                                        jedec_id,
                                        ..
                                    }
                                    | fujimod_catalog::assembly::PeripheralPart::SerialNandFlash {
                                        jedec_id,
                                        ..
                                    },
                            } => *jedec_id,
                            _ => None,
                        })
                })
                .ok_or("flash controller needs a body flash part with a JEDEC identity")?;
            options.insert("jedec_id".into(), integer(u64::from(jedec)));
            "flash-storage-controller"
        }
        BlockDevice::FlashStorageDma { .. } => "flash-storage-dma",
        BlockDevice::Gates {
            configuration_word, ..
        } => {
            if let Some(word) = configuration_word {
                options.insert("configuration_word".into(), integer(*word));
            }
            if block.id == "clock-gates" {
                "clock-gates"
            } else {
                "power-gates"
            }
        }
        BlockDevice::ClockBanks { .. } => "clock-banks",
        BlockDevice::PanelPll { .. } => "panel-pll",
        BlockDevice::GeneralDma {
            companion_irq_base,
            a53_irq_base,
            ..
        } => {
            if let Some(base) = companion_irq_base {
                options.insert("companion_irq_base".into(), integer(u64::from(*base)));
            }
            if let Some(base) = a53_irq_base {
                options.insert("a53_irq_base".into(), integer(u64::from(*base)));
            }
            "general-dma"
        }
        BlockDevice::AudioDma {
            setup_ticks,
            bytes_per_tick,
            ..
        } => {
            options.insert(
                "irq_base".into(),
                integer(u64::from(irq("irq").unwrap_or(0))),
            );
            options.insert("setup_ticks".into(), integer(*setup_ticks));
            options.insert("bytes_per_tick".into(), integer(*bytes_per_tick));
            "audio-dma"
        }
        BlockDevice::Xp5Gpio {
            port: None,
            pad_model,
            ..
        } => {
            use fujimod_catalog_configuration::application::ApplicationBlockInput;
            let table = |pairs: Vec<(&str, toml::Value)>| {
                toml::Value::Table(
                    pairs
                        .into_iter()
                        .map(|(key, value)| (key.to_string(), value))
                        .collect(),
                )
            };
            if let Some(fujimod_catalog::soc::GpioPadModel::DataLatch) = pad_model {
                options.insert("pad_model".into(), toml::Value::String("data-latch".into()));
            }
            // A pin's interrupt is wired when the body connects something to that pin; the
            // companion core (core4) takes its pins on its own controller.
            let mut irqs = [
                ("input_irqs", Vec::new()),
                ("companion_input_irqs", Vec::new()),
            ];
            for interrupt in &block.interrupts {
                let Some((port, pin)) = interrupt
                    .output
                    .strip_prefix("port")
                    .and_then(|rest| rest.split_once("-pin"))
                else {
                    continue;
                };
                let (Ok(port), Ok(pin)) = (port.parse::<u8>(), pin.parse::<u8>()) else {
                    return Err(format!(
                        "block {}: interrupt output {} is not a pin",
                        block.id, interrupt.output
                    ));
                };
                let Some(active) = selection
                    .body
                    .connections
                    .iter()
                    .flat_map(|connection| &connection.signals)
                    .find_map(|signal| match signal {
                        ConnectionSignal::Gpio {
                            port: p,
                            pin: n,
                            active_level,
                        } if *p == port && *n == pin => Some(*active_level),
                        _ => None,
                    })
                else {
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
                if !rows.is_empty() {
                    options.insert(key.into(), toml::Value::Array(rows));
                }
            }
            let feedback = gpio_feedback(selection, Some(ticks_per_second))?;
            if !feedback.is_empty() {
                options.insert(
                    "feedback".into(),
                    toml::Value::try_from(feedback).map_err(|error| error.to_string())?,
                );
            }
            if let Some(ApplicationBlockInput::Gpio {
                outputs,
                pins,
                registers,
                ..
            }) = gpio_input
            {
                if !outputs.is_empty() {
                    options.insert(
                        "initial_outputs".into(),
                        toml::Value::Array(
                            outputs
                                .iter()
                                .map(|word| {
                                    table(vec![
                                        ("port", integer(u64::from(word.port))),
                                        ("levels", integer(u64::from(word.levels))),
                                    ])
                                })
                                .collect(),
                        ),
                    );
                }
                if !pins.is_empty() {
                    options.insert(
                        "initial_inputs".into(),
                        toml::Value::Array(
                            pins.iter()
                                .map(|level| {
                                    table(vec![
                                        ("port", integer(u64::from(level.port))),
                                        ("pin", integer(u64::from(level.pin))),
                                        ("high", toml::Value::Boolean(level.high)),
                                    ])
                                })
                                .collect(),
                        ),
                    );
                }
                if !registers.is_empty() {
                    options.insert(
                        "initial_registers".into(),
                        toml::Value::Array(
                            registers
                                .iter()
                                .map(|word| {
                                    table(vec![
                                        ("port", integer(u64::from(word.port))),
                                        ("register", integer(u64::from(word.register))),
                                        ("value", integer(u64::from(word.value))),
                                    ])
                                })
                                .collect(),
                        ),
                    );
                }
            }
            "gpio"
        }
        BlockDevice::Xp5Gpio { port: Some(3), .. } => "gpio-port3",
        BlockDevice::Xp5Gpio {
            port: Some(port), ..
        } => {
            return Err(format!(
                "block {}: no XP5 single-port GPIO view of port {port}",
                block.id
            ));
        }
        BlockDevice::HdmiTransmitter {
            hot_plug_block_id, ..
        } => {
            // The transmitter owns the hot-plug word of its cable-status block: one model
            // answers the sink's hot-plug line there and its DDC and status here.
            let hot_plug = selection
                .soc
                .blocks
                .iter()
                .find(|other| &other.id == hot_plug_block_id)
                .ok_or_else(|| format!("block {}: no block {hot_plug_block_id}", block.id))?;
            let Some(BlockDevice::CableStatus {
                connected_level, ..
            }) = &hot_plug.device
            else {
                return Err(format!(
                    "block {}: {hot_plug_block_id} is not a cable-status word",
                    block.id
                ));
            };
            options.insert(
                "interrupt".into(),
                integer(u64::from(
                    irq("irq").ok_or_else(|| format!("block {}: no interrupt", block.id))?,
                )),
            );
            options.insert("hot_plug_base".into(), integer(hot_plug.base));
            options.insert("hot_plug_size".into(), integer(hot_plug.size));
            options.insert(
                "connected_level".into(),
                integer(u64::from(*connected_level)),
            );
            // A body with an HDMI connector presents a sink; whether one is plugged in at the
            // start is the run's input for that part.
            let connectors: Vec<_> = selection
                .parts
                .iter()
                .filter(|part| {
                    matches!(
                        &part.model,
                        fujimod_catalog::reference::PartModel::Peripheral {
                            interface: fujimod_catalog::assembly::PeripheralPart::HdmiConnector
                        }
                    )
                })
                .collect();
            match connectors.as_slice() {
                [] => {}
                [connector] => {
                    let connected = part_inputs.iter().find_map(|input| match input {
                        fujimod_catalog_configuration::application::ApplicationPartInput::HdmiSink {
                            part_id,
                            connected,
                        } if part_id == &connector.id => Some(*connected),
                        _ => None,
                    });
                    options.insert(
                        "sink_connected".into(),
                        toml::Value::Boolean(connected.unwrap_or(false)),
                    );
                }
                _ => {
                    return Err(format!(
                        "body {} installs more than one HDMI connector",
                        selection.body.id
                    ));
                }
            }
            "hdmi-transmitter"
        }
        BlockDevice::CableStatus { .. }
            if block.kind == SocBlockKind::DisplayController
                && selection.soc.blocks.iter().any(|other| {
                    matches!(
                        &other.device,
                        Some(BlockDevice::HdmiTransmitter { hot_plug_block_id, .. })
                            if hot_plug_block_id == &block.id
                    )
                }) =>
        {
            return Ok(None);
        }
        BlockDevice::CableStatus {
            connected_level, ..
        } => match block.kind {
            SocBlockKind::UsbController if *connected_level == 1 => "usb-connection-status",
            SocBlockKind::DisplayController => {
                options.insert(
                    "connected_level".into(),
                    integer(u64::from(*connected_level)),
                );
                "hdmi-hot-plug-status"
            }
            _ => return Err(format!("block {}: no XP5 cable status model", block.id)),
        },
        BlockDevice::Xp5Adc { .. } => {
            if let Some(samples) = samples {
                options.insert(
                    "samples".into(),
                    toml::Value::Array(
                        samples
                            .iter()
                            .map(|value| integer(u64::from(*value)))
                            .collect(),
                    ),
                );
            }
            "adc"
        }
        BlockDevice::Temperature { .. } => "soc-temperature",
        BlockDevice::InterprocessorMailbox { .. } => "interprocessor-mailbox",
        BlockDevice::AudioSerial { reset_offset, .. } => {
            match run
                .audio_signals
                .iter()
                .find(|signal| signal.block_id == block.id)
            {
                Some(signal) => {
                    options.insert("sample_rate_hz".into(), integer(signal.sample_rate_hz));
                    options.insert("tone_hz".into(), integer(signal.tone_hz));
                    options.insert(
                        "tone_amplitude".into(),
                        toml::Value::Float(f64::from(signal.tone_amplitude_thousandths) / 1000.0),
                    );
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
        BlockDevice::DdrPhy { .. } => {
            options.insert("registers".into(), stub_registers(block));
            "bringup-stub"
        }
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
            if body_hardware(selection).is_some_and(|hardware| hardware.idle_status_blocks.contains(&block.id)) {
                options.insert("idle_status".into(), toml::Value::Boolean(true));
            }
            "imaging-input"
        }
        BlockDevice::ImagingResize { .. } => {
            options.insert("completion_us".into(), integer(1000));
            "imaging-resize"
        }
        BlockDevice::ImagingPaf { .. } => "imaging-paf",
        BlockDevice::ImagingPreview {
            focus_statistics, ..
        } => {
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
        BlockDevice::ImagingResz {
            handshake,
            inline_source,
            ..
        } => {
            stage_handshake(&mut options, handshake.as_ref(), irq("irq"));
            if *inline_source {
                options.insert("inline_source".into(), toml::Value::Boolean(true));
            }
            "imaging-resz"
        }
        BlockDevice::InputCapture {
            transmit_requests, ..
        } => {
            if !transmit_requests.is_empty() {
                options.insert(
                    "dma_transmit".into(),
                    toml::Value::Array(
                        transmit_requests
                            .iter()
                            .map(|wire| {
                                let mut row = toml::Table::new();
                                row.insert("channel".into(), integer(u64::from(wire.channel)));
                                row.insert("request".into(), integer(u64::from(wire.request)));
                                toml::Value::Table(row)
                            })
                            .collect(),
                    ),
                );
            }
            if let Some(stage) = selection
                .body
                .hardware
                .as_ref()
                .and_then(|hardware| hardware.stabilizer.as_ref())
                .filter(|stage| stage.capture_block_id == block.id)
            {
                let part = selection
                    .parts
                    .iter()
                    .find(|part| part.id == stage.part_id)
                    .ok_or_else(|| format!("no stabilizer part {}", stage.part_id))?;
                let fujimod_catalog::reference::PartModel::Peripheral {
                    interface:
                        fujimod_catalog::assembly::PeripheralPart::StabilizerStage {
                            latency_ns,
                            center,
                            rest,
                            lag_shift,
                            drive_gain,
                        },
                } = &part.model
                else {
                    return Err(format!("part {} is not a stabilizer stage", part.id));
                };
                let output = format!("channel{}", stage.capture_channel);
                let line = block
                    .interrupts
                    .iter()
                    .find(|row| row.output == output && row.core.as_deref() == Some("core4"))
                    .map(|row| row.line)
                    .ok_or_else(|| {
                        format!(
                            "block {}: capture {output} has no companion interrupt",
                            block.id
                        )
                    })?;
                let array = |values: Vec<i64>| {
                    toml::Value::Array(values.into_iter().map(toml::Value::Integer).collect())
                };
                let mut sequencer = toml::Table::new();
                sequencer.insert("channel".into(), integer(u64::from(stage.capture_channel)));
                sequencer.insert("companion_irq_line".into(), integer(u64::from(line)));
                sequencer.insert("latency_ns".into(), integer(*latency_ns));
                sequencer.insert(
                    "center".into(),
                    array(center.iter().map(|v| i64::from(*v)).collect()),
                );
                sequencer.insert(
                    "rest".into(),
                    array(rest.iter().map(|v| i64::from(*v)).collect()),
                );
                sequencer.insert("lag_shift".into(), integer(u64::from(*lag_shift)));
                sequencer.insert(
                    "drive_channels".into(),
                    array(stage.drive_channels.iter().map(|v| i64::from(*v)).collect()),
                );
                sequencer.insert(
                    "drive_gain".into(),
                    array(drive_gain.iter().map(|v| i64::from(*v)).collect()),
                );
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
        BlockDevice::Pl022 { source_hz, .. } if body_hardware(selection).is_some_and(|hardware| hardware.polled_serial_blocks.contains(&block.id)) => {
            options.insert("source_hz".into(), integer(*source_hz));
            let endpoints = serial_endpoints(selection, block, part_inputs)?;
            if !endpoints.is_empty() {
                options.insert("endpoints".into(), toml::Value::Array(endpoints));
            }
            "spi-controller"
        }
        BlockDevice::Pl022 {
            source_hz,
            ticks_per_second,
            receive_request,
            transmit_request,
            companion_irq_line,
            ..
        } => {
            options.insert("source_hz".into(), integer(*source_hz));
            options.insert("ticks_per_second".into(), integer(*ticks_per_second));
            if let Some(value) = receive_request {
                options.insert("receive_request".into(), integer(u64::from(*value)));
            }
            if let Some(value) = transmit_request {
                options.insert("transmit_request".into(), integer(u64::from(*value)));
            }
            if let Some(value) = companion_irq_line {
                options.insert("companion_irq_line".into(), integer(u64::from(*value)));
            }
            if let Some(line) = irq("irq") {
                options.insert("gic_line".into(), integer(u64::from(line)));
            }
            if body_hardware(selection)
                .is_some_and(|hardware| hardware.unobserved_interrupt_blocks.contains(&block.id)) {
                options.insert(
                    "companion_irq_unconnected".into(),
                    toml::Value::Boolean(true),
                );
            }
            let endpoints = serial_endpoints(selection, block, part_inputs)?;
            let mut wire = match lens_wire(selection, block)? {
                Some(wire) => wire,
                None => dedicated_wire(selection, block)?.unwrap_or_default(),
            };
            if !endpoints.is_empty() {
                if !wire.is_empty() {
                    return Err(format!(
                        "block {}: a lens mount shares its port with chip-selected endpoints",
                        block.id
                    ));
                }
                wire.insert("kind".into(), toml::Value::String("gpio-selected".into()));
                wire.insert("endpoints".into(), toml::Value::Array(endpoints));
            }
            if wire.is_empty() {
                // Nothing attached: a transfer refuses instead of inventing a reply.
                wire.insert("kind".into(), toml::Value::String("unconnected".into()));
            }
            options.insert("wire".into(), toml::Value::Table(wire));
            "pl022"
        }
        BlockDevice::Completion {
            fixed_words,
            starts,
            completion,
            ack,
            irq_mask,
            delay_ticks,
            ..
        } => {
            if !fixed_words.is_empty() {
                options.insert(
                    "registers".into(),
                    toml::Value::Array(
                        fixed_words
                            .iter()
                            .map(|word| {
                                let mut row = toml::Table::new();
                                row.insert("offset".into(), integer(word.offset));
                                row.insert("mode".into(), toml::Value::String("constant".into()));
                                row.insert("value".into(), integer(u64::from(word.value)));
                                toml::Value::Table(row)
                            })
                            .collect(),
                    ),
                );
            }
            handshake_options(
                &mut options,
                starts,
                completion,
                ack.as_ref(),
                irq_mask.as_ref(),
            );
            options.insert("delay_us".into(), integer(delay_ticks / 1000));
            if let Some(line) = irq("irq") {
                options.insert("irq".into(), integer(u64::from(line)));
            }
            "completion-stub"
        }
        BlockDevice::VideoCodec { .. } => {
            options.insert(
                "irq".into(),
                integer(u64::from(irq("irq").ok_or_else(|| {
                    format!("block {} declares no interrupt", block.id)
                })?)),
            );
            "xp5-codec"
        }
        BlockDevice::ImagingBuffer { .. } => "imaging-buffer",
        BlockDevice::Xp5ImagingStatistics { .. } => {
            // Statistics banks in RAM the run declares are that RAM: the device writes them
            // through the bus instead of serving a window over them.
            if memory.in_ram(block.base, block.size) {
                options.insert("ram".into(), toml::Value::Boolean(true));
            }
            "imaging-statistics"
        }
        // A loader start is the direct-flash boot; a ROM start replaces the straps with its own.
        BlockDevice::BootControl {
            direct_flash_straps,
            ..
        } => {
            options.insert(
                "source_straps".into(),
                integer(u64::from(*direct_flash_straps)),
            );
            options.insert("initial_control".into(), integer(0));
            options.insert("initial_primary_control".into(), integer(0));
            "boot-control"
        }
        BlockDevice::ImagingDsp { handshake, .. } => {
            stage_handshake(&mut options, handshake.as_ref(), irq("irq"));
            "completion-stub"
        }
        BlockDevice::RegisterBlock {
            idle_bits,
            fixed_words,
            reads_back,
            ..
        } => {
            if !idle_bits.is_empty() || !fixed_words.is_empty() {
                options.insert(
                    "registers".into(),
                    toml::Value::Array(
                        fixed_words
                            .iter()
                            .map(|word| {
                                let mut row = toml::Table::new();
                                row.insert("offset".into(), integer(word.offset));
                                row.insert("mode".into(), toml::Value::String("constant".into()));
                                row.insert("value".into(), integer(u64::from(word.value)));
                                toml::Value::Table(row)
                            })
                            .collect(),
                    ),
                );
            }
            if block.id == "dma-routes" {
                "dma-request-routing"
            } else if block.id == "dma-address-high" {
                "dma-address-high"
            } else if *reads_back
                || block
                    .registers
                    .iter()
                    .any(|register| register.reads.is_some())
            {
                options.insert("registers".into(), stub_registers(block));
                "bringup-stub"
            } else {
                "write-only-registers"
            }
        }
        BlockDevice::SdHost {
            transfer_ticks,
            clock_ticks,
            address_high,
            ..
        } => {
            let mut controller = toml::Table::new();
            controller.insert("base".into(), integer(block.base));
            controller.insert("protocol".into(), toml::Value::String("sdhc".into()));
            controller.insert("transfer_ticks".into(), integer(*transfer_ticks));
            controller.insert("clock_ticks".into(), integer(*clock_ticks));
            controller.insert("rca".into(), integer(1));
            // The host's DMA reaches the run's declared DRAM: the span from 0, then the others.
            let (low, banks): (Vec<_>, Vec<_>) = memory.dram().iter().partition(|span| span.0 == 0);
            let [(_, low)] = low.as_slice() else {
                return Err(format!(
                    "block {}: the SD host's DMA needs the run's DRAM from address 0; the run \
                     declares none there",
                    block.id
                ));
            };
            controller.insert("ram_bytes".into(), integer(*low));
            if !banks.is_empty() {
                controller.insert(
                    "ram_banks".into(),
                    toml::Value::Array(
                        banks
                            .iter()
                            .map(|(base, end)| {
                                toml::Value::Array(vec![integer(*base), integer(end - base)])
                            })
                            .collect(),
                    ),
                );
            }
            controller.insert("dram_alias".into(), toml::Value::Boolean(false));
            if let Some(address) = address_high {
                controller.insert("address_high".into(), integer(*address));
            }
            if let Some(steps) = body_hardware(selection)
                .into_iter()
                .flat_map(|hardware| &hardware.sd_clock_steps)
                .find(|steps| steps.block_id == block.id)
            {
                let mut dividers = toml::Table::new();
                dividers.insert("interface".into(), integer(u64::from(steps.interface)));
                dividers.insert(
                    "default_speed".into(),
                    integer(u64::from(steps.default_speed)),
                );
                dividers.insert("high_speed".into(), integer(u64::from(steps.high_speed)));
                controller.insert("speed_dividers".into(), toml::Value::Table(dividers));
            }
            options.insert("controller".into(), toml::Value::Table(controller));
            if let Some(slot) = selection
                .body
                .hardware
                .iter()
                .flat_map(|hardware| &hardware.card_slots)
                .find(|slot| slot.controller_id == block.id)
            {
                options.insert(
                    "detect_gpio".into(),
                    gpio_line(selection, &slot.detect_connection_id)?,
                );
            }
            "sdhci"
        }
        BlockDevice::Decompressor { .. } => "lz",
        BlockDevice::Movdc { .. } => "movdc",
        BlockDevice::SensorCommand { .. } => {
            options.insert("command_latency_us".into(), integer(1));
            // The sensor chip's facts are the body's one image-sensor part.
            let sensors: Vec<_> = selection
                .parts
                .iter()
                .filter_map(|part| match &part.model {
                    fujimod_catalog::reference::PartModel::Peripheral {
                        interface:
                            fujimod_catalog::assembly::PeripheralPart::ImageSensor {
                                read_gate,
                                standby_gate,
                                fast_readout,
                                ..
                            },
                    } => Some((read_gate, standby_gate, fast_readout)),
                    _ => None,
                })
                .collect();
            let [(read_gate, standby_gate, fast_readout)] = sensors.as_slice() else {
                return Err(format!(
                    "block {}: body {} installs {} image-sensor parts; the sensor command \
                     interface needs exactly one",
                    block.id,
                    selection.body.id,
                    sensors.len()
                ));
            };
            for (key, gate) in [("read_gate", read_gate), ("standby_gate", standby_gate)] {
                if let Some(gate) = gate {
                    let mut row = toml::Table::new();
                    row.insert("register".into(), integer(u64::from(gate.register)));
                    row.insert("mask".into(), integer(u64::from(gate.mask)));
                    options.insert(key.into(), toml::Value::Table(row));
                }
            }
            if let Some(readout) = fast_readout {
                options.insert(
                    "frame_timing_readout".into(),
                    integer(u64::from(readout.readout)),
                );
                options.insert("frame_timing_line_ns".into(), integer(readout.line_ns));
            }
            "imaging-sensor"
        }
        BlockDevice::ImagingReceiver { .. } => "imaging-receiver",
        BlockDevice::Display { .. } => "display",
        BlockDevice::IoGroups { .. } => "pin-control",
        BlockDevice::Usb { .. } => {
            options.insert(
                "interrupt".into(),
                integer(u64::from(irq("irq").unwrap_or(0))),
            );
            options.insert("speed".into(), integer(0));
            options.insert("command_ticks".into(), integer(1000));
            options.insert("transaction_ticks".into(), integer(125000));
            "usb-device-controller"
        }
        BlockDevice::UsbPhy { .. } => "usb-phy",
        BlockDevice::JpegCodec {
            run_ticks,
            direction,
            ..
        } => {
            let line = irq("irq")
                .ok_or_else(|| format!("block {}: JPEG codec has no interrupt", block.id))?;
            options.insert("irq".into(), integer(u64::from(line)));
            options.insert("run_ticks".into(), integer(*run_ticks));
            match direction {
                Some(fujimod_catalog::soc::JpegDirection::Decode) => "jpeg-decoder",
                Some(fujimod_catalog::soc::JpegDirection::Encode) => "jpeg-encoder",
                None => {
                    return Err(format!(
                        "block {}: XP5 JPEG blocks declare a direction",
                        block.id
                    ));
                }
            }
        }
        BlockDevice::I2c {
            fifo_depth,
            clock_hz,
            tick_hz,
            ..
        } => {
            let _ = (fifo_depth, clock_hz, tick_hz);
            if let Some(line) = irq("interrupt") {
                options.insert("interrupt".into(), integer(u64::from(line)));
            }
            let mut targets = Vec::new();
            for wire in selection
                .body
                .wiring
                .iter()
                .filter(|wire| wire.source_id == block.id)
            {
                let address = wire.address.ok_or_else(|| {
                    format!(
                        "wire {} -> {} has no bus address",
                        wire.source_id, wire.target_id
                    )
                })?;
                let part = selection
                    .parts
                    .iter()
                    .find(|part| part.id == wire.target_id)
                    .ok_or_else(|| {
                        format!(
                            "wire {} -> {}: no such part",
                            wire.source_id, wire.target_id
                        )
                    })?;
                let fujimod_catalog::reference::PartModel::Peripheral { interface } = &part.model
                else {
                    return Err(format!("part {} is not a control-bus peripheral", part.id));
                };
                let mut interface =
                    toml::Value::try_from(interface).map_err(|error| error.to_string())?;
                if let Some(line) = target_interrupt(selection, &block.id, address)? {
                    interface
                        .as_table_mut()
                        .ok_or_else(|| format!("part {} has no interface table", part.id))?
                        .insert("interrupt".into(), integer(u64::from(line)));
                }
                let mut row = toml::Table::new();
                row.insert("address".into(), integer(u64::from(address)));
                row.insert("part".into(), interface);
                targets.push(toml::Value::Table(row));
            }
            if !targets.is_empty() {
                options.insert("targets".into(), toml::Value::Array(targets));
            }
            if let Some(time) = body_hardware(selection)
                .into_iter()
                .flat_map(|hardware| &hardware.bus_byte_times)
                .find(|time| time.block_id == block.id)
            {
                options.insert(
                    "byte_ns".into(),
                    integer(time.ticks * 1_000_000_000 / ticks_per_second),
                );
            }
            "i2c-controller"
        }
        BlockDevice::ResetGates { .. } => "imaging-reset",
        BlockDevice::GicV2 { .. } | BlockDevice::GicV3 { .. } => return Ok(None),
        BlockDevice::UnmodeledBusDrain { .. } => return Ok(None),
        BlockDevice::DramController { .. } => return Ok(None),
        other => {
            return Err(format!(
                "block {} has no XP5 emulator window for {other:?}",
                block.id
            ));
        }
    };
    Ok(Some((kind, options)))
}

/// The interrupt line of the control-bus target at `address` on `bus`: a body connection that
/// routes that serial target and an interrupt-controller input carries it.
fn target_interrupt(
    selection: &ResolvedSelection,
    bus: &str,
    address: u8,
) -> Result<Option<u32>, String> {
    let mut lines = selection
        .body
        .connections
        .iter()
        .filter(|connection| {
            connection.signals.iter().any(|signal| {
                matches!(signal, ConnectionSignal::Serial { block_id, address: a, .. }
                    if block_id == bus && *a == address)
            })
        })
        .flat_map(|connection| {
            connection.signals.iter().filter_map(|signal| match signal {
                ConnectionSignal::Interrupt {
                    controller_id,
                    line,
                } => Some((controller_id, *line)),
                _ => None,
            })
        });
    let Some((controller, line)) = lines.next() else {
        return Ok(None);
    };
    if lines.next().is_some() {
        return Err(format!(
            "control-bus target {address:#x} on {bus} has more than one interrupt"
        ));
    }
    if !selection
        .soc
        .blocks
        .iter()
        .any(|block| &block.id == controller && block.kind == SocBlockKind::InterruptController)
    {
        return Err(format!(
            "control-bus target {address:#x} on {bus} interrupts {controller}, not an \
             interrupt controller"
        ));
    }
    Ok(Some(line))
}

/// The body's GPIO feedback wires: each drives an input pad from the output
/// latch bit of the same pin number (register 0, the port's drive field).
fn gpio_feedback(
    selection: &ResolvedSelection,
    ticks_per_second: Option<u64>,
) -> Result<Vec<Feedback>, String> {
    let body = &selection.body;
    let Some(hardware) = body.hardware.as_ref() else {
        return Ok(vec![]);
    };
    let pin = |id: &str| -> Result<(usize, usize, bool), String> {
        let connection = body
            .connections
            .iter()
            .find(|connection| connection.id == id)
            .ok_or_else(|| {
                format!(
                    "body {} feedback refers to missing connection {id}",
                    body.id
                )
            })?;
        match connection.signals.as_slice() {
            [
                ConnectionSignal::Gpio {
                    port,
                    pin,
                    active_level,
                },
            ] => Ok((usize::from(*port), usize::from(*pin), *active_level)),
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
                (Some(ns), Some(rate)) => {
                    u64::try_from(u128::from(ns) * u128::from(rate) / 1_000_000_000)
                        .map_err(|_| format!("body {} feedback rise time overflows", body.id))?
                }
                (Some(_), None) => {
                    return Err(format!(
                        "body {} delayed GPIO feedback needs a run tick rate",
                        body.id
                    ));
                }
            };
            Ok(Feedback {
                port,
                register: 0,
                bit,
                input_port,
                input_pin,
                rise_delay,
                latch_high: wire.latch_high,
            })
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
    let contacts = resources
        .sd
        .iter()
        .map(|(base, media)| (format!("{base:#x}"), media.contacts.clone()))
        .collect();
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
    use fujimod_emulator_parts_controls::encoder::{Encoder, OutputWiring, Outputs, Wiring};
    let mut encoders = Vec::new();
    let mut gpio_sources = Vec::new();
    let mut adc_sources = Vec::new();
    for (name, options) in [(REAR_DIAL_NODE, dials.rear), (FRONT_DIAL_NODE, dials.front)] {
        let Some(options) = options else { continue };
        let (outputs, sources) = match &options.outputs {
            Outputs::Gpio => (
                OutputWiring::Gpio(Box::new(DialPads(resources.pins.clone()))),
                &mut gpio_sources,
            ),
            Outputs::Adc { .. } => (
                OutputWiring::Adc(
                    resources
                        .adc_samples
                        .clone()
                        .ok_or("ADC dial needs an ADC")?,
                ),
                &mut adc_sources,
            ),
        };
        let encoder = Encoder::new(
            options,
            Wiring {
                outputs,
                counter: None,
            },
        )?;
        let node = graph.attach(Box::new(encoder));
        sources.push(node);
        encoders.push((name, node));
    }
    if !adc_sources.is_empty() {
        let adc = graph.at(fujimod_emulator_xp5_io::adc::BASE)?;
        graph.depend_on(adc, adc_sources)?;
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
            // RAM-backed statistics register no device.
            Err(_)
                if window.kind == "imaging-statistics"
                    && window.options.get("ram") == Some(&toml::Value::Boolean(true)) => {}
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
        causes.extend(&gpio_sources);
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
    let hdmi = windows
        .iter()
        .find(|window| window.kind == "hdmi-transmitter")
        .map(|window| graph.at(window.base))
        .transpose()?;
    let bus = Bus::from_graph(mode, graph)?;
    Ok(DeviceAssembly {
        contacts,
        bus,
        inputs: crate::session::Inputs::new(resources.pins, gpio_devices, usb, hdmi),
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
mod dram_tests {
    use super::{RunMemory, RunRam, catalog_dram, device_windows};
    use fujimod_catalog_resolution::ResolvedConfiguration;

    fn memory(regions: &[(u64, u64, bool)]) -> RunMemory {
        RunMemory::new(
            regions
                .iter()
                .map(|&(base, size, dram)| RunRam { base, size, dram }),
        )
        .unwrap()
    }

    fn drains(resolved: &ResolvedConfiguration, memory: &RunMemory) -> Vec<u64> {
        device_windows(resolved, memory)
            .unwrap()
            .iter()
            .filter(|window| window.kind == "discarded-read")
            .map(|window| window.base)
            .collect()
    }

    fn sd_reach(resolved: &ResolvedConfiguration, memory: &RunMemory) -> Vec<toml::Value> {
        device_windows(resolved, memory)
            .unwrap()
            .iter()
            .filter(|window| window.kind == "sdhci")
            .map(|window| {
                let controller = window.options["controller"].as_table().unwrap();
                toml::Value::Array(vec![
                    controller["ram_bytes"].clone(),
                    controller
                        .get("ram_banks")
                        .cloned()
                        .unwrap_or(toml::Value::Array(vec![])),
                ])
            })
            .collect()
    }

    #[test]
    fn each_body_gets_its_own_drain_beside_the_soc_ones() {
        let x100vi = ResolvedConfiguration::bundled("x100vi-131").unwrap();
        let xt5 = ResolvedConfiguration::bundled("xt5-431").unwrap();
        let x100vi = drains(&x100vi, &catalog_dram(&x100vi));
        let xt5 = drains(&xt5, &catalog_dram(&xt5));
        assert!(x100vi.contains(&0x6800_0000) && !x100vi.contains(&0x6000_0000));
        assert!(xt5.contains(&0x6000_0000) && !xt5.contains(&0x6800_0000));
        for drains in [&x100vi, &xt5] {
            assert!(drains.contains(&0x8000_0000) && drains.contains(&0x1_c000_0000));
        }
    }

    #[test]
    fn a_drain_in_declared_dram_is_ram_and_others_stay() {
        // The X-H2S body declares three drains; DRAM over two of them leaves only the third
        // (and the SoC's drains outside DRAM). Non-DRAM RAM does not remove a drain.
        let xh2s = ResolvedConfiguration::bundled("xh2s-730").unwrap();
        let low = drains(&xh2s, &memory(&[(0, 0x8000_0000, true)]));
        for drain in [0x8000_0000, 0x1_0000_0000, 0x1_8000_0000, 0x1_c000_0000] {
            assert!(low.contains(&drain), "{drain:#x}");
        }
        let wide = drains(
            &xh2s,
            &memory(&[
                (0, 0x8000_0000, true),
                (0x8000_0000, 0x8000_0000, true),
                (0x1_0000_0000, 0x8000_0000, true),
                (0x1_8000_0000, 0x1000, false),
            ]),
        );
        assert_eq!(wide, [0x1_8000_0000, 0x1_c000_0000]);
    }

    #[test]
    fn the_sd_hosts_reach_the_declared_dram() {
        let xt5 = ResolvedConfiguration::bundled("xt5-431").unwrap();
        let reach = sd_reach(
            &xt5,
            &memory(&[
                (0, 0x4000_0000, true),
                (0x4000_0000, 0x8000_0000, true),
                (0xfa20_0000, 0x10_0000, false),
                (0x1_c000_4000, 0x3fff_c000, true),
                (0xff80_0000, 0x4_0000, false),
            ]),
        );
        let expected: toml::Value =
            toml::from_str::<toml::Table>("v = [0xc0000000, [[0x1c0004000, 0x3fffc000]]]").unwrap()
                ["v"]
                .clone();
        assert_eq!(reach, [expected.clone(), expected]);
        // DRAM only from 0: no banks.
        assert_eq!(
            sd_reach(&xt5, &memory(&[(0, 0x8000_0000, true)]))[0],
            toml::Value::Array(vec![
                toml::Value::Integer(0x8000_0000),
                toml::Value::Array(vec![])
            ])
        );
        // No DRAM from 0 (or none at all): the SD hosts refuse.
        for regions in [
            &[(0x4000_0000, 0x4000_0000, true)][..],
            &[(0, 0x4000_0000, false)][..],
        ] {
            let error = device_windows(&xt5, &memory(regions)).unwrap_err();
            assert!(error.contains("DRAM from address 0"), "{error}");
        }
    }

    #[test]
    fn statistics_in_declared_ram_are_ram_backed() {
        let statistics = |memory: &RunMemory| {
            let resolved = ResolvedConfiguration::bundled("xt5-431").unwrap();
            device_windows(&resolved, memory)
                .unwrap()
                .into_iter()
                .find(|window| window.kind == "imaging-statistics")
                .unwrap()
                .options
        };
        // Boot SRAM declared as (non-DRAM) RAM over the banks.
        let ram = statistics(&memory(&[
            (0, 0x8000_0000, true),
            (0xff80_0000, 0x4_0000, false),
        ]));
        assert_eq!(ram.get("ram"), Some(&toml::Value::Boolean(true)));
        // RAM that covers only part of the banks, or none, leaves the device window.
        for regions in [
            &[(0, 0x8000_0000, true), (0xff80_0000, 0x1000, false)][..],
            &[(0, 0x8000_0000, true)][..],
        ] {
            assert!(!statistics(&memory(regions)).contains_key("ram"));
        }
    }

    #[test]
    fn overlapping_dram_regions_are_refused() {
        let regions = [
            RunRam {
                base: 0,
                size: 0x8000_0000,
                dram: true,
            },
            RunRam {
                base: 0x7000_0000,
                size: 0x1000_0000,
                dram: true,
            },
        ];
        let error = RunMemory::new(regions).unwrap_err();
        assert!(error.contains("overlap"), "{error}");
        // Non-DRAM RAM may sit anywhere; an empty region is refused.
        let mut sram = regions;
        sram[1].dram = false;
        assert!(RunMemory::new(sram).is_ok());
        sram[1].size = 0;
        assert!(RunMemory::new(sram).is_err());
    }
}

#[cfg(test)]
mod image_sensor_tests {
    use super::device_windows;
    use fujimod_catalog_resolution::ResolvedConfiguration;

    fn sensor(resolved: &ResolvedConfiguration) -> Result<toml::Table, String> {
        Ok(device_windows(resolved, &super::catalog_dram(resolved))?
            .into_iter()
            .find(|window| window.kind == "imaging-sensor")
            .expect("sensor command window")
            .options)
    }

    fn gate(register: i64, mask: i64) -> toml::Value {
        toml::Value::Table(toml::Table::from_iter([
            ("register".to_string(), toml::Value::Integer(register)),
            ("mask".to_string(), toml::Value::Integer(mask)),
        ]))
    }

    #[test]
    fn the_sensor_window_carries_the_body_sensor_part() {
        for firmware in ["xt5-431", "xh2-530", "xe5-112", "x100vi-131"] {
            let options = sensor(&ResolvedConfiguration::bundled(firmware).unwrap()).unwrap();
            assert_eq!(options.get("read_gate"), Some(&gate(0x59, 4)), "{firmware}");
            assert_eq!(options.get("standby_gate"), Some(&gate(0, 1)), "{firmware}");
            assert_eq!(
                (
                    options["frame_timing_readout"].as_integer(),
                    options["frame_timing_line_ns"].as_integer()
                ),
                (Some(7), Some(6000)),
                "{firmware}"
            );
        }
        // The X-H2S sensor has neither gate and no decoded readout.
        let options = sensor(&ResolvedConfiguration::bundled("xh2s-730").unwrap()).unwrap();
        for key in [
            "read_gate",
            "standby_gate",
            "frame_timing_readout",
            "frame_timing_line_ns",
        ] {
            assert!(!options.contains_key(key), "xh2s {key}");
        }
        let options = sensor(&ResolvedConfiguration::bundled("xm5-130").unwrap()).unwrap();
        assert_eq!(options.get("read_gate"), Some(&gate(0x59, 4)));
        assert_eq!(options.get("standby_gate"), Some(&gate(0, 1)));
        assert!(!options.contains_key("frame_timing_readout"));
    }

    #[test]
    fn a_body_without_exactly_one_sensor_part_refuses_the_sensor_command_block() {
        let mut none = ResolvedConfiguration::bundled("xt5-431").unwrap();
        none.reference
            .parts
            .retain(|part| part.id != "x-trans-5-hr-image-sensor");
        let error = sensor(&none).unwrap_err();
        assert!(error.contains("installs 0 image-sensor parts"), "{error}");

        let mut two = ResolvedConfiguration::bundled("xt5-431").unwrap();
        let mut copy = two
            .reference
            .parts
            .iter()
            .find(|part| part.id == "x-trans-5-hr-image-sensor")
            .unwrap()
            .clone();
        copy.id = "second-image-sensor".into();
        two.reference.parts.push(copy);
        let error = sensor(&two).unwrap_err();
        assert!(error.contains("installs 2 image-sensor parts"), "{error}");
    }
}

#[cfg(test)]
mod gpio_feedback_tests {
    use super::device_windows;
    use fujimod_catalog::reference::ConnectionSignal;
    use fujimod_catalog_resolution::ResolvedConfiguration;

    fn resolved(firmware: &str) -> ResolvedConfiguration {
        ResolvedConfiguration::bundled(firmware).unwrap()
    }

    fn feedback(resolved: &ResolvedConfiguration) -> Result<serde_json::Value, String> {
        let windows = device_windows(resolved, &super::catalog_dram(resolved))?;
        let gpio = windows.iter().find(|window| window.kind == "gpio").unwrap();
        Ok(serde_json::to_value(gpio.options.get("feedback")).unwrap())
    }

    #[test]
    fn declared_bodies_feed_the_sensor_power_enable_back_to_its_pad() {
        for firmware in ["x100vi-131", "xt5-431"] {
            let wire = &feedback(&resolved(firmware)).unwrap()[0];
            assert_eq!(
                (
                    &wire["port"],
                    &wire["register"],
                    &wire["bit"],
                    &wire["input_port"],
                    &wire["input_pin"]
                ),
                (&4.into(), &0.into(), &4.into(), &4.into(), &4.into()),
                "{firmware}"
            );
        }
    }

    #[test]
    fn a_body_without_feedback_leaves_the_gpio_window_unchanged() {
        let mut resolved = resolved("xt5-431");
        resolved
            .reference
            .body
            .hardware
            .as_mut()
            .unwrap()
            .gpio_feedback
            .clear();
        assert_eq!(feedback(&resolved).unwrap(), serde_json::Value::Null);
    }

    #[test]
    fn inverting_and_non_gpio_feedback_wires_are_refused() {
        let mut inverted = resolved("xt5-431");
        let input = inverted
            .reference
            .body
            .hardware
            .as_ref()
            .unwrap()
            .gpio_feedback[0]
            .input_connection_id
            .clone();
        let connection = inverted
            .reference
            .body
            .connections
            .iter_mut()
            .find(|c| c.id == input)
            .unwrap();
        connection.signals = vec![ConnectionSignal::Gpio {
            port: 4,
            pin: 4,
            active_level: false,
        }];
        let error = feedback(&inverted).err().unwrap();
        assert!(error.contains("inverting GPIO feedback"), "{error}");

        let mut analog = resolved("xt5-431");
        let connection = analog
            .reference
            .body
            .connections
            .iter_mut()
            .find(|c| c.id == input)
            .unwrap();
        connection.signals = vec![ConnectionSignal::Adc {
            channel: 0,
            threshold: None,
        }];
        let error = feedback(&analog).err().unwrap();
        assert!(error.contains("exactly one GPIO pin"), "{error}");
    }

    fn sd_detect(resolved: &ResolvedConfiguration) -> Result<serde_json::Value, String> {
        let windows = device_windows(resolved, &super::catalog_dram(resolved))?;
        let host = windows
            .iter()
            .find(|window| window.kind == "sdhci" && window.base == 0xff9b_0000)
            .unwrap();
        Ok(serde_json::to_value(host.options.get("detect_gpio")).unwrap())
    }

    fn power_programmer(resolved: &ResolvedConfiguration) -> Result<serde_json::Value, String> {
        let windows = device_windows(resolved, &super::catalog_dram(resolved))?;
        let endpoints = windows.iter().flat_map(|window| {
            let wire = window.options.get("wire").and_then(toml::Value::as_table);
            window
                .options
                .get("endpoints")
                .or_else(|| wire?.get("endpoints"))
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
        });
        let programmers: Vec<_> = endpoints
            .filter_map(|endpoint| endpoint.get("part")?.get("programmer"))
            .collect();
        Ok(serde_json::to_value(programmers).unwrap())
    }

    #[test]
    fn a_body_card_slot_wires_its_hosts_card_detect_pad() {
        assert_eq!(
            sd_detect(&resolved("xe5-112")).unwrap(),
            serde_json::json!({"port": 11, "pin": 12, "active_high": false})
        );
        // A body that declares no slot leaves the host without a detect pad.
        assert_eq!(
            sd_detect(&resolved("xt5-431")).unwrap(),
            serde_json::Value::Null
        );
        let mut analog = resolved("xe5-112");
        analog
            .reference
            .body
            .connections
            .iter_mut()
            .find(|connection| connection.id == "sd-card-detect")
            .unwrap()
            .signals = vec![ConnectionSignal::Adc {
            channel: 0,
            threshold: None,
        }];
        let error = sd_detect(&analog).unwrap_err();
        assert!(error.contains("not one GPIO line"), "{error}");
    }

    #[test]
    fn a_power_controller_attachment_with_a_mode_pad_gets_a_programmer() {
        assert_eq!(
            power_programmer(&resolved("xe5-112")).unwrap(),
            serde_json::json!([{
                "mode": {"port": 8, "pin": 1, "active_high": true},
                "flash_bytes": 0x1_0000,
            }])
        );
        assert_eq!(
            power_programmer(&resolved("xt5-431")).unwrap(),
            serde_json::json!([])
        );
        // Only a power controller has a programming mode.
        let mut misplaced = resolved("xe5-112");
        let attachments = &mut misplaced
            .reference
            .body
            .hardware
            .as_mut()
            .unwrap()
            .serial_attachments;
        let (mode, bytes) = attachments
            .iter_mut()
            .find(|attachment| attachment.part_id == "power-controller")
            .map(|attachment| {
                (
                    attachment.programming_mode_connection_id.take(),
                    attachment.programming_flash_bytes.take(),
                )
            })
            .unwrap();
        let other = attachments
            .iter_mut()
            .find(|attachment| attachment.part_id == "serial-command-8")
            .unwrap();
        other.programming_mode_connection_id = mode;
        other.programming_flash_bytes = bytes;
        let error = power_programmer(&misplaced).unwrap_err();
        assert!(error.contains("only a power controller"), "{error}");
    }
}

#[cfg(test)]
mod firmware_entry_tests {
    use super::startup_from_profile;
    use fujimod_catalog::firmware::{Entry, EntryLevel};
    use fujimod_catalog_configuration::RunEngine;
    use fujimod_catalog_resolution::ResolvedConfiguration;
    use fujimod_emulator::board::{
        bus::{Bus, Mode},
        factory,
    };
    use fujimod_emulator_cpu::{Register, timeline::Ratio};
    use std::collections::BTreeMap;

    #[test]
    fn el3t_catalog_entry_initializes_native_cpu_and_refuses_wrong_mode() {
        let resolved = ResolvedConfiguration::bundled("xt5-431").unwrap();
        let mut profile = resolved
            .reference
            .firmware
            .layouts
            .as_ref()
            .unwrap()
            .profiles
            .get("xt5-431")
            .unwrap()
            .clone();
        let cpu = &mut profile.cpu.as_mut().unwrap()[0];
        cpu.pc = 0x1234_0000;
        cpu.cpsr = None;
        cpu.regs = None;
        cpu.entry = Some(Entry {
            level: EntryLevel::El3t,
            pstate: 0x3cc,
            regs: BTreeMap::from([("x0".into(), 0x1122), ("x1".into(), 0x3344)]),
        });
        cpu.sysregs.sp_el0 = 0x5566_7788;
        cpu.validate().unwrap();
        let startup = startup_from_profile(&resolved, &profile, 1).unwrap();
        let start = startup.processors.into_iter().next().unwrap();
        let graph = factory::Graph {
            processors: vec![factory::Processor {
                cpu_backend: Default::default(),
                model: fujimod_emulator_cpu::Model::ArmV8A,
                registers: start.registers,
                released: true,
                parked_until_sgi: false,
                mprofile: None,
                rprofile: None,
            }],
            ram: vec![],
            dma_mappings: vec![],
            decoders: vec![],
            mappings: vec![],
            peripherals: vec![],
            instructions_per_tick: Ratio::new(1, 1).unwrap(),
        };
        let board = graph
            .build(
                factory::Options::new(RunEngine::Unicorn)
                    .with_translation_cache_size_bytes(1 << 20),
                Bus::new(Mode::Strict),
                None,
            )
            .unwrap();
        assert_eq!(board.read_register(0, Register::Pc).unwrap(), 0x1234_0000);
        assert_eq!(board.read_register(0, Register::Pstate).unwrap(), 0x3cc);
        assert_eq!(board.read_register(0, Register::X(0)).unwrap(), 0x1122);
        assert_eq!(board.read_register(0, Register::X(1)).unwrap(), 0x3344);
        assert_eq!(
            board
                .read_register(
                    0,
                    Register::Arm64System(
                        fujimod_emulator_cpu::arm64::system_register("SP_EL0").unwrap()
                    )
                )
                .unwrap(),
            0x5566_7788
        );

        profile.cpu.as_mut().unwrap()[0]
            .entry
            .as_mut()
            .unwrap()
            .pstate = 0x3c5;
        let error = startup_from_profile(&resolved, &profile, 1)
            .err()
            .expect("wrong-mode entry must refuse before Board construction");
        assert!(error.contains("does not match declared level"), "{error}");
    }
}
