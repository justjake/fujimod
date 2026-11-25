//! Caller-side construction of the X100VI run on FujiMod's standard board.
//!
//! Firmware paths, preloads and device options remain inputs owned by this
//! application. CPU execution and device scheduling are
//! delegated to FujiMod's catalog-resolved XP5 session.

use super::{
    manifest::BootState,
    firmware::Firmware,
    rom_start::{self, Entry, RomStart},
};
use fujimod_catalog::reference::{ProcessorModel, RegionSize, Soc};
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
use fujimod_emulator_xp5_storage::{flash_storage, nand_storage};
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

/// Mailbox interrupt lines the catalog declares on the block behind the
/// companion window. Request and control have fixed companion routes; reply,
/// status and fault are controller lines.
fn mailbox_pins(soc: &Soc, base: u64, companion: usize) -> Result<mailbox::Pins, String> {
    let block = soc
        .blocks
        .iter()
        .find(|block| block.base == base)
        .ok_or_else(|| {
            format!(
                "SoC {} has no block at the mailbox window {base:#x}",
                soc.id
            )
        })?;
    let core = soc
        .processors
        .get(companion)
        .ok_or("mailbox companion core is absent")?
        .id
        .as_str();
    let companion_line = |output: &str| {
        block
            .interrupts
            .iter()
            .find(|row| row.output == output && row.core.as_deref() == Some(core))
            .map(|row| row.line)
            .ok_or_else(|| {
                format!(
                    "SoC {} mailbox block {} has no {output} line for core {core}",
                    soc.id, block.id
                )
            })
    };
    let controller_line = |output: &str| {
        block
            .interrupts
            .iter()
            .find(|row| row.output == output)
            .map(|row| row.line)
            .ok_or_else(|| {
                format!(
                    "SoC {} mailbox block {} has no {output} line",
                    soc.id, block.id
                )
            })
    };
    Ok(mailbox::Pins {
        companion,
        request: companion_line("request")?,
        control: companion_line("control")?,
        reply: controller_line("reply")?,
        status: controller_line("status")?,
        fault: controller_line("fault")?,
    })
}

fn devices_and_resources(
    state: &BootState,
    firmware: &Firmware,
    run_directory: Option<&Path>,
    outputs: &crate::output::Outputs,
    mut windows: Vec<DeviceWindow>,
    assets: &BTreeMap<String, Vec<u8>>,
    card_images: &[(u64, &fujimod_emulator_devices::parts::sd_card::Card)],
    soc: &Soc,
    companion_core: Option<usize>,
) -> Result<(Vec<DeviceWindow>, xp5_board::Resources), String> {
    let pins = fujimod_emulator_devices::xp5::pins::Pins::shared();
    let companion = match state
        .device
        .iter()
        .find(|window| window.kind == "interprocessor-mailbox")
    {
        Some(window) => Some(xp5_board::Companion {
            pins: mailbox_pins(
                soc,
                window.base,
                companion_core.ok_or("mailbox requires a Cortex-M4 core")?,
            )?,
            channel: mailbox::Channel::default(),
        }),
        None => None,
    };
    let mut resources = xp5_board::Resources {
        sd: xp5_board::sd::resolve(state.sd.clone(), assets)?,
        counting_dial: None,
        pins,
        clocks: panel_clocks::Clocks::new(),
        companion,
        storage: None,
        displays: BTreeMap::new(),
        pwm: Default::default(),
    };
    for &(base, card) in card_images {
        // A raw card image replaces the formatted volume of its slot: `--card-image` in slot 1,
        // in the ROM start and in the loader start, and `--card-image-2` in slot 2.
        let slot = resources
            .sd
            .get_mut(&base)
            .ok_or_else(|| format!("a card image needs the SD slot at {base:#x}"))?;
        slot.card = Some(card.clone());
    }
    for window in &mut windows {
        match window.kind.as_str() {
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
                let mut image = firmware.dat_payload(origin, xor)?;
                // A host input: extents of the installed image that start erased.
                if let Ok(spec) = std::env::var("XT5_FLASH_ERASED") {
                    flash_storage::erase_installed(&mut image, &spec)?;
                }
                let medium = window
                    .options
                    .get("medium")
                    .and_then(toml::Value::as_str)
                    .unwrap_or("nor");
                resources.storage = Some(match medium {
                    "nor" => flash_storage::Medium::Nor(Arc::new(Mutex::new(
                        flash_storage::Storage::new(image)?,
                    ))),
                    "serial-nand" => flash_storage::Medium::Nand(Arc::new(Mutex::new(
                        nand_storage::NandStorage::new(image)?,
                    ))),
                    other => return Err(format!("unknown flash storage medium {other}")),
                });
                window.options.remove("dat_origin");
                window.options.remove("xor");
            }
            "display" => {
                let mut options = window.options.clone();
                if let Some(directory) = run_directory {
                    options.entry("frame_dir").or_insert_with(|| {
                        toml::Value::String(directory.join(format!("display-{:x}", window.base))
                            .to_string_lossy().into_owned())
                    });
                } else {
                    options.remove("frame_dir");
                }
                let display = super::display_output::configure(outputs, window.base, &options)?;
                resources.displays.insert(window.base, display);
                window.options.clear();
            }
            _ => {}
        }
    }
    Ok((windows, resources))
}

/// Write the flash image into the RAM that backs the CPU's flash aperture in a ROM start. A DMA
/// transfer reads RAM only, so the setup stage's copy from the aperture needs this content for
/// serial NAND as well (its linear main area).
fn shadow_flash_aperture(
    storage: Option<&flash_storage::Medium>,
    session: &mut Session,
) -> Result<(), String> {
    let bytes = match storage {
        Some(flash_storage::Medium::Nor(storage)) => {
            let mut storage = storage.lock().map_err(|_| "flash storage lock poisoned")?;
            let bytes = storage.read(0, FLASH_APERTURE_SIZE)?;
            // The seeds are already in `bytes`; only later changes are new.
            storage.take_dirty_pages();
            bytes
        }
        Some(flash_storage::Medium::Nand(storage)) => storage
            .lock()
            .map_err(|_| "flash storage lock poisoned")?
            .read(0, FLASH_APERTURE_SIZE)?,
        None => return Err("ROM start needs the flash image to back the aperture".into()),
    };
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

/// Whether the boot manifest declares exactly `base..base+bytes` as RAM.
fn caller_ram_declares(ram: &[super::manifest::RamRegion], base: u64, bytes: u64) -> bool {
    ram.iter().any(|region| region.base == base && region.size == bytes)
}

/// Set `registers` (hex address -> value) on a power-controller part, keeping every register it
/// already holds at another address. Addresses compare by value: `"0x1f"` and `"1f"` are the same.
fn merge_power_registers(part: &mut toml::Table, registers: &toml::Value) -> Result<(), String> {
    let address = |key: &str| {
        u8::from_str_radix(key.trim_start_matches("0x"), 16)
            .map_err(|error| format!("power-controller register {key}: {error}"))
    };
    let added = registers.as_table().ok_or("power-controller registers must be a table")?;
    let held = part
        .entry("registers")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or("power-controller registers must be a table")?;
    for (key, value) in added {
        let wanted = address(key)?;
        let same: Vec<String> = held
            .keys()
            .filter(|existing| address(existing).is_ok_and(|found| found == wanted))
            .cloned()
            .collect();
        for existing in same {
            held.remove(&existing);
        }
        held.insert(key.clone(), value.clone());
    }
    Ok(())
}

/// The boot state of a ROM start: nothing of the stock firmware is preloaded, because the ROM
/// and stage one bring it up from the emulated internal flash; the boot-source straps are an
/// explicit input; the card carries only the caller's stage-one files.
fn rom_boot_state(state: &BootState, rom: &RomStart) -> Result<BootState, String> {
    let mut state = state.clone();
    state.load_firmware = false;
    state.preload.clear();
    // Stage one configures the serial flash itself.
    if state.flash.take().is_some() {
        for window in state.device.iter_mut().filter(|window| window.kind == "flash-storage-controller") {
            window.options.remove("volatile_dummy_cycles");
        }
    }
    let mut options = toml::Table::new();
    options.insert("source_straps".into(), toml::Value::Integer(rom.source_straps.into()));
    options.insert("initial_control".into(), toml::Value::Integer(0));
    options.insert("initial_primary_control".into(), toml::Value::Integer(0));
    // A manifest may declare boot control for its loader start; the ROM start's straps replace it.
    state.device.retain(|window| window.kind != "boot-control");
    state.device.push(DeviceWindow {
        base: 0xff90_e000,
        size: 4,
        kind: "boot-control".into(),
        options,
    });
    let mut fragment = rom_start::Fragment::bundled()?;
    // The firmware's own stock-setup windows come first, so their stub registers answer before
    // the shared fragment's registers of the same page.
    let mut windows = rom.setup_windows.clone();
    let slot_base = match &rom.entry {
        Entry::MaskRom(_) => {
            if let Some(registers) = &rom.reset_registers {
                fragment.power_controller_registers.clone_from(registers);
            }
            Some(0xff9b_0000)
        }
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
    windows.extend(fragment.device);
    state.ram.extend(fragment.ram);
    for window in windows {
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
    for options in fragment.window_options.into_iter().chain(rom.setup_window_options.iter().cloned()) {
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
                    // Registers the endpoint already holds (a user profile header) stay; the
                    // reset registers of this start replace only their own addresses.
                    merge_power_registers(part, &registers)?;
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
    loader_cards: &[super::loader_card::LoaderCard],
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
        let setup_start = rom.is_some_and(|rom| matches!(rom.entry, Entry::SetupHandoff(_)));
        profile.gic_initial = None;
        // The parked cores' redistributors are awake (the profile's WAKER words for cores other
        // than 0): the SGI that starts the reset stage's workers cannot reach a sleeping one, and
        // the stage wakes only its own. INFERRED for the uncaptured boot ROM, like the CPU
        // interface below.
        profile.gic_redistributor_initial = profile
            .gic_redistributor_initial
            .take()
            .filter(|_| setup_start)
            .map(|words| {
                let mut kept: Vec<_> = words.into_iter().filter(|word| word.core != 0).collect();
                // SGI 0 enabled in each parked core's SGI frame (ISENABLER0 at +0x10100), the
                // wake-up the reset stage sends them; INFERRED like the words above.
                let enables: Vec<_> = kept
                    .iter()
                    .filter(|word| word.offset == 0x14)
                    .cloned()
                    .map(|mut word| {
                        word.offset = 0x10100;
                        word.value = 1;
                        word
                    })
                    .collect();
                kept.extend(enables);
                kept
            });
        for cpu in profile.cpu.iter_mut().flatten() {
            // A core other than 0 parks in the setup stage's trampoline (wfi) and can only take
            // the SGI that wakes it if its CPU interface is enabled. The stage never enables it,
            // so the uncaptured boot ROM must have. INFERRED: the values are the ones the loader
            // hand-off profile carries (priority mask 0xff, both groups enabled).
            let parked = setup_start && cpu.id != 0;
            cpu.sysregs.icc_pmr_el1 = if parked { 0xff } else { 0 };
            cpu.sysregs.icc_igrpen0_el1 = u32::from(parked).into();
            cpu.sysregs.icc_igrpen1_el1 = u32::from(parked).into();
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
                    // Every core enters the stage at its base: a core other than 0 skips `main`
                    // and parks in the stage's trampoline (wfi) until the reset stage wakes it
                    // with an SGI, so the reset stage's worker loop has a core to run on.
                    start.released = true;
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
            let drains = reference
                .body
                .hardware
                .as_mut()
                .map(|hardware| &mut hardware.bus_drain)
                .filter(|drains| !drains.is_empty())
                .ok_or("manifest moves the bus drain, which the body does not declare")?;
            let [drain] = drains.as_mut_slice() else {
                return Err("manifest moves the bus drain, which the body declares more than once".into());
            };
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
    for (core, processor) in resolved.reference.soc.processors.iter().enumerate() {
        if processor.model != ProcessorModel::CortexM4 {
            continue;
        }
        for window in &state.companion_window {
            execution.mappings.push(board::factory::MemoryMap {
                core,
                address: window.address,
                physical: window.physical,
                size: usize::try_from(window.size)
                    .map_err(|_| "companion window exceeds host address space")?,
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
            // A manifest that declares this extent as RAM (the X-T5's boot SRAM) already mapped
            // it for the cores above and maps it for the bus below; a second mapping overlaps.
            if caller_ram_declares(&state.ram, base, bytes) {
                continue;
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
    // Boot control also owns a secondary window outside its manifest entry.
    if state.device.iter().any(|window| window.kind == "boot-control") {
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
    // A body whose catalog record declares ladders drives them through the
    // session's switch panel; a manifest copy of ladder wiring is refused.
    if state.device.iter().any(|window| {
        window.kind == "adc" && window.options.keys().any(|key| key != "samples")
    }) {
        return Err("controls come from the body's catalog ladders, not the boot manifest".into());
    }
    let ladders = fujimod_emulator_xp5_board::ladder_controls::ladders(&resolved)?;
    let panel = ladders
        .map(|options| -> Result<_, String> {
            Ok((options, fujimod_emulator_xp5_board::ladder_controls::panel_names(&resolved)?))
        })
        .transpose()?;
    let run_scene = match resolved.configuration.scene.as_ref() {
        Some(RunScene::Xp5(scene)) => scene,
        _ => return Err("X100VI run configuration has no XP5 scene".into()),
    };
    let mut windows = state.device.clone();
    xp5_boot::apply_run_scene(&resolved, &mut windows, run_scene, ticks_per_second)?;
    let outputs = crate::output::Outputs::default();
    let (windows, resources) = devices_and_resources(
        state,
        firmware,
        run_directory,
        &outputs,
        windows,
        &rom.map(RomStart::assets).unwrap_or_else(|| {
            loader_cards.iter().flat_map(|card| card.assets.clone()).collect()
        }),
        &rom.map_or_else(
            || loader_cards.iter().filter_map(|card| Some((card.slot, card.card.as_ref()?))).collect(),
            |rom| rom.card_image.iter().map(|image| (super::loader_card::SLOT, &image.card)).collect::<Vec<_>>(),
        ),
        &reference.soc,
        reference.soc.processors.iter()
            .position(|processor| processor.model == ProcessorModel::CortexM4),
    )?;
    if windows.is_empty() {
        return Err("X100VI caller boot manifest has no device windows".into());
    }
    if let Some(profile) = profile {
        // The user profile's journal records, as declared seeds of the flash storage (a NOR
        // session overlay or initial NAND main-area input), so snapshots carry them.
        let storage = resources.storage.as_ref().ok_or("a user profile needs flash storage")?;
        for (offset, bytes) in &profile.records {
            storage.seed_bytes(*offset, bytes)?;
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
    let wiring = serde_json::to_vec(state).map_err(|error| error.to_string())?;
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
    // A DMA master reads memory, not bus devices, so a start without a ROM gets the ROM start's
    // flash shadow: aperture memory filled from the flash storage and kept in step with
    // programmed pages.
    let dma_flash_shadow = rom.is_none()
        && matches!(resources.storage, Some(flash_storage::Medium::Nor(_)))
        && state.device.iter().any(|w| {
            w.kind == "flash-storage" && w.base == FLASH_APERTURE_BASE && w.size as usize == FLASH_APERTURE_SIZE
        });
    if dma_flash_shadow {
        let size = FLASH_APERTURE_SIZE;
        // As in a ROM start, CPU reads then see the shadow (the CPU's view of flash is the
        // image, not the controller's read-mode refusals); the bus device stays registered.
        execution.peripherals = carve_out(
            std::mem::take(&mut execution.peripherals),
            FLASH_APERTURE_BASE,
            size as u64,
        );
        for (core, processor) in resolved.reference.soc.processors.iter().enumerate() {
            if processor.model == ProcessorModel::CortexA53 {
                execution.mappings.push(board::factory::MemoryMap {
                    core,
                    address: FLASH_APERTURE_BASE,
                    physical: FLASH_APERTURE_BASE,
                    size,
                    permissions: 5,
                });
            }
        }
        if !execution.ram.iter().any(|ram| ram.base == FLASH_APERTURE_BASE && ram.size == size) {
            execution.ram.push(board::factory::Ram { base: FLASH_APERTURE_BASE, size });
        }
        execution.dma_mappings.push(board::memory::BusMapping {
            permissions: 1,
            ..board::memory::BusMapping::ram(FLASH_APERTURE_BASE, size)
        });
    }
    let storage = resources.storage.clone();
    let read_limit = execution.ram.iter().try_fold(0usize, |bytes, region| {
        bytes.checked_add(region.size).ok_or("XP5 RAM extent overflows")
    })?;
    let gpio_node = windows
        .iter()
        .find(|window| window.kind == "gpio")
        .map(|window| format!("{:#x}", window.base));
    let mut resources = resources;
    use fujimod_emulator_xp5_board::ladders::CommandDial;
    let options = panel.as_ref().map(|(options, _)| options);
    resources.counting_dial = match options.and_then(|options| options.rear_dial.as_ref()) {
        Some(CommandDial::Counting(dial)) => Some(dial.register),
        _ => None,
    };
    let encoder = |dial: &fujimod_emulator_xp5_board::ladders::QuadratureDial| {
        fujimod_emulator_parts_controls::encoder::Options {
            pins: Some(dial.pins),
            outputs: Default::default(),
            // One detent edge every 2 ms, well inside the sampler's period.
            phase_ticks: (ticks_per_second / 500).max(1),
            rest_phase: dial.rest_phase,
        }
    };
    let dials = xp5_boot::Dials {
        rear: match options.and_then(|options| options.rear_dial.as_ref()) {
            Some(CommandDial::Quadrature(dial)) => Some(encoder(dial)),
            _ => None,
        },
        front: options.and_then(|options| options.front_dial.as_ref()).map(encoder),
    };
    let (mut session, mut inputs) = xp5_boot::open(xp5_boot::OpenOptions {
        definition,
        mode,
        processors: execution,
        windows,
        dials,
        resources,
        engine: run.engine,
        interrupts,
        aprofile,
        images,
        scene,
    })?;
    if let Some((options, names)) = panel {
        fujimod_emulator_xp5_board::panel::connect(
            &mut session,
            &mut inputs,
            options,
            names,
            gpio_node.ok_or("ladder controls need the board's GPIO controller")?,
            ticks_per_second,
        )?;
    }
    session.board.set_block_execution(
        state.mprofile_block_execution
            && std::env::var_os("FUJIMOD_MPROFILE_INSTRUCTIONS").is_none(),
    );
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
    if dma_flash_shadow {
        shadow_flash_aperture(storage.as_ref(), &mut session)?;
        let storage = storage.clone().ok_or("flash backing is absent")?;
        session.window_action = Some(fujimod_emulator_session::WindowAction {
            interval: std::num::NonZeroU64::new((ticks_per_second / 1_000).max(1)).unwrap(),
            synchronize: Box::new(move |board| sync_flash_aperture(&storage, board)),
        });
    }
    let serial_nand = matches!(storage, Some(flash_storage::Medium::Nand(_)));
    if rom.is_some() && !setup_without_flash {
        shadow_flash_aperture(storage.as_ref(), &mut session)?;
        let storage = storage.ok_or("early startup flash backing is absent")?;
        // Serial NAND refuses program and erase, so its shadow never changes.
        if !serial_nand {
            session.window_action = Some(fujimod_emulator_session::WindowAction {
                interval: std::num::NonZeroU64::new((ticks_per_second / 1_000).max(1)).unwrap(),
                synchronize: Box::new(move |board| sync_flash_aperture(&storage, board)),
            });
        }
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
    fn an_entry_extent_the_manifest_declares_as_ram_is_not_mapped_twice() {
        let ram: Vec<super::super::manifest::RamRegion> =
            toml::from_str::<toml::Table>("[[ram]]\nbase = 0xff800000\nsize = 0x40000\n").unwrap()["ram"]
                .clone()
                .try_into()
                .unwrap();
        assert!(caller_ram_declares(&ram, 0xff80_0000, 0x4_0000));
        assert!(!caller_ram_declares(&ram, 0xff80_0000, 0x3_c000));
        assert!(!caller_ram_declares(&ram, 0xfff0_0000, 0x1_0000));
    }

    #[test]
    fn mailbox_lines_come_from_the_catalog_block() {
        let selection = fujimod_catalog::ReferenceCatalog::bundled()
            .unwrap()
            .resolve("x100vi", "x100vi-131")
            .unwrap();
        let mailbox = selection
            .soc
            .blocks
            .iter()
            .find(|block| block.id == "mailbox")
            .unwrap();
        let line = |output: &str| {
            mailbox
                .interrupts
                .iter()
                .find(|row| row.output == output)
                .unwrap()
        };
        assert_eq!(line("request").core.as_deref(), Some("core4"));
        assert_eq!(line("control").core.as_deref(), Some("core4"));
        assert!(line("reply").core.is_none());
        assert!(line("status").core.is_none());
        assert!(line("fault").core.is_none());
        let pins = mailbox_pins(&selection.soc, mailbox.base, 4).unwrap();
        assert_eq!(
            (
                pins.request,
                pins.control,
                pins.reply,
                pins.status,
                pins.fault
            ),
            (55, 58, 141, 142, 143)
        );
        assert_eq!(pins.companion, 4);
        assert!(mailbox_pins(&selection.soc, mailbox.base, 0).is_err());
    }

    #[test]
    fn start_registers_keep_a_profile_header_and_replace_their_own_addresses() {
        let mut part: toml::Table =
            toml::from_str("kind = \"power-controller\"\n[registers]\n\"0x1f\" = 7\n\"0x10\" = 9\n").unwrap();
        let start: toml::Value = toml::from_str("\"0\" = 0x8202\n\"10\" = 2\n").unwrap();
        merge_power_registers(&mut part, &start).unwrap();
        let held = part["registers"].as_table().unwrap();
        assert_eq!(held.len(), 3);
        assert_eq!(held["0x1f"].as_integer(), Some(7));
        assert_eq!(held["0"].as_integer(), Some(0x8202));
        assert_eq!(held["10"].as_integer(), Some(2));
        let mut bare: toml::Table = toml::from_str("kind = \"power-controller\"\n").unwrap();
        merge_power_registers(&mut bare, &start).unwrap();
        assert_eq!(bare["registers"].as_table().unwrap().len(), 2);
        let bad: toml::Value = toml::from_str("\"zz\" = 1\n").unwrap();
        assert!(merge_power_registers(&mut bare, &bad).unwrap_err().contains("zz"));
    }

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
