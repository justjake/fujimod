//! Caller-side construction of the X100VI run on FujiMod's standard board.
//!
//! Firmware paths, preloads, device options and resistor-key geometry remain
//! inputs owned by this application. CPU execution and device scheduling are
//! delegated to FujiMod's catalog-resolved XP5 session.

use super::{
    manifest::BootState,
    firmware::Firmware,
    rom_start::{self, Entry, RomStart},
};
use fujimod_catalog::reference::{ProcessorModel, RegionSize};
use fujimod_catalog_configuration::{RunInputs, RunScene};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator::board::{self, bus::Mode};
use fujimod_emulator_cpu::{Register, arm64};
use fujimod_emulator_session::Session;
use fujimod_emulator_board_graph as graph;
use fujimod_emulator_devices::{graph::DeviceWindow, xp5::panel_clocks};
use fujimod_emulator_xp5_board::{
    board as xp5_board,
    boot::{self as xp5_boot, MemoryImage},
    session::Inputs,
};
use fujimod_emulator_xp5_io::mailbox;
use fujimod_emulator_xp5_storage::flash_storage;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
};

pub(crate) struct Opened {
    pub session: Session,
    pub inputs: Inputs,
    pub outputs: crate::output::Outputs,
    pub ticks_per_second: u64,
    /// Instructions per scheduler turn: the run configuration's
    /// `execution.quantum` unless the caller overrides it for this run.
    pub quantum: usize,
    pub read_limit: usize,
}

fn merge_image(
    images: &mut Vec<MemoryImage>,
    mut incoming: MemoryImage,
) -> Result<(), String> {
    let mut index = 0;
    while index < images.len() {
        let old = &images[index];
        let old_end = old.address + old.bytes.len() as u64;
        let incoming_end = incoming.address + incoming.bytes.len() as u64;
        if incoming.address >= old_end || old.address >= incoming_end {
            index += 1;
            continue;
        }
        let base = incoming.address.min(old.address);
        let end = incoming_end.max(old_end);
        let size =
            usize::try_from(end - base).map_err(|_| "memory image merge exceeds host space")?;
        if size > 512 << 20 {
            return Err("merged XP5 memory image exceeds 512 MiB".into());
        }
        let mut bytes = vec![0; size];
        let old = images.remove(index);
        let old_start = usize::try_from(old.address - base).map_err(|_| "invalid memory image")?;
        bytes[old_start..old_start + old.bytes.len()].copy_from_slice(&old.bytes);
        let incoming_start =
            usize::try_from(incoming.address - base).map_err(|_| "invalid memory image")?;
        bytes[incoming_start..incoming_start + incoming.bytes.len()]
            .copy_from_slice(&incoming.bytes);
        incoming = MemoryImage {
            address: base,
            bytes,
        };
        index = 0;
    }
    images.push(incoming);
    images.sort_by_key(|image| image.address);
    Ok(())
}

fn initial_images(
    firmware: &Firmware,
    state: &BootState,
) -> Result<Vec<MemoryImage>, String> {
    let mut images = Vec::new();
    if state.load_firmware {
        for region in &firmware.regions {
            merge_image(
                &mut images,
                MemoryImage {
                    address: region.address,
                    bytes: region.bytes.clone(),
                },
            )?;
        }
    }
    for preload in &state.preload {
        merge_image(
            &mut images,
            MemoryImage {
                address: preload.address,
                bytes: super::manifest::preload_bytes(preload, firmware)?,
            },
        )?;
    }
    Ok(images)
}

fn devices_and_resources(
    state: &BootState,
    firmware: &Firmware,
    controls: Option<fujimod_emulator_parts_controls::key_input::ControlsOptions>,
    run_directory: Option<&Path>,
    outputs: &crate::output::Outputs,
    viewer_keys: Option<crate::frame_viewer::KeyHandler>,
    mut windows: Vec<DeviceWindow>,
    assets: &BTreeMap<String, Vec<u8>>,
    card_image: Option<&fujimod_emulator_devices::parts::sd_card::Card>,
    companion_core: Option<usize>,
) -> Result<
    (
        Vec<DeviceWindow>,
        xp5_board::Resources,
        Option<fujimod_emulator_parts_controls::key_input::ControlsOptions>,
    ),
    String,
> {
    let pins = fujimod_emulator_devices::xp5::pins::Pins::shared();
    let companion = if state.device.iter().any(|window| window.kind == "interprocessor-mailbox") {
        Some(xp5_board::Companion {
            pins: mailbox::Pins {
                companion: companion_core.ok_or("mailbox requires a Cortex-M4 core")?,
                request: 55,
                control: 58,
                reply: 141,
                status: 142,
                fault: 143,
            },
            channel: mailbox::Channel::default(),
        })
    } else {
        None
    };
    let mut resources = xp5_board::Resources {
        sd: xp5_board::sd::resolve(state.sd.clone(), assets)?,
        // Replaced by the declared key model when the devices are assembled.
        controls: Arc::new(fujimod_emulator_parts_controls::absent::AbsentControls),
        pins,
        clocks: panel_clocks::Clocks::new(),
        companion,
        storage: None,
        displays: BTreeMap::new(),
        pwm: Default::default(),
    };
    if let Some(card) = card_image {
        // A raw card image (`--card-image`) replaces the formatted volume of slot 1, in the ROM
        // start and in the loader start.
        let slot = resources
            .sd
            .get_mut(&0xff9b_0000)
            .ok_or("a card image needs the SD slot at 0xff9b0000")?;
        slot.card = Some(card.clone());
    }
    // With the viewer on, every display bank publishes into the primary display's directory,
    // which is the one directory the viewer serves (latest-bank0.png and latest-bank1.png).
    let viewer_directory = windows.iter()
        .find(|window| window.kind == "display" && window.options.contains_key("viewer_port"))
        .map(|window| window.base);
    for window in &mut windows {
        match window.kind.as_str() {
            "adc" => {
                window.options.remove("resistor_keys");
            }
            "interprocessor-mailbox" => window.options.clear(),
            "flash-storage" => {
                let number = |key: &str| {
                    window
                        .options
                        .get(key)
                        .and_then(toml::Value::as_integer)
                        .and_then(|value| u64::try_from(value).ok())
                        .ok_or_else(|| format!("flash storage requires {key}"))
                };
                let origin =
                    usize::try_from(number("dat_origin")?).map_err(|error| error.to_string())?;
                let xor = u8::try_from(number("xor")?).map_err(|error| error.to_string())?;
                let image = firmware.dat_payload(origin, xor)?;
                resources.storage = Some(flash_storage::Medium::Nor(Arc::new(Mutex::new(
                    flash_storage::Storage::new(image)?,
                ))));
                window.options.remove("dat_origin");
                window.options.remove("xor");
            }
            "display" => {
                let mut options = window.options.clone();
                if let Some(directory) = run_directory {
                    options.entry("frame_dir").or_insert_with(|| {
                        let base = viewer_directory.unwrap_or(window.base);
                        toml::Value::String(directory.join(format!("display-{base:x}"))
                            .to_string_lossy().into_owned())
                    });
                } else {
                    options.remove("frame_dir");
                }
                let display = super::display_output::configure(outputs, window.base, &options, viewer_keys.clone())?;
                resources.displays.insert(window.base, display);
                window.options.clear();
            }
            _ => {}
        }
    }
    Ok((windows, resources, controls))
}

/// Write the flash image into the RAM that backs the CPU's flash aperture in a ROM start.
fn shadow_flash_aperture(
    storage: Option<&flash_storage::Medium>,
    session: &mut Session,
) -> Result<(), String> {
    let Some(flash_storage::Medium::Nor(storage)) = storage else {
        return Err("ROM start needs the NOR flash image to back the aperture".into());
    };
    let mut storage = storage.lock().map_err(|_| "flash storage lock poisoned")?;
    let bytes = storage.read(0, FLASH_APERTURE_SIZE)?;
    // The seeds are already in `bytes`; only later changes are new.
    storage.take_dirty_pages();
    session.board.write_physical(FLASH_APERTURE_BASE, &bytes)
}

/// Copy the pages the stock firmware programmed or erased since the last call into the aperture
/// memory, so a CPU read through the aperture sees what a controller read sees. Called between
/// run slices (1 ms of guest time), so a read in the slice that changes a page can still see the
/// old bytes. The stock boot does erase and program flash (about 1.6 MiB at `0x6d60000` by the
/// first-start screens, in the hand-off boot too), so a ROM start cannot refuse program and erase.
fn sync_flash_aperture(
    storage: &flash_storage::Medium,
    board: &board::Board,
) -> Result<(), String> {
    let flash_storage::Medium::Nor(storage) = storage else {
        return Err("ROM start needs the NOR flash image to back the aperture".into());
    };
    let mut storage = storage.lock().map_err(|_| "flash storage lock poisoned")?;
    for page in storage.take_dirty_pages() {
        let offset = page * 4096;
        let bytes = storage.read(offset, 4096)?;
        board.write_physical(FLASH_APERTURE_BASE + offset, &bytes)?;
    }
    Ok(())
}

/// CPU aperture granule: the X-T5 runner covers bus-routed register space in whole 16 KiB pages.
const APERTURE_GRANULE: u64 = 0x4000;

#[derive(Debug, PartialEq, Eq)]
enum RamPlan {
    /// The catalog's RAM already backs the whole region.
    Covered,
    /// The region becomes RAM of this run.
    Promote,
}

/// Decide what a manifest `[[ram]]` region means next to the catalog's RAM (`(base, size)` pairs).
/// The DRAM window is `dram_base..dram_limit`.
fn plan_caller_ram(
    base: u64,
    size: u64,
    catalog: &[(u64, u64)],
    dram_base: u64,
    dram_limit: u64,
) -> Result<RamPlan, String> {
    let end = base.checked_add(size).ok_or("caller RAM region overflows")?;
    if size == 0 {
        return Err(format!("caller RAM region {base:#x}+{size:#x} is empty"));
    }
    let range = |&(start, len): &(u64, u64)| (start, start.saturating_add(len));
    if catalog
        .iter()
        .map(range)
        .any(|(start, stop)| start <= base && end <= stop)
    {
        return Ok(RamPlan::Covered);
    }
    if catalog
        .iter()
        .map(range)
        .any(|(start, stop)| start < end && base < stop)
    {
        return Err(format!(
            "caller RAM region {base:#x}+{size:#x} partly overlaps catalog RAM"
        ));
    }
    if base >= dram_limit || end <= dram_base {
        return Ok(RamPlan::Promote);
    }
    Err(format!(
        "caller RAM region {base:#x}+{size:#x} has no physical backing inside the DRAM window \
         {dram_base:#x}..{dram_limit:#x}; grow DRAM through the manifest [dram] table"
    ))
}

/// Decide whether a manifest device window needs a new CPU aperture. `occupied` holds the
/// `(base, size)` ranges the core already sees as apertures, RAM or RAM mappings. Returns the
/// page-rounded `(base, size)` to add, or `None` when the window is fully inside occupied ranges.
fn plan_aperture(
    base: u64,
    size: u64,
    occupied: &[(u64, u64)],
) -> Result<Option<(u64, u64)>, String> {
    if size == 0 {
        return Err("window is empty".into());
    }
    let window_end = base.checked_add(size).ok_or("window overflows")?;
    let mut ranges: Vec<(u64, u64)> = occupied
        .iter()
        .map(|&(start, len)| (start, start.saturating_add(len)))
        .filter(|&(start, stop)| start < stop)
        .collect();
    ranges.sort_unstable();
    // Fully inside the union of the occupied ranges: walk the window left to right.
    let mut cursor = base;
    for &(start, stop) in &ranges {
        if start <= cursor && cursor < stop {
            cursor = stop;
        }
        if cursor >= window_end {
            return Ok(None);
        }
    }
    let first = base & !(APERTURE_GRANULE - 1);
    let last = window_end
        .checked_add(APERTURE_GRANULE - 1)
        .ok_or("window page range overflows")?
        & !(APERTURE_GRANULE - 1);
    if let Some(&(start, stop)) = ranges.iter().find(|&&(start, stop)| start < last && first < stop) {
        return Err(format!(
            "window partly overlaps existing aperture or RAM {start:#x}..{stop:#x} \
             (its pages are {first:#x}..{last:#x})"
        ));
    }
    Ok(Some((first, last - first)))
}

const FLASH_APERTURE_BASE: u64 = 0xf000_0000;
const FLASH_APERTURE_SIZE: usize = 0x0800_0000;

/// Remove `base..base+size` (physical) from the register apertures, keeping the rest.
fn carve_out(
    peripherals: Vec<board::factory::Peripheral>,
    base: u64,
    size: u64,
) -> Vec<board::factory::Peripheral> {
    let end = base + size;
    let mut kept = Vec::new();
    for peripheral in peripherals {
        let physical_end = peripheral.physical + peripheral.size as u64;
        if peripheral.physical >= end || physical_end <= base {
            kept.push(peripheral);
            continue;
        }
        for (start, stop) in [
            (peripheral.physical, base.min(physical_end)),
            (end.max(peripheral.physical), physical_end),
        ] {
            if start < stop {
                kept.push(board::factory::Peripheral {
                    core: peripheral.core,
                    address: peripheral.address + (start - peripheral.physical),
                    physical: start,
                    size: (stop - start) as usize,
                });
            }
        }
    }
    kept
}

/// The boot state of a ROM start: nothing of the stock firmware is preloaded, because the ROM
/// and stage one bring it up from the emulated internal flash; the boot-source straps are an
/// explicit input; the card carries only the caller's stage-one files.
fn rom_boot_state(state: &BootState, rom: &RomStart) -> Result<BootState, String> {
    let mut state = state.clone();
    state.load_firmware = false;
    state.preload.clear();
    let mut options = toml::Table::new();
    options.insert("source_straps".into(), toml::Value::Integer(rom.source_straps.into()));
    options.insert("initial_control".into(), toml::Value::Integer(0));
    options.insert("initial_primary_control".into(), toml::Value::Integer(0));
    state.device.push(DeviceWindow {
        base: 0xff90_e000,
        size: 4,
        kind: "boot-control".into(),
        options,
    });
    let mut fragment = rom_start::Fragment::bundled()?;
    let slot_base = match &rom.entry {
        Entry::MaskRom(_) => Some(0xff9b_0000),
        Entry::CardHandoff(handoff) => {
            fragment.power_controller_registers.clone_from(&handoff.power_controller_registers);
            Some(handoff.slot)
        }
        Entry::SetupHandoff(_) => {
            // The setup start runs from internal flash: no SD host options, no card, and the
            // power controller's registers are the camera's own.
            fragment.power_controller_registers.clear();
            fragment.window_options.clear();
            None
        }
    };
    state.ram.extend(fragment.ram);
    for window in fragment.device {
        // A bring-up stub page the manifest already owns (the X-T5's memory-controller pages
        // 0xfaf27000 and 0xfaf37000) keeps the manifest's registers and gains the ROM-start
        // registers it does not list, instead of a second device on the same page.
        let owned = state.device.iter_mut().find(|existing| {
            existing.base == window.base
                && existing.size == window.size
                && existing.kind == "bringup-stub"
                && window.kind == "bringup-stub"
        });
        let Some(existing) = owned else {
            state.device.push(window);
            continue;
        };
        let offset = |register: &toml::Value| register.get("offset").and_then(toml::Value::as_integer);
        let added = window.options.get("registers").and_then(toml::Value::as_array).cloned().unwrap_or_default();
        let registers = existing
            .options
            .entry("registers")
            .or_insert_with(|| toml::Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or("bring-up stub registers must be an array")?;
        for register in added {
            if !registers.iter().any(|listed| offset(listed) == offset(&register)) {
                registers.push(register);
            }
        }
    }
    for options in fragment.window_options {
        let window = state
            .device
            .iter_mut()
            .find(|window| window.base == options.base)
            .ok_or_else(|| format!("ROM start sets options on absent window {:#x}", options.base))?;
        window.options.extend(options.set);
    }
    if !fragment.power_controller_registers.is_empty() {
        // The power controller's reset registers: set on the existing endpoint, not duplicated.
        let registers = toml::Value::Table(
            fragment
                .power_controller_registers
                .iter()
                .map(|(address, value)| (address.clone(), toml::Value::Integer(i64::from(*value))))
                .collect(),
        );
        let mut found = false;
        for window in state.device.iter_mut().filter(|window| window.kind == "spi-controller") {
            let Some(toml::Value::Array(endpoints)) = window.options.get_mut("endpoints") else {
                continue;
            };
            for endpoint in endpoints {
                let part = endpoint.get_mut("part").and_then(toml::Value::as_table_mut);
                if let Some(part) = part.filter(|part| part.get("kind").and_then(toml::Value::as_str) == Some("power-controller")) {
                    part.insert("registers".into(), registers.clone());
                    found = true;
                }
            }
        }
        if !found {
            return Err("ROM start found no power-controller endpoint to set registers on".into());
        }
    }
    let Some(slot_base) = slot_base else {
        return Ok(state);
    };
    let slot = state
        .sd
        .iter_mut()
        .find(|slot| slot.base == slot_base)
        .ok_or("early startup needs the SD card slot declared by its board facts")?;
    slot.present = rom.card_present;
    slot.files = rom.card_files.iter().map(|(name, _)| (name.clone(), name.clone())).collect();
    // The card model keeps only nonzero sectors and bounds them (the manifest's default holds a
    // stage file). A startup card also carries the RAM image (about 105 MiB), so the bound grows
    // with the files plus room for the FAT and directory sectors, up to the model's 1 GiB limit.
    let file_sectors: usize = rom.card_files.iter().map(|(_, bytes)| bytes.len().div_ceil(512)).sum();
    slot.format.max_dirty_sectors = slot.format.max_dirty_sectors.max((file_sectors + 8192).min(1 << 21));
    Ok(state)
}

/// Open a caller-described XP5 input set through FujiMod's shared XP5 board, optionally resetting
/// the A53 cores into the boot ROM.
pub(crate) fn open_from(
    firmware: &Firmware,
    state: &BootState,
    resolved: &ResolvedConfiguration,
    scene_override: Option<&Path>,
    run_directory: Option<&Path>,
    mode: Mode,
    quantum_override: Option<usize>,
    rom: Option<&RomStart>,
    loader_card: Option<&super::loader_card::LoaderCard>,
    viewer_keys: Option<crate::frame_viewer::KeyHandler>,
    profile: Option<&super::profile::Placed>,
) -> Result<Opened, String> {
    let mask_rom = rom.is_some_and(|start| matches!(start.entry, Entry::MaskRom(_)));
    let rom_state;
    let state = match rom {
        Some(rom) => {
            rom_state = rom_boot_state(state, rom)?;
            &rom_state
        }
        None => state,
    };
    if resolved.configuration.firmware_id != firmware.id || state.firmware != firmware.id {
        return Err("run configuration, firmware input and boot manifest differ".into());
    }
    let run = resolved
        .configuration
        .execution
        .ok_or("run configuration has no execution policy")?;
    let quantum = quantum_override.unwrap_or(run.quantum);
    if quantum == 0 {
        return Err("processor quantum must be positive".into());
    }
    let preset = resolved
        .configuration
        .preset
        .as_ref()
        .ok_or("run configuration has no hardware preset")?;
    let firmware_profile_id = preset
        .firmware_profile_id
        .as_deref()
        .ok_or("run configuration has no firmware startup profile")?;
    let firmware_profile = resolved
        .reference
        .firmware
        .layouts
        .as_ref()
        .and_then(|layouts| layouts.profiles.get(firmware_profile_id))
        .ok_or_else(|| format!("firmware has no startup profile {firmware_profile_id}"))?;
    let ticks_per_second = match resolved.configuration.inputs.as_ref() {
        Some(RunInputs::Xp5(inputs)) => inputs.ticks_per_second,
        _ => return Err("X100VI run configuration has no XP5 inputs".into()),
    };
    let mut startup = if rom.is_some() {
        // Reset state: the interrupt controller and CPU interfaces start unprogrammed.
        let mut profile = firmware_profile.clone();
        profile.gic_initial = None;
        profile.gic_redistributor_initial = None;
        for cpu in profile.cpu.iter_mut().flatten() {
            cpu.sysregs.icc_pmr_el1 = 0;
            cpu.sysregs.icc_igrpen0_el1 = 0;
            cpu.sysregs.icc_igrpen1_el1 = 0;
        }
        xp5_boot::startup_from_profile(&resolved, &profile, ticks_per_second)?
    } else {
        xp5_boot::startup_from_profile(&resolved, firmware_profile, ticks_per_second)?
    };
    if let Some(rom) = rom {
        let mpidr = Register::Arm64System(arm64::system_register("MPIDR_EL1")?);
        let mut primary = true;
        for start in &mut startup.processors {
            let is_a53 = resolved
                .reference
                .soc
                .processors
                .iter()
                .any(|p| p.id == start.processor_id && p.model == ProcessorModel::CortexA53);
            if !is_a53 {
                continue;
            }
            let affinity = start.registers.iter().find(|(r, _)| *r == mpidr).map(|(_, v)| *v);
            match &rom.entry {
                Entry::MaskRom(_) => {
                    start.registers = vec![
                        (Register::Pc, rom_start::ROM_BASE),
                        (Register::Pstate, rom_start::ROM_PSTATE),
                    ];
                    // The ROM parks the secondary cores after reset.
                    start.released = true;
                }
                Entry::CardHandoff(handoff) => {
                    start.registers = vec![
                        (Register::Pc, handoff.pc),
                        (Register::Pstate, rom_start::ROM_PSTATE),
                        (Register::Sp, handoff.sp),
                    ];
                    start.released = primary;
                    primary = false;
                }
                Entry::SetupHandoff(handoff) => {
                    start.registers = vec![
                        (Register::Pc, handoff.pc),
                        (Register::Pstate, rom_start::ROM_PSTATE),
                        (Register::Sp, handoff.sp),
                    ];
                    start.released = primary;
                    primary = false;
                }
            }
            start.registers.extend(affinity.map(|v| (mpidr, v)));
        }
    }
    // The manifest may extend the catalog body's DRAM map to what the firmware uses.
    let mut overridden = None;
    let dram_limit = match &state.dram {
        Some(dram) => {
            dram.validate(resolved.configuration.dram_address_space.is_some())?;
            let reference = overridden.insert(resolved.reference.clone());
            let drain = reference
                .body
                .hardware
                .as_mut()
                .and_then(|hardware| hardware.bus_drain.as_mut())
                .ok_or("manifest moves the bus drain, which the body does not declare")?;
            drain.address = dram.bus_drain;
            dram.limit
        }
        None => resolved.dram_address_space_limit_bytes()?,
    };
    let reference = overridden.as_ref().unwrap_or(&resolved.reference);
    let mut execution = graph::assemble_with_starts(
        reference,
        dram_limit,
        run.instructions_per_tick,
        Some(ticks_per_second),
        &startup.processors,
    )?;
    // The XP5 catalog currently declares the companion's translated views;
    // caller-owned application RAM remains identity-mapped by this run input.
    // A caller RAM region the catalog already backs is skipped. One at or above the run's DRAM
    // limit (the DDR PHY register and firmware space) or outside the DRAM window becomes RAM of
    // this run. Anything else is refused: DRAM growth goes through the validated `[dram]` table.
    let dram_base = resolved
        .reference
        .soc
        .memory_regions
        .iter()
        .find(|region| matches!(region.size, RegionSize::BodyDram))
        .map(|region| region.base)
        .ok_or("SoC declares no body DRAM region")?;
    for region in &state.ram {
        let size = usize::try_from(region.size)
            .map_err(|_| "caller RAM region exceeds host address space")?;
        let catalog: Vec<(u64, u64)> = execution
            .ram
            .iter()
            .map(|ram| (ram.base, ram.size as u64))
            .collect();
        match plan_caller_ram(region.base, region.size, &catalog, dram_base, dram_limit)? {
            RamPlan::Covered => {}
            RamPlan::Promote => execution.ram.push(board::factory::Ram { base: region.base, size }),
        }
        // Caller RAM wins over a SoC register aperture page under it (the X-T5 maps DRAM channel 2
        // from 0x80000000, where the SoC profile declares a 4-byte peripheral window). Device windows
        // are separate and still refuse an overlap.
        execution.peripherals =
            carve_out(std::mem::take(&mut execution.peripherals), region.base, region.size);
    }
    for (core, processor) in resolved.reference.soc.processors.iter().enumerate() {
        if processor.model != ProcessorModel::CortexA53 {
            continue;
        }
        for region in &state.ram {
            let size = usize::try_from(region.size)
                .map_err(|_| "caller RAM region exceeds host address space")?;
            execution.mappings.push(board::factory::MemoryMap {
                core,
                address: region.base,
                physical: region.base,
                size,
                permissions: 7,
            });
        }
    }
    if rom.is_some() {
        // The catalog declares the ROM's pages as part of its last register window and the flash
        // aperture as a register window. A ROM start needs both as memory for the A53 cores: the
        // ROM is read-only code, and stock setup executes flash veneers through the aperture, which
        // a bus device cannot serve as instruction fetch. The aperture's bytes are written after
        // the board opens (`shadow_flash_aperture`) and refreshed for programmed and erased pages
        // between slices (`sync_flash_aperture`). The bus still registers the flash-storage
        // aperture device, and the controller and its DMA read the storage directly. The bus
        // memory mapping is read-only (host loading bypasses it), so no DMA master can overwrite
        // the ROM or the shadow. Cut by physical range: the M4's local addresses alias the same
        // windows, and the M4 gets no mapping for either window.
        let entry_memory = if mask_rom {
            (rom_start::ROM_BASE, rom_start::ROM_SIZE as u64, 5)
        } else {
            let boot_sram = reference.soc.memory_regions.iter()
                .find(|region| region.id == "boot-sram")
                .ok_or("SD card handoff requires the SoC boot SRAM")?;
            let RegionSize::Bytes { bytes } = boot_sram.size else {
                return Err("SD card handoff requires a fixed SRAM extent".into());
            };
            (boot_sram.base, bytes, 7)
        };
        for (base, bytes, permissions) in [
            entry_memory,
            (FLASH_APERTURE_BASE, FLASH_APERTURE_SIZE as u64, 5),
        ] {
            execution.peripherals = carve_out(std::mem::take(&mut execution.peripherals), base, bytes);
            let size = usize::try_from(bytes).map_err(|_| "region exceeds host address space")?;
            if !execution.ram.iter().any(|ram| ram.base == base && ram.size == size) {
                execution.ram.push(board::factory::Ram { base, size });
            }
            execution.dma_mappings.push(board::memory::BusMapping {
                permissions: permissions & 3,
                ..board::memory::BusMapping::ram(base, size)
            });
            for (core, processor) in resolved.reference.soc.processors.iter().enumerate() {
                if processor.model != ProcessorModel::CortexA53 {
                    continue;
                }
                execution.mappings.push(board::factory::MemoryMap {
                    core,
                    address: base,
                    physical: base,
                    size,
                    permissions,
                });
            }
        }
    }
    for region in &state.ram {
        execution.dma_mappings.push(board::memory::BusMapping::ram(
            region.base,
            usize::try_from(region.size)
                .map_err(|_| "caller RAM region exceeds host address space")?,
        ));
    }
    // A device window the manifest declares outside the SoC blocks (the DDR PHY pages) needs a
    // CPU aperture too, or the guest's access never reaches the bus. Cover it with whole 16 KiB
    // pages, as the X-T5 runner does for the register space it routes to the bus. A window fully
    // inside existing apertures or RAM needs none; a partial overlap with a block aperture, a RAM
    // mapping or an aperture added here is an error. This loop covers the manifest's
    // `state.device` windows only, not the windows `apply_run_scene` adds to `windows`.
    for (core, processor) in resolved.reference.soc.processors.iter().enumerate() {
        if processor.model != ProcessorModel::CortexA53 {
            continue;
        }
        for window in &state.device {
            let mut occupied: Vec<(u64, u64)> = Vec::new();
            for existing in execution.peripherals.iter().filter(|existing| existing.core == core) {
                occupied.push((existing.address, existing.size as u64));
            }
            for ram in &execution.ram {
                occupied.push((ram.base, ram.size as u64));
            }
            for mapping in execution.mappings.iter().filter(|mapping| mapping.core == core) {
                occupied.push((mapping.address, mapping.size as u64));
            }
            let planned = plan_aperture(window.base, window.size as u64, &occupied).map_err(
                |error| format!("device window {} at {:#x}: {error}", window.kind, window.base),
            )?;
            if let Some((start, size)) = planned {
                execution.peripherals.push(board::factory::Peripheral {
                    core,
                    address: start,
                    physical: start,
                    size: usize::try_from(size)
                        .map_err(|_| "device aperture exceeds host address space")?,
                });
            }
        }
    }
    // The boot-control latch the ROM reads and writes outside the manifest's windows.
    if rom.is_some() {
        for (core, processor) in resolved.reference.soc.processors.iter().enumerate() {
            if processor.model != ProcessorModel::CortexA53 {
                continue;
            }
            let mut occupied: Vec<(u64, u64)> = Vec::new();
            for existing in execution.peripherals.iter().filter(|existing| existing.core == core) {
                occupied.push((existing.address, existing.size as u64));
            }
            for ram in &execution.ram {
                occupied.push((ram.base, ram.size as u64));
            }
            for mapping in execution.mappings.iter().filter(|mapping| mapping.core == core) {
                occupied.push((mapping.address, mapping.size as u64));
            }
            if let Some((start, size)) = plan_aperture(0xfa14_0000, 4, &occupied)
                .map_err(|error| format!("boot-control window at 0xfa140000: {error}"))?
            {
                execution.peripherals.push(board::factory::Peripheral {
                    core,
                    address: start,
                    physical: start,
                    size: usize::try_from(size)
                        .map_err(|_| "device aperture exceeds host address space")?,
                });
            }
        }
    }
    // Discovery only: route every register page the manifest does not own to the bus, which
    // shadows and records the access instead of refusing it with an unmapped-memory fault. The
    // X-T5 bring-up runner does the same for its SD-card and ROM-stage starts. Stock setup
    // touches register space (DDR controller and PHY) that the default hand-off never reaches.
    if rom.is_some() && mode == Mode::Discovery {
        const GRANULE: u64 = 0x4000;
        for (core, processor) in resolved.reference.soc.processors.iter().enumerate() {
            if processor.model != ProcessorModel::CortexA53 {
                continue;
            }
            let mut page = 0xf800_0000u64;
            let mut adds: Vec<(u64, u64)> = Vec::new();
            let mut run: Option<u64> = None;
            while page < 0x1_0000_0000 {
                let covered = execution
                    .ram
                    .iter()
                    .any(|r| page < r.base + r.size as u64 && r.base < page + GRANULE)
                    || execution.peripherals.iter().any(|p| {
                        p.core == core && page < p.address + p.size as u64 && p.address < page + GRANULE
                    });
                if covered {
                    if let Some(start) = run.take() {
                        adds.push((start, page));
                    }
                } else if run.is_none() {
                    run = Some(page);
                }
                page += GRANULE;
            }
            if let Some(start) = run {
                adds.push((start, page));
            }
            for (start, end) in adds {
                execution.peripherals.push(board::factory::Peripheral {
                    core,
                    address: start,
                    physical: start,
                    size: (end - start) as usize,
                });
            }
        }
    }
    let interrupts = startup.interrupts;
    let aprofile = startup.aprofile;
    // The body's key model has one source. A body whose catalog record declares
    // ladders gets the ladder-key model built from those ladders and the run's
    // key-free values; otherwise the manifest's ADC window may name the
    // caller-owned resistor-key layout (GPIO pads and two ADC channels). A
    // manifest copy of ladder wiring is refused, and so is a body with both.
    let adc_options = state
        .device
        .iter()
        .find(|window| window.kind == "adc")
        .map(|window| &window.options);
    if adc_options.is_some_and(|options| options.contains_key("ladder_keys")) {
        return Err(
            "ladder keys come from the body's catalog ladders and the run configuration, not the boot manifest"
                .into(),
        );
    }
    let resistor_keys = adc_options
        .and_then(|options| options.get("resistor_keys"))
        .map(|value| serde_json::to_value(value).map_err(|error| error.to_string()))
        .transpose()?;
    let ladder_keys = fujimod_emulator_xp5_board::ladder_controls::ladder_keys(&resolved)?;
    let controls = match (resistor_keys, ladder_keys) {
        (Some(_), Some(_)) => {
            return Err(
                "boot manifest declares resistor keys for a body whose catalog declares ladders".into(),
            );
        }
        (Some(options), None) => Some(
            fujimod_emulator_parts_controls::key_input::ControlsOptions::ResistorKeys(
                serde_json::from_value(options)
                    .map_err(|error| format!("physical resistor keys: {error}"))?,
            ),
        ),
        (None, Some(options)) => Some(
            fujimod_emulator_parts_controls::key_input::ControlsOptions::LadderKeys(options),
        ),
        (None, None) => None,
    };
    let run_scene = match resolved.configuration.scene.as_ref() {
        Some(RunScene::Xp5(scene)) => scene,
        _ => return Err("X100VI run configuration has no XP5 scene".into()),
    };
    let mut windows = state.device.clone();
    xp5_boot::apply_run_scene(&resolved, &mut windows, run_scene, ticks_per_second)?;
    let outputs = crate::output::Outputs::default();
    let (windows, resources, controls) = devices_and_resources(
        state,
        firmware,
        controls,
        run_directory,
        &outputs,
        viewer_keys,
        windows,
        &rom.map(RomStart::assets)
            .or_else(|| loader_card.map(|card| card.assets.clone()))
            .unwrap_or_default(),
        rom.and_then(|rom| rom.card_image.as_ref()).map(|image| &image.card)
            .or_else(|| loader_card.and_then(|card| card.card.as_ref())),
        reference.soc.processors.iter()
            .position(|processor| processor.model == ProcessorModel::CortexM4),
    )?;
    if windows.is_empty() {
        return Err("X100VI caller boot manifest has no device windows".into());
    }
    if let Some(profile) = profile {
        // The user profile's journal records, in the NOR session overlay like any later
        // program, so snapshots carry them.
        let storage = resources.storage.as_ref().ok_or("a user profile needs NOR flash storage")?;
        for (offset, bytes) in &profile.records {
            for (at, byte) in (*offset..).zip(bytes) {
                storage.seed(at, *byte)?;
            }
        }
    }
    let mut images = initial_images(firmware, state)?;
    if let Some(Entry::MaskRom(bytes)) = rom.map(|rom| &rom.entry) {
        images.push(MemoryImage {
            address: rom_start::ROM_BASE,
            bytes: bytes.clone(),
        });
    }
    let run_scene = serde_json::to_value(run_scene).map_err(|error| error.to_string())?;
    let scene = scene_override.map(crate::host::read_scene).transpose()?;
    let mut checkpoint_wiring = state.clone();
    for window in &mut checkpoint_wiring.device {
        // Host frame publication does not change the modeled boot graph.
        window.options.remove("viewer_port");
        window.options.remove("viewer_bind");
    }
    let wiring = serde_json::to_vec(&checkpoint_wiring).map_err(|error| error.to_string())?;
    let wiring_sha256 = format!("{:x}", Sha256::digest(wiring));
    let mut definition = json!({
        "schema": 1,
        "configuration_id": resolved.configuration.id,
        "body_id": resolved.configuration.body_id,
        "firmware_id": resolved.configuration.firmware_id,
        "firmware_profile_id": firmware_profile_id,
        "engine": run.engine,
        "mode": mode.name(),
        "quantum": quantum,
        "execution": {"kind":"xp5"},
        "run_scene": run_scene,
        "startup": resolved.configuration.start,
        "boot_wiring_sha256": wiring_sha256,
        "update_sha256": firmware.source_sha256
    });
    if let (Some(profile), Some(object)) = (profile, definition.as_object_mut()) {
        object.insert("camera_settings".into(), json!(profile.requested));
    }
    // A manifest DRAM override changes the machine, so it is part of the run's identity.
    if let (Some(dram), Some(object)) = (&state.dram, definition.as_object_mut()) {
        object.insert(
            "dram".into(),
            json!({"limit": dram.limit, "bus_drain": dram.bus_drain}),
        );
    }
    let storage = resources.storage.clone();
    let read_limit = execution.ram.iter().try_fold(0usize, |bytes, region| {
        bytes.checked_add(region.size).ok_or("XP5 RAM extent overflows")
    })?;
    let (mut session, inputs) = xp5_boot::open(xp5_boot::OpenOptions {
        definition,
        mode,
        processors: execution,
        windows,
        controls,
        resources,
        engine: run.engine,
        interrupts,
        aprofile,
        images,
        scene,
    })?;
    if let Some(Entry::CardHandoff(handoff)) = rom.map(|rom| &rom.entry) {
        let stage = session.card_read(&format!("{:#x}", handoff.slot), rom_start::CARD_FILE_NAME,
            None, handoff.stage_max_bytes)?;
        if stage.is_empty() || stage.len() % 4 != 0 {
            return Err("SD card stage must contain whole AArch64 instructions".into());
        }
        session.board.write_physical(handoff.pc, &stage)?;
    }
    if let Some(Entry::SetupHandoff(handoff)) = rom.map(|rom| &rom.entry) {
        let stage = firmware.xor_dat_range(handoff.dat_offset, handoff.length, &handoff.sha256)?;
        session.board.write_physical(handoff.pc, &stage)?;
    }
    // The setup start has no flash backing until the board declares the serial-NAND medium;
    // every other early start needs it.
    let setup_without_flash = storage.is_none()
        && rom.is_some_and(|rom| matches!(rom.entry, Entry::SetupHandoff(_)));
    if rom.is_some() && !setup_without_flash {
        shadow_flash_aperture(storage.as_ref(), &mut session)?;
        let storage = storage.ok_or("early startup flash backing is absent")?;
        session.window_action = Some(fujimod_emulator_session::WindowAction {
            interval: std::num::NonZeroU64::new((ticks_per_second / 1_000).max(1)).unwrap(),
            synchronize: Box::new(move |board| sync_flash_aperture(&storage, board)),
        });
    }
    Ok(Opened {
        session,
        inputs,
        outputs,
        ticks_per_second,
        quantum,
        read_limit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRAM: (u64, u64) = (0, 0x8000_0000);

    #[test]
    fn caller_ram_covered_by_catalog_is_skipped() {
        let plan = plan_caller_ram(0x6800_0000, 0x1800_0000, &[DRAM], 0, 0x8000_0000);
        assert_eq!(plan, Ok(RamPlan::Covered));
    }

    #[test]
    fn caller_ram_at_or_above_dram_limit_is_promoted() {
        let phy = plan_caller_ram(0xfb00_0000, 0x1a_0000, &[DRAM], 0, 0x8000_0000);
        assert_eq!(phy, Ok(RamPlan::Promote));
        let at_limit = plan_caller_ram(0x8000_0000, 0x1000, &[DRAM], 0, 0x8000_0000);
        assert_eq!(at_limit, Ok(RamPlan::Promote));
    }

    #[test]
    fn caller_ram_growing_dram_is_refused() {
        // Catalog DRAM stops at 0x68000000 but the run's limit is 0x80000000 (as if the catalog
        // had not been grown): the region lies inside the window without backing.
        let short = (0, 0x6800_0000);
        let error = plan_caller_ram(0x6800_0000, 0x1000, &[short], 0, 0x8000_0000).unwrap_err();
        assert!(error.contains("no physical backing"), "{error}");
        // A region straddling the end of catalog RAM is a partial overlap.
        let error = plan_caller_ram(0x67ff_f000, 0x2000, &[short], 0, 0x8000_0000).unwrap_err();
        assert!(error.contains("partly overlaps"), "{error}");
        assert!(plan_caller_ram(u64::MAX - 1, 4, &[DRAM], 0, 0x8000_0000).is_err());
    }

    #[test]
    fn aperture_is_added_for_a_free_window_and_rounded_to_pages() {
        let plan = plan_aperture(0xfb5a_0000, 0x1000, &[(0xfb00_0000, 0x1a_0000)]);
        assert_eq!(plan, Ok(Some((0xfb5a_0000, 0x4000))));
    }

    #[test]
    fn aperture_is_skipped_when_fully_inside_existing_ranges() {
        assert_eq!(plan_aperture(0xfa00_1000, 0x100, &[(0xfa00_0000, 0x4000)]), Ok(None));
        // Spanning two adjacent apertures is still fully inside.
        let two = [(0xfa00_0000, 0x4000), (0xfa00_4000, 0x4000)];
        assert_eq!(plan_aperture(0xfa00_3f00, 0x200, &two), Ok(None));
    }

    #[test]
    fn aperture_partial_overlap_is_an_error() {
        // Window runs off the end of a block aperture.
        assert!(plan_aperture(0xfa00_3f00, 0x200, &[(0xfa00_0000, 0x4000)]).is_err());
        // Window is outside, but its page touches a RAM mapping.
        assert!(plan_aperture(0xfb1a_0000, 0x1000, &[(0xfb1a_2000, 0x2000)]).is_err());
        assert!(plan_aperture(u64::MAX - 0x10, 0x100, &[]).is_err());
        assert!(plan_aperture(0xfb5a_0000, 0, &[]).is_err());
    }
}
