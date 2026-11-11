//! Local bring-up harness for the X-T5 4.31 firmware on FujiMod's XP5 board.
//!
//! This is not upstream FujiMod code. It assembles the catalog's XP5 SoC
//! description with the X-T5 body and runs the stock update's first-stage
//! loader, standing in for the boot ROM (which we do not have): it copies the
//! loader from flash into boot SRAM and starts core 0 at its entry.
//!
//! SD-card start (fujimod-recovery XP5 startup card):
//!   xt5-bringup --update FWUP0030.DAT --start sdcard --sd-dir CARD_DIR \
//!     --secondaries --set 1:pc=CONT --set 2:pc=CONT --set 3:pc=CONT \
//!     --wake-secondaries-at 1800 --quantum 100 --instructions 9000000000 --chunk 100000000 \
//!     --stub 0xfaf07000:0x780=constant:1,0x784=constant:0,0x800=constant:1 --no-stop \
//!     --frames-dir DIR --all-frames
//! where CONT is the stage-one continuation the card's loader returns to.
//! Slot-1 card for the handoff start (host SDHCI at 0xff9b0000, INTID 262;
//! the second register set at
//! 0xff9b2200 reports no UHS-II so the standard path is taken):
//!   --card 0xff9b0000@262=card.img --card 0xff9b2200/0x1e0=card.img \
//!   --stub 0xff9b2000/0x200:0x7c=constant:1
//! where card.img is a raw MBR/FAT32 image (diskutil partitionDisk on an
//! attached sparse file). The OS enumerates the card and reads/writes it;
//! with a card present the REC task then starts live view and polls the
//! imaging-sensor block (0xff948284..0xff9482d8) without finishing, so the
//! shooting screen is not drawn yet (black frames after 9.3 s).
//! The card's stage 1 writes ICC_SRE_EL2 and ICC_IGRPEN1_EL3, which the CPU
//! backend lacks; an emulator copy of the card must replace those two writes
//! with NOPs and rewrite the loader's CRC. --quantum 100 avoids the image-4 four-core
//! barrier race (docs/STATUS.md "Barrier race"). The wizard's language page
//! appears at about 3.9 s guest time (1.9 s of that is the card load).

use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator::board::{bus::Mode, factory};
use fujimod_emulator_boot::graph::{ProcessorStart, assemble_with_starts};
use fujimod_emulator_cpu::{Register, arm64};
use fujimod_emulator_xp5_board::{board as xp5_board, boot};
use fujimod_emulator_xp5_display::display;
use fujimod_emulator_xp5_storage::flash_storage;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

const CONFIGURATION: &str = "xt5-431";
const PROFILE: &str = "xt5-431";
const BOOT_SRAM: u64 = 0xff80_0000;
// Body bus-drain note: the DRAM map ends at 0x60000000 (the catalog run limit 1 GiB is only the
// kernel's first identity map); DMA targets 0x40010000.
// After first-start setup the firmware writes near the top of 2 GiB, so the body has at least
// 2 GiB; the body's bus-drain read at 0x60000000 then lands in DRAM.
const DRAM_LIMIT: u64 = 0x8000_0000;

struct Args {
    update: PathBuf,
    instructions: u64,
    chunk: u64,
    quantum: usize,
    engine: factory::Engine,
    mode: Mode,
    start: String,
    images: Vec<(u64, PathBuf)>,
    stop_at: Vec<u64>,
    state_out: Option<PathBuf>,
    quiet: bool,
    omit: Vec<String>,
    secondaries: bool,
    trace: Option<PathBuf>,
    trace_limit: usize,
    trace_after_ms: u64,
    watch_ranges: Vec<(u64, u64)>,
    set_regs: Vec<(Option<usize>, String, u64)>,
    watch: Vec<u64>,
    pokes: Vec<(u64, u32)>,
    gpio_inputs: Vec<(usize, usize, bool)>,
    frames_dir: Option<PathBuf>,
    policy: Option<String>,
    dumps: Vec<(u64, usize)>,
    presses: Vec<(u64, String, u64)>,
    all_frames: bool,
    allow_modified: bool,
    save_snapshot: Option<PathBuf>,
    load_snapshot: Option<PathBuf>,
    dump_files: Vec<(u64, usize, PathBuf)>,
    /// `--start sdcard`: directory holding the SD card files (marble_init.bin, ffboot.bin, ...).
    sd_dir: Option<PathBuf>,
    /// Skip the bring-up register stubs that let the stock setup run from SRAM.
    no_stubs: bool,
    /// Extra stub windows: (base, registers as bringup_stub JSON).
    stubs: Vec<(u64, u64, Vec<Value>)>,
    wake_ms: Option<u64>,
    /// `--card BASE=IMAGE`: a raw host card image (MBR/FAT32 or exFAT) attached
    /// to an SDHCI host at BASE for any start mode.
    cards: Vec<(u64, u64, Option<u64>, PathBuf)>,
}

fn parse_u64(text: &str) -> Result<u64, String> {
    let text = text.replace('_', "");
    if let Some(hex) = text.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).map_err(|e| format!("{text}: {e}"))
    } else {
        text.parse().map_err(|e| format!("{text}: {e}"))
    }
}

fn args() -> Result<Args, String> {
    let mut out = Args {
        update: PathBuf::from("FWUP0030.DAT"),
        instructions: 50_000_000,
        chunk: 1_000_000,
        quantum: 1000,
        engine: factory::Engine::Unicorn,
        mode: Mode::Discovery,
        start: "stage0".into(),
        images: vec![],
        stop_at: vec![],
        state_out: None,
        quiet: false,
        omit: vec!["statistics".into(), "panel-spi".into()],
        secondaries: false,
        trace: None,
        trace_limit: 200_000,
        trace_after_ms: 0,
        watch_ranges: vec![],
        set_regs: vec![],
        watch: vec![],
        pokes: vec![],
        gpio_inputs: vec![],
        frames_dir: None,
        policy: None,
        dumps: vec![],
        presses: vec![],
        all_frames: false,
        allow_modified: false,
        save_snapshot: None,
        load_snapshot: None,
        dump_files: vec![],
        sd_dir: None,
        no_stubs: false,
        stubs: vec![],
        wake_ms: None,
        cards: vec![],
    };
    let mut it = std::env::args().skip(1);
    while let Some(key) = it.next() {
        let mut value = || it.next().ok_or(format!("missing value for {key}"));
        match key.as_str() {
            "--update" => out.update = value()?.into(),
            "--instructions" => out.instructions = parse_u64(&value()?)?,
            "--chunk" => out.chunk = parse_u64(&value()?)?,
            "--quantum" => out.quantum = parse_u64(&value()?)? as usize,
            "--engine" => {
                out.engine = match value()?.as_str() {
                    "unicorn" => factory::Engine::Unicorn,
                    "hvf" | "hypervisor" => factory::Engine::Hypervisor,
                    other => return Err(format!("unknown engine {other}")),
                }
            }
            "--mode" => {
                out.mode = match value()?.as_str() {
                    "strict" => Mode::Strict,
                    "discovery" => Mode::Discovery,
                    other => return Err(format!("unknown mode {other}")),
                }
            }
            "--start" => out.start = value()?,
            "--sd-dir" => out.sd_dir = Some(value()?.into()),
            "--card" => {
                let v = value()?;
                // BASE[/SIZE][@IRQ]=IMAGE; IRQ is the GIC line the host raises.
                let (base, path) = v.split_once('=').ok_or("--card BASE[/SIZE][@IRQ]=IMAGE")?;
                let (base, irq) = match base.split_once('@') {
                    Some((b, i)) => (b, Some(parse_u64(i)?)),
                    None => (base, None),
                };
                let (base, size) = match base.split_once('/') {
                    Some((b, s)) => (b, parse_u64(s)?),
                    None => (base, 0x1000),
                };
                out.cards.push((parse_u64(base)?, size, irq, path.into()));
            }
            "--no-stubs" => out.no_stubs = true,
            // Wake cores 1-3 at guest time MS (SD start: the secondaries park
            // in the stock flash loop until the emulator wakes them).
            "--wake-secondaries-at" => out.wake_ms = Some(parse_u64(&value()?)?),
            "--stub" => {
                // BASE:OFFSET=MODE[:VALUE][,OFFSET=MODE[:VALUE]...]; MODE as in bringup_stub::Mode.
                let v = value()?;
                let (base, list) = v.split_once(':').ok_or("--stub BASE[/SIZE]:OFFSET=MODE[:VALUE],...")?;
                let (base, size) = match base.split_once('/') {
                    Some((b, s)) => (b, parse_u64(s)?),
                    None => (base, 0x1000),
                };
                let mut registers = vec![];
                for item in list.split(',') {
                    let (offset, spec) = item.split_once('=').ok_or("--stub OFFSET=MODE[:VALUE]")?;
                    let (mode, val) = spec.split_once(':').unwrap_or((spec, "0"));
                    registers.push(json!({"offset": parse_u64(offset)?, "mode": mode, "value": parse_u64(val)?}));
                }
                out.stubs.push((parse_u64(base)?, size, registers));
            }
            "--image" => {
                let v = value()?;
                let (addr, path) = v.split_once('=').ok_or("--image ADDR=FILE")?;
                out.images.push((parse_u64(addr)?, path.into()));
            }
            "--stop-at" => out.stop_at.push(parse_u64(&value()?)?),
            "--no-stop" => out.stop_at.clear(),
            "--state-out" => out.state_out = Some(value()?.into()),
            "--quiet" => out.quiet = true,
            "--omit" => out.omit.push(value()?),
            "--keep" => {
                let v = value()?;
                out.omit.retain(|o| *o != v);
            }
            "--secondaries" => out.secondaries = true,
            "--set" => {
                // [CORE:]REG=VALUE, e.g. x20=0x1000 or 0:SP_EL1=0x2000
                let v = value()?;
                let (lhs, val) = v.split_once('=').ok_or("--set [CORE:]REG=VALUE")?;
                let (core, reg) = match lhs.split_once(':') {
                    Some((c, r)) => (Some(parse_u64(c)? as usize), r.to_string()),
                    None => (None, lhs.to_string()),
                };
                out.set_regs.push((core, reg, parse_u64(val)?));
            }
            "--handoff-images" => {
                // Every dram_<8 hex digits>_<name>.bin in DIR is loaded at
                // the address its name gives.
                let dir = PathBuf::from(value()?);
                let mut found = vec![];
                for entry in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
                    let path = entry.map_err(|e| e.to_string())?.path();
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if let Some(hex) = name.strip_prefix("dram_").and_then(|r| r.get(..8))
                        && name.ends_with(".bin")
                        && let Ok(addr) = u64::from_str_radix(hex, 16)
                    {
                        found.push((addr, path));
                    }
                }
                if found.is_empty() {
                    return Err(format!("no dram_<address>_*.bin images in {}", dir.display()));
                }
                found.sort();
                out.images.extend(found);
            }
            "--gpio" => {
                // PORT:PIN=LEVEL, an external GPIO input level set before the run.
                for item in value()?.split(',') {
                    let (pp, level) = item.split_once('=').ok_or("--gpio PORT:PIN=0|1")?;
                    let (port, pin) = pp.split_once(':').ok_or("--gpio PORT:PIN=0|1")?;
                    out.gpio_inputs.push((port.parse().map_err(|_| "bad GPIO port")?, pin.parse().map_err(|_| "bad GPIO pin")?, level == "1"));
                }
            }
            "--poke" => {
                // ADDR=U32 written to physical memory after the images.
                let v = value()?;
                let (a, val) = v.split_once('=').ok_or("--poke ADDR=VALUE")?;
                out.pokes.push((parse_u64(a)?, parse_u64(val)? as u32));
            }
            "--dump" => {
                // ADDR:LEN hex-dumped from physical memory at the end.
                let v = value()?;
                let (a, l) = v.split_once(':').ok_or("--dump ADDR:LEN")?;
                out.dumps.push((parse_u64(a)?, parse_u64(l)? as usize));
            }
            "--press" => {
                // TIME_MS:KEY[:HOLD_MS], guest time; KEY as in resistor_keys::Key::parse.
                for item in value()?.split(',') {
                    let parts: Vec<&str> = item.split(':').collect();
                    if parts.len() < 2 { return Err("--press TIME_MS:KEY[:HOLD_MS]".into()); }
                    let hold = parts.get(2).map(|h| parse_u64(h)).transpose()?.unwrap_or(150);
                    out.presses.push((parse_u64(parts[0])?, parts[1].to_string(), hold));
                }
            }
            "--all-frames" => out.all_frames = true,
            // Accept a patched update (same size, different hash) for mod testing.
            "--allow-modified" => out.allow_modified = true,
            "--dump-to" => {
                // ADDR:LEN:PATH raw physical memory written to a file at the end.
                let v = value()?;
                let mut it = v.splitn(3, ':');
                let (a, l, p) = (it.next().unwrap_or(""), it.next().ok_or("--dump-to ADDR:LEN:PATH")?, it.next().ok_or("--dump-to ADDR:LEN:PATH")?);
                out.dump_files.push((parse_u64(a)?, parse_u64(l)? as usize, p.into()));
            }
            "--save-snapshot" => out.save_snapshot = Some(value()?.into()),
            "--load-snapshot" => out.load_snapshot = Some(value()?.into()),
            "--policy" => out.policy = Some(value()?),
            "--frames-dir" => out.frames_dir = Some(value()?.into()),
            "--watch" => {
                for part in value()?.split(',') {
                    out.watch.push(parse_u64(part)?);
                }
            }
            "--trace" => out.trace = Some(value()?.into()),
            "--trace-limit" => out.trace_limit = parse_u64(&value()?)? as usize,
            "--trace-after" => out.trace_after_ms = parse_u64(&value()?)?,
            "--watch-range" => {
                for item in value()?.split(',') {
                    let (lo, hi) = item.split_once('-').ok_or("--watch-range LO-HI[,LO-HI]")?;
                    out.watch_ranges.push((parse_u64(lo)?, parse_u64(hi)?));
                }
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(out)
}

fn sysreg(name: &str) -> Register {
    Register::Arm64System(arm64::system_register(name).unwrap())
}

fn snapshot(board: &fujimod_emulator::board::Board, core: usize) -> Value {
    let r = |reg| board.read_register(core, reg).map(|v| format!("{v:#x}")).unwrap_or_else(|e| e);
    let mut v = json!({
        "pc": r(Register::Pc),
        "pstate": r(Register::Pstate),
        "sp": r(Register::Sp),
        "x0": r(Register::X(0)), "x1": r(Register::X(1)), "x2": r(Register::X(2)),
        "x19": r(Register::X(19)), "x20": r(Register::X(20)), "x21": r(Register::X(21)),
        "x30": r(Register::X(30)),
    });
    for name in ["VBAR_EL1", "SCTLR_EL1", "SP_EL0", "SP_EL1", "ELR_EL1", "ESR_EL1", "FAR_EL1", "CNTV_CTL_EL0", "CNTV_CVAL_EL0", "0xdf11", "0xdf12", "ICC_PMR_EL1", "ICC_IGRPEN1_EL1", "TPIDR_EL1", "0xc005", "0xc000"] {
        v[name] = json!(r(sysreg(name)));
    }
    v
}

fn run() -> Result<(), String> {
    let args = args()?;
    let resolved = ResolvedConfiguration::bundled(CONFIGURATION)?;
    let selection = &resolved.reference;
    let profile = selection
        .firmware
        .layouts
        .as_ref()
        .and_then(|l| l.profiles.get(PROFILE))
        .ok_or("firmware has no xt5-431 layout profile")?;
    let flash_fact = profile.flash_image.as_ref().ok_or("no flash_image fact")?;

    // Update file -> installed flash image.
    let update = std::fs::read(&args.update).map_err(|e| format!("{}: {e}", args.update.display()))?;
    let expected = selection.firmware.update.as_ref().ok_or("no update identity")?;
    {
        use sha2::{Digest, Sha256};
        let digest = format!("{:x}", Sha256::digest(&update));
        if digest != expected.sha256 {
            if !args.allow_modified || update.len() as u64 != expected.size {
                return Err(format!("update sha256 {digest} != catalog {}", expected.sha256));
            }
            eprintln!("modified update: sha256 {digest} (stock {})", expected.sha256);
        }
    }
    if flash_fact.xor != 0xff {
        return Err("only XOR 0xff flash images are handled".into());
    }
    let flash = fujimod_settings_free_flash(&update, flash_fact.dat_origin)?;

    let ticks_per_second = 1_000_000_000u64;
    let startup = boot::startup_from_profile(&resolved, profile, ticks_per_second)?;

    // Processor starts.
    let starts: Vec<ProcessorStart> = match args.start.as_str() {
        "handoff" => startup
            .processors
            .iter()
            .map(|p| ProcessorStart {
                processor_id: p.processor_id.clone(),
                released: p.released,
                registers: p.registers.clone(),
            })
            .collect(),
        // stage0: the boot ROM copies the setup code (flash 0x800.., header
        // word 0x10 bytes) to SRAM base and enters at byte 0; the setup's
        // absolute data pointers only resolve with the code at the base.
        // sdcard: the ROM loads marble_init.bin at the same base and enters
        // at byte 0 (fujimod-recovery's XP5 card contract).
        "stage0" | "sdcard" => startup
            .processors
            .iter()
            .enumerate()
            .map(|(index, p)| {
                let mpidr = p
                    .registers
                    .iter()
                    .find(|(r, _)| *r == sysreg("MPIDR_EL1"))
                    .map(|(_, v)| *v)
                    .unwrap_or(index as u64);
                ProcessorStart {
                    processor_id: p.processor_id.clone(),
                    released: index == 0 || args.secondaries,
                    registers: vec![
                        (Register::Pc, BOOT_SRAM),
                        // EL3h, DAIF masked: the ROM enters the setup at EL3
                        // (stage1 programs SCR_EL3/ICC_SRE_EL3; the card loader
                        // parks any core that is not at EL3).
                        (Register::Pstate, 0x3cd),
                        // The ROM enters with its own stack; the card loader
                        // pushes onto it before the stock setup sets sp. Use
                        // the top of the boot SRAM workspace (set after the
                        // mode so it lands in SP_EL3).
                        (Register::Sp, BOOT_SRAM + 0x3c000),
                        (sysreg("MPIDR_EL1"), mpidr),
                    ],
                }
            })
            .collect(),
        other => return Err(format!("unknown start {other}")),
    };
    let mut starts = starts;
    for (core, reg, value) in &args.set_regs {
        let register = match reg.as_str() {
            "pc" | "PC" => Register::Pc,
            "pstate" | "PSTATE" => Register::Pstate,
            "sp" | "SP" => Register::Sp,
            r if r.starts_with('x') => Register::X(r[1..].parse().map_err(|_| format!("bad register {r}"))?),
            r => sysreg(r),
        };
        for (index, start) in starts.iter_mut().enumerate() {
            if core.is_none_or(|c| c == index) {
                start.registers.retain(|(r, _)| *r != register);
                start.registers.push((register, *value));
            }
        }
    }
    let rate = resolved
        .configuration
        .execution
        .as_ref()
        .map(|e| e.instructions_per_tick)
        .ok_or("configuration has no execution policy")?;
    let mut graph = assemble_with_starts(selection, DRAM_LIMIT, rate, Some(ticks_per_second), &starts)?;
    // Execute-in-place: stage0 runs code from the flash aperture, which the
    // flash-storage device cannot serve as instructions. Back the aperture with
    // read-only RAM holding the flash image instead of the device window.
    const FLASH_BASE: u64 = 0xf000_0000;
    const FLASH_SIZE: usize = 0x0800_0000;
    graph.ram.push(factory::Ram { base: FLASH_BASE, size: FLASH_SIZE });
    // DDR PHY register/firmware space (two PHYs at 0xfb000000 and
    // 0xfb400000): the stock setup writes 16-bit PHY registers and DMA-copies
    // the PHY IMEM/DMEM firmware to 0xfb0a0000/0xfb4a0000. Back it with RAM
    // up to the mailbox pages at +0x1a0000, which stay bring-up stub windows.
    if matches!(args.start.as_str(), "stage0" | "sdcard") {
        for base in [0xfb00_0000u64, 0xfb40_0000] {
            graph.ram.push(factory::Ram { base, size: 0x1a_0000 });
        }
    }
    graph.peripherals.retain(|p| !(p.core < 4 && p.address < DRAM_LIMIT));
    graph.peripherals.retain(|p| !(p.address < FLASH_BASE + FLASH_SIZE as u64 && FLASH_BASE < p.address + p.size as u64));
    // The XP5 catalog declares no A53 memory map; identity-map DRAM and boot SRAM.
    for core in 0..4usize {
        if !graph.mappings.iter().any(|m| m.core == core) {
            for r in graph.ram.iter() {
                graph.mappings.push(factory::MemoryMap { core, address: r.base, physical: r.base, size: r.size, permissions: if r.base == FLASH_BASE { 5 } else { 7 } });
            }
        }
    }
    // Discovery: route every otherwise-unmapped page in the SoC's register
    // space to the bus, which shadows and records unowned MMIO.
    if args.mode == Mode::Discovery {
        const GRANULE: u64 = 0x4000;
        for core in 0..4usize {
            let covered = |page: u64, graph: &factory::Graph| {
                graph.ram.iter().any(|r| page < r.base + r.size as u64 && r.base < page + GRANULE)
                    || graph.peripherals.iter().any(|p| p.core == core && page < p.address + p.size as u64 && p.address < page + GRANULE)
                    || (0xf000_0000..0xf800_0000).contains(&page)
            };
            let mut page = 0xf800_0000u64;
            let mut run: Option<u64> = None;
            let mut adds = vec![];
            while page < 0x1_0000_0000 {
                if covered(page, &graph) {
                    if let Some(start) = run.take() { adds.push((start, page)); }
                } else if run.is_none() {
                    run = Some(page);
                }
                page += GRANULE;
            }
            if let Some(start) = run { adds.push((start, page)); }
            for (start, end) in adds {
                graph.peripherals.push(factory::Peripheral { core, address: start, physical: start, size: (end - start) as usize });
            }
        }
    }
    // The XP5 catalog declares no DMA bus map; give bus masters (and host
    // loading) an identity view of every RAM region.
    if graph.dma_mappings.is_empty() {
        graph.dma_mappings = graph
            .ram
            .iter()
            .map(|r| fujimod_emulator::board::memory::BusMapping::ram(r.base, r.size))
            .collect();
    }
    if std::env::var_os("XT5_DEBUG_GRAPH").is_some() {
        for r in &graph.ram { eprintln!("ram {:#x}+{:#x}", r.base, r.size); }
        for m in &graph.mappings { eprintln!("map core{} {:#x}->{:#x}+{:#x} p{}", m.core, m.address, m.physical, m.size, m.permissions); }
        for m in &graph.dma_mappings { eprintln!("dma {m:?}"); }
        eprintln!("peripherals {}", graph.peripherals.len());
    }

    // Device windows from the SoC profile, minus any we choose to omit.
    let mut windows = boot::windows(selection, "xp5")?;
    let omitted: Vec<String> = windows
        .iter()
        .filter(|w| args.omit.iter().any(|o| *o == w.kind || o.trim_start_matches("imaging-") == w.kind.trim_start_matches("imaging-")))
        .map(|w| format!("{}@{:#x}", w.kind, w.base))
        .collect();
    windows.retain(|w| w.kind != "flash-storage");
    windows.retain(|w| !(w.kind == "unmodeled-bus-drain" && w.base < DRAM_LIMIT));
    windows.retain(|w| !args.omit.iter().any(|o| *o == w.kind || o.trim_start_matches("imaging-") == w.kind.trim_start_matches("imaging-")));
    // The X-T5 configuration declares an empty scene; borrow the sibling X100VI
    // body's imaging frame timing so the imaging blocks have bounded periods.
    {
        let sibling = ResolvedConfiguration::bundled("x100vi-131")?;
        if let Some(fujimod_catalog_configuration::RunScene::Xp5(scene)) = sibling.configuration.scene.as_ref() {
            let mut scene = scene.clone();
            scene.frames.retain(|f| f.block_ids.iter().all(|id| {
                selection.soc.blocks.iter().any(|b| b.id == *id && windows.iter().any(|w| w.base == b.base))
            }));
            boot::apply_run_scene(&resolved, &mut windows, &scene, ticks_per_second)?;
        }
    }
    for w in windows.iter_mut() {
        if matches!(w.kind.as_str(), "imaging-rifp" | "imaging-rifp-io") {
            w.options.entry("completion_us").or_insert(toml_int(1000));
            w.options.entry("source_luma").or_insert(toml_int(128));
        }
        if matches!(w.kind.as_str(), "imaging-preview" | "movdc") {
            // The REC start sequence gives up after about 1 s without a frame, so
            // the receiver must deliver frames at a camera-like rate (default 30 fps).
            let period = std::env::var("XT5_FRAME_PERIOD_US")
                .ok()
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(33_333);
            w.options.insert("frame_period_us".into(), toml_int(period));
            w.options.entry("source_luma").or_insert(toml_int(128));
        }
        if w.kind == "adc" {
            // configurations/emulator/xt5-431/run.json adc block samples.
            let mut samples = vec![toml_int(2713)];
            samples.extend(std::iter::repeat_n(toml_int(4095), 14));
            w.options.entry("samples").or_insert(toml::Value::Array(samples));
        }
        if w.kind == "pl022" {
            // Line 99 follows the upstream XP5 fixture; the X-T5 wire's parts are unknown.
            w.options.entry("companion_irq_line").or_insert(toml_int(99));
            // The companion selects GPIO port 11 pin 5 (active low) before its
            // first transfer, as on the X100VI; use that body's two endpoints.
            w.options.entry("wire").or_insert(
                toml::Value::try_from(json!({"kind": "gpio-selected", "endpoints": [
                    {"select": {"port": 11, "pin": 5, "active_high": false},
                     "part": {"kind": "bis-configuration-registers", "revision": [3, 12], "idle_high": false}},
                    {"select": {"port": 11, "pin": 3, "active_high": false},
                     "part": {"kind": "lsm6ds-configuration-registers"}}
                ]}))
                .map_err(|e| e.to_string())?,
            );
        }
        if w.kind == "spi-controller" {
            w.options.entry("source_hz").or_insert(toml_int(48_000_000));
            if !w.options.contains_key("endpoints") {
                let block = selection.soc.blocks.iter().find(|b| b.base == w.base).map(|b| b.id.as_str());
                let sink = |port: u32, pin: u32| json!({"select": {"port": port, "pin": pin, "active_high": false},
                                                        "part": {"kind": "generic-sink", "miso_high": false}});
                let endpoints = match block {
                    // Body wiring: spi8 -> power-controller, select GPIO 15/15 active high.
                    // Run inputs: power-controller clock 845151906 s, valid.
                    // stage1 core 1 polls register 0 until bit 15 is
                    // set (ready), then reads register 0x10 and stores its low
                    // byte's bits as the wake cause (boot-info +0x150); image 4
                    // powers down unless bit 1 (power switch) is set.
                    // The handoff start synthesizes this field instead. Meanings
                    // are inferred from the code paths, not from the part.
                    Some("spi8") => json!([{"select": {"port": 15, "pin": 15, "active_high": true},
                            "part": {"kind": "power-controller", "rtc_seconds": 845151906u32, "rtc_valid": true,
                                     "registers": if matches!(args.start.as_str(), "stage0" | "sdcard") { json!({"0": 0x8202, "10": 0x0002}) } else { json!({}) }}}]),
                    // Traced X-T5 chip selects; parts unidentified.
                    Some("spi4") => json!([sink(12, 7)]),
                    Some("spi5") => json!([sink(12, 8)]),
                    // No GPIO select traced (controller-native select, like the X100VI's
                    // imaging serial output): bind to an unused, always-low latch.
                    Some("spi3") => json!([sink(19, 15)]),
                    _ => json!([]),
                };
                w.options.insert("endpoints".into(), toml::Value::try_from(endpoints).map_err(|e| e.to_string())?);
            }
        }
        if w.kind == "flash-storage-controller" {
            // catalog/parts/xt5-serial-nor: jedec_id 2145056 (0x20bb20).
            w.options.entry("jedec_id").or_insert(toml_int(0x20bb20));
        }
        if w.kind == "imaging-resize" {
            w.options.entry("completion_us").or_insert(toml_int(1000));
        }
        if w.kind == "gpio" && !w.options.contains_key("feedback") {
            // GPIO 4/4 is the first image-sensor power enable (power table
            // index 1, driven by the sensor power-up sequence). The
            // REC task reads the same pad back to tell whether the
            // sensor is already powered, so the read must follow the drive.
            w.options.insert(
                "feedback".into(),
                toml::Value::try_from(json!([{"port": 4, "register": 0, "bit": 4,
                                              "input_port": 4, "input_pin": 4}]))
                .map_err(|e| e.to_string())?,
            );
        }
        if w.kind == "imaging-receiver" {
            // The REC start sequence gives up after about 1 s without a frame, so
            // the receiver must deliver frames at a camera-like rate (default 30 fps).
            let period = std::env::var("XT5_FRAME_PERIOD_US")
                .ok()
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(33_333);
            w.options.insert("frame_period_us".into(), toml_int(period));
        }
    }
    for w in windows.iter_mut().filter(|w| w.kind == "flash-storage") {
        w.options.insert("erased_tail".into(), toml_int(0xff));
    }
    if std::env::var_os("XT5_DEBUG_WINDOWS").is_some() {
        for w in &windows {
            if !w.options.is_empty() || w.kind.contains("spi") {
                eprintln!("window {} {:#x} {:?}", w.kind, w.base, w.options);
            }
        }
    }
    // Bring-up stubs for blocks the stock setup polls and the catalog does not
    // model. Each one answers a register with a scripted value; the real
    // hardware behaviour behind them is unknown (see bringup_stub.rs).
    if matches!(args.start.as_str(), "stage0" | "sdcard") && !args.no_stubs {
        let stub = |base: u64, registers: Vec<Value>| -> Result<fujimod_emulator_devices::graph::DeviceWindow, String> {
            let options = toml::Value::try_from(json!({ "registers": registers })).map_err(|e| e.to_string())?;
            let toml::Value::Table(options) = options else { return Err("stub options".into()) };
            Ok(fujimod_emulator_devices::graph::DeviceWindow { base, size: 0x1000, kind: "bringup-stub".into(), options })
        };
        // DDR PHY mailbox polled by stage0: status +0x08 bit0 must read 0,
        // then 1 after the +0x62 acknowledge; message +0x64 == 7 ends training.
        for base in [0xfb1a_0000u64, 0xfb5a_0000] {
            windows.push(stub(base, vec![
                json!({"offset": 0x08, "mode": "toggle", "value": 1}),
                json!({"offset": 0x64, "mode": "constant", "value": 7}),
            ])?);
        }
        // stage0's memory-controller setup: 0xfaf07780 must read non-zero after 0xfaf07680 is
        // written; 0xfaf07784 low byte is a result code (0 keeps the later
        // kernel path from writing a flash configuration page).
        // The same block is polled at +0x7800 (`& 1 == 1`, stage0 generic poll
        // over the four memory-configuration bases 0xfaf00000..0xfaf30000).
        for base in [0xfaf0_7000u64, 0xfaf1_7000, 0xfaf2_7000, 0xfaf3_7000] {
            windows.push(stub(base, vec![
                json!({"offset": 0x780, "mode": "after_write", "value": 1}),
                json!({"offset": 0x784, "mode": "constant", "value": 0}),
                json!({"offset": 0x800, "mode": "constant", "value": 1}),
            ])?);
        }
        if let Some(ms) = args.wake_ms {
            let options = toml::Value::try_from(json!({"registers": [], "wake_tick": ms * 1_000_000, "wake_cores": [1, 2, 3]}))
                .map_err(|e| e.to_string())?;
            let toml::Value::Table(options) = options else { return Err("wake options".into()) };
            windows.push(fujimod_emulator_devices::graph::DeviceWindow { base: 0xfaf4_7000, size: 0x1000, kind: "bringup-stub".into(), options });
        }
    }
    // Caller stubs apply to every start mode (for example the SD host vendor
    // block 0xff9b2000, whose +0x22c the OS polls after writing 1).
    for (base, size, registers) in &args.stubs {
        let options = toml::Value::try_from(json!({ "registers": registers })).map_err(|e| e.to_string())?;
        let toml::Value::Table(options) = options else { return Err("stub options".into()) };
        windows.retain(|w| w.base != *base);
        windows.push(fujimod_emulator_devices::graph::DeviceWindow { base: *base, size: *size, kind: "bringup-stub".into(), options });
    }
    // SD host for the card start. The XP5 catalog declares no SD controller;
    // the base is fujimod-recovery's XP5 `board.host` (0xff9b0000). The
    // emulator's SDHCI model stands in for the real host, whose register map
    // is unverified.
    const SD_HOST: u64 = 0xff9b_0000;
    let mut sd_media = BTreeMap::new();
    if args.start == "sdcard" {
        let dir = args.sd_dir.as_ref().ok_or("--start sdcard requires --sd-dir DIR")?;
        // The card loader enables the host interrupt output; route it to an
        // otherwise unused GIC line (the loader polls, nothing consumes it).
        let options = toml::Value::try_from(json!({"controller": {
            "base": SD_HOST, "protocol": "sdhc", "transfer_ticks": 4, "clock_ticks": 2, "rca": 1,
            "ram_bytes": DRAM_LIMIT, "high_speed": false,
        }, "irq": 500})).map_err(|e| e.to_string())?;
        let toml::Value::Table(options) = options else { return Err("sdhci options".into()) };
        windows.push(fujimod_emulator_devices::graph::DeviceWindow { base: SD_HOST, size: 0x1000, kind: "sdhci".into(), options });
        let mut assets = BTreeMap::new();
        let mut files = vec![];
        let mut names: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file()).collect();
        names.sort();
        for path in names {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            eprintln!("card file {name}: {} bytes", bytes.len());
            assets.insert(name.clone(), bytes);
            files.push((name.clone(), name));
        }
        if !assets.contains_key("marble_init.bin") {
            return Err(format!("{}: no marble_init.bin", dir.display()));
        }
        let inputs = xp5_board::sd::MediaInputs {
            base: SD_HOST,
            format: fujimod_emulator_storage::fat::Options {
                sectors: 0x80_0000,
                max_dirty_sectors: 0x100000,
                partition_start: 8192,
                sectors_per_cluster: 64,
                label: "BOOT".into(),
                read_only_files: vec![],
            },
            files,
            present: true,
            protected: false,
        };
        sd_media = xp5_board::sd::resolve(vec![inputs], &assets)?;
    }
    // Host card images: only the non-zero sectors are loaded into the sparse
    // card part. Each image gets its own SDHCI host window; the base is the
    // caller's (trace the OS to find the slot's host).
    for (index, (base, size, irq, path)) in args.cards.iter().enumerate() {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if bytes.len() % 512 != 0 { return Err(format!("{}: not a whole number of sectors", path.display())); }
        let sectors = u32::try_from(bytes.len() / 512).map_err(|_| "card image too large")?;
        let mut card = fujimod_emulator_devices::parts::sd_card::Card::new(
            fujimod_emulator_devices::parts::sd_card::Options { sectors, max_dirty_sectors: 0x10_0000 },
        )?;
        let mut loaded = 0u64;
        for (lba, sector) in bytes.as_chunks::<512>().0.iter().enumerate() {
            if sector.iter().any(|b| *b != 0) {
                card.write_bytes(lba as u64 * 512, sector)?;
                loaded += 1;
            }
        }
        eprintln!("card {}: {} sectors, {loaded} non-zero, host {base:#x}", path.display(), sectors);
        let options = toml::Value::try_from(json!({"controller": {
            "base": base, "protocol": "sdhc", "transfer_ticks": 4, "clock_ticks": 2, "rca": 1,
            "ram_bytes": DRAM_LIMIT, "high_speed": false,
        }, "irq": irq.unwrap_or(500 + index as u64)})).map_err(|e| e.to_string())?;
        let toml::Value::Table(options) = options else { return Err("sdhci options".into()) };
        windows.retain(|w| w.base != *base);
        windows.push(fujimod_emulator_devices::graph::DeviceWindow { base: *base, size: *size, kind: "sdhci".into(), options });
        sd_media.insert(*base, xp5_board::sd::Media {
            card: Some(card),
            contacts: fujimod_emulator_devices::parts::card_contacts::Contacts::new(true, false),
        });
    }
    let mut displays = BTreeMap::new();
    for w in windows.iter().filter(|w| w.kind == "display") {
        displays.insert(w.base, display::Config { period_ns: None, raw_formats: [None; 4] });
    }
    let storage = flash_storage::Storage::new(flash.clone())?;
    let resources = xp5_board::Resources {
        sd: sd_media,
        controls: Arc::new(fujimod_emulator_parts_controls::absent::AbsentControls),
        pins: fujimod_emulator_devices::xp5::pins::Pins::shared(),
        clocks: fujimod_emulator_devices::xp5::panel_clocks::Clocks::new(),
        companion: Some(xp5_board::Companion {
            // catalog/soc/xp5 hardware.companion: core4, mailbox lines.
            pins: fujimod_emulator_xp5_io::mailbox::Pins {
                companion: 4,
                request: 55,
                control: 58,
                reply: 141,
                status: 142,
                fault: 143,
            },
            channel: Default::default(),
        }),
        storage: Some(flash_storage::Medium::Nor(Arc::new(Mutex::new(storage)))),
        displays,
    };

    let mut images = vec![];
    if args.start == "stage0" {
        // Header word 0x10 is the ROM copy length; the copy starts after the
        // 0x800-byte header and lands at the SRAM base.
        let size = u32::from_le_bytes(flash[0x10..0x14].try_into().unwrap()) as usize & 0x3fff_ffff;
        images.push(boot::MemoryImage { address: BOOT_SRAM, bytes: flash[0x800..0x800 + size].to_vec() });
    }
    if args.start == "sdcard" {
        let path = args.sd_dir.as_ref().unwrap().join("marble_init.bin");
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        images.push(boot::MemoryImage { address: BOOT_SRAM, bytes });
    }
    {
        let mut xip = flash.clone();
        xip.resize(FLASH_SIZE, 0xff);
        images.push(boot::MemoryImage { address: FLASH_BASE, bytes: xip });
    }
    for (address, path) in &args.images {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        images.push(boot::MemoryImage { address: *address, bytes });
    }

    let (mut session, inputs) = boot::open(boot::OpenOptions {
        definition: json!({"bringup": CONFIGURATION}),
        mode: args.mode,
        processors: graph,
        windows,
        controls: fujimod_emulator_xp5_board::ladder_controls::ladder_keys(&resolved)?
            .map(fujimod_emulator_parts_controls::key_input::ControlsOptions::LadderKeys),
        resources,
        engine: args.engine,
        interrupts: startup.interrupts,
        aprofile: startup.aprofile,
        images: vec![],
        scene: None,
    })?;
    let board = &mut session.board;
    eprintln!("opened: omitted {omitted:?}");
    for image in &images {
        // Core 0's view: boot SRAM is not on the DMA bus map.
        board
            .write_physical(image.address, &image.bytes)
            .or_else(|_| board.write_memory(0, image.address, &image.bytes))
            .map_err(|e| format!("image at {:#x}: {e}", image.address))?;
        eprintln!("loaded {:#x} bytes at {:#x}", image.bytes.len(), image.address);
    }

    if let Some(path) = &args.load_snapshot {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let snapshot = session.import_snapshot(&bytes)?;
        session.restore(&snapshot)?;
        eprintln!("restored snapshot {} at t={}", path.display(), session.board.now());
    }
    // The sensor power enable (GPIO 4/4) reads low until the power sequence
    // drives it; the catalog default of high makes the REC task skip sensor
    // power-up. --gpio 4:4=1 restores the old behavior.
    let mut gpio_inputs = vec![(4usize, 4usize, false)];
    gpio_inputs.retain(|&(p, q, _)| !args.gpio_inputs.iter().any(|&(a, b, _)| (a, b) == (p, q)));
    gpio_inputs.extend(args.gpio_inputs.iter().copied());
    for &(port, pin, high) in &gpio_inputs {
        inputs.gpio(&mut session, port, pin, high)?;
        eprintln!("gpio input {port}:{pin} = {}", high as u8);
    }
    // Schedule key presses after any snapshot restore, which replaces the input queue.
    for (at_ms, key, hold_ms) in &args.presses {
        let key = fujimod_emulator_xp5_board::session::Key::parse(key)?;
        inputs.key(&mut session, at_ms * 1_000_000, key, true)?;
        inputs.key(&mut session, (at_ms + hold_ms) * 1_000_000, key, false)?;
    }
    let board = &mut session.board;
    if let Some(policy) = &args.policy {
        board.set_execution_policy(fujimod_emulator_cpu::ExecutionPolicy::parse(policy)?)?;
    }
    eprintln!("execution policy: {:?}", board.execution_policy());
    for (address, value) in &args.pokes {
        // RAM first; otherwise through core 0's bus view so device registers
        // (for example GICR_WAKER) can be poked too.
        board
            .write_physical(*address, &value.to_le_bytes())
            .or_else(|_| board.write_memory(0, *address, &value.to_le_bytes()))
            .map_err(|e| format!("poke {address:#x}: {e}"))?;
    }
    // The boot ROM's register-init table, applied before the loader runs.
    if args.start == "stage0" {
        let count = u32::from_le_bytes(flash[0x14..0x18].try_into().unwrap()) as usize;
        for i in 0..count {
            let at = 0x18 + i * 8;
            let address = u32::from_le_bytes(flash[at..at + 4].try_into().unwrap()) as u64;
            let value = u32::from_le_bytes(flash[at + 4..at + 8].try_into().unwrap());
            match board.write_physical(address, &value.to_le_bytes()) {
                Ok(()) => eprintln!("init {address:#x} <- {value:#x}"),
                Err(e) => eprintln!("init {address:#x} <- {value:#x} FAILED: {e}"),
            }
        }
    }

    if !args.watch.is_empty() {
        let mut w = args.watch.clone();
        w.sort_unstable();
        w.dedup();
        board.watch_pcs(&w)?;
    }
    if !args.watch_ranges.is_empty() {
        board.transaction_watch_begin(args.watch_ranges.clone(), args.trace_limit)?;
    }
    let mut tracing = false;
    if args.trace.is_some() && args.trace_after_ms == 0 && args.watch_ranges.is_empty() {
        board.observe_transactions(Some(args.trace_limit));
        tracing = true;
    }
    let cores = 5usize;
    let started = Instant::now();
    let mut ran = 0u64;
    let mut result = Ok(());
    let mut last_pcs = vec![0u64; cores];
    let mut stall = 0u32;
    let mut latest: BTreeMap<usize, fujimod_emulator_devices::graphics::scanout::Frame> = BTreeMap::new();
    let mut frame_count: BTreeMap<usize, u64> = BTreeMap::new();
    'outer: while ran < args.instructions {
        let step = args.chunk.min(args.instructions - ran);
        if let Err(e) = board.run_for_instructions(step, args.quantum) {
            result = Err(e);
            break;
        }
        ran += step;
        if !tracing && args.watch_ranges.is_empty() && args.trace.is_some() && board.now() >= args.trace_after_ms * 1_000_000 {
            board.observe_transactions(Some(args.trace_limit));
            tracing = true;
            eprintln!("tracing from t={}", board.now());
        }
        for frame in board.take_frames() {
            *frame_count.entry(frame.output).or_default() += 1;
            if let (true, Some(dir)) = (args.all_frames, &args.frames_dir) {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                let rgb: Vec<u8> = frame
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| [p[0], p[1], p[2]])
                    .collect();
                let png = fujimod_recording::Frame::rgb(frame.width, frame.height, rgb)?
                    .png()
                    .map(|p| p.to_vec())?;
                let path = dir.join(format!(
                    "output{}-seq{:04}-t{}ms.png",
                    frame.output,
                    frame.sequence,
                    board.now() / 1_000_000
                ));
                std::fs::write(&path, png).map_err(|e| e.to_string())?;
            }
            latest.insert(frame.output, frame);
        }
        for line in board.take_console() {
            println!("console: {line}");
        }
        let pcs: Vec<u64> = (0..cores)
            .map(|c| board.read_register(c, Register::Pc).unwrap_or(0))
            .collect();
        if !args.quiet {
            let counts = board.instruction_counts();
            eprintln!(
                "[{:>12}] pcs {} counts {:?} t={}",
                ran,
                pcs.iter().map(|p| format!("{p:#x}")).collect::<Vec<_>>().join(" "),
                counts,
                board.now()
            );
        }
        for (core, pc) in pcs.iter().enumerate() {
            if args.stop_at.contains(pc) {
                eprintln!("core {core} reached stop address {pc:#x}");
                break 'outer;
            }
        }
        if pcs == last_pcs {
            stall += 1;
            if stall >= 5 {
                eprintln!("PCs unchanged for 5 chunks; stopping");
                break;
            }
        } else {
            stall = 0;
        }
        last_pcs = pcs;
    }
    let elapsed = started.elapsed();
    for (address, len, path) in &args.dump_files {
        let mut bytes = vec![0u8; *len];
        board.read_physical(*address, &mut bytes).map_err(|e| format!("dump {address:#x}: {e}"))?;
        std::fs::write(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        eprintln!("dumped {len:#x} bytes at {address:#x} to {}", path.display());
    }
    for (address, len) in &args.dumps {
        let mut bytes = vec![0u8; *len];
        match board.read_physical(*address, &mut bytes) {
            Ok(()) => {
                for (i, chunk) in bytes.chunks(16).enumerate() {
                    let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
                    eprintln!("dump {:#012x}: {}", address + (i * 16) as u64, hex.join(" "));
                }
            }
            Err(e) => eprintln!("dump {address:#x}: {e}"),
        }
    }
    let state = board.state();
    let report = json!({
        "result": match &result { Ok(()) => "ok".to_string(), Err(e) => e.clone() },
        "instructions": ran,
        "seconds": elapsed.as_secs_f64(),
        "guest_time": board.now(),
        "cores": (0..cores).map(|c| snapshot(board, c)).collect::<Vec<_>>(),
        "failure": state.get("failure"),
        "unmodeled_system": state.get("unmodeled_system"),
        "released": state.get("released"),
        "watched": board.pc_entries().iter().map(|e| json!({
            "core": e["core"], "pc": format!("{:#x}", e["pc"].as_u64().unwrap_or(0)),
            "x0_3": e["arguments"].as_array().map(|a| a.iter().map(|v| format!("{:#x}", v.as_u64().unwrap_or(0))).collect::<Vec<_>>()),
            "tick": e["tick"],
            "sp": format!("{:#x}", e["sp"].as_u64().unwrap_or(0)),
            "backtrace": e["backtrace"].as_array().map(|a| a.iter().map(|v| format!("{:#x}", v.as_u64().unwrap_or(0))).collect::<Vec<_>>()),
        })).collect::<Vec<_>>(),
        "waiting": state.get("waiting"),
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    if let Some(dir) = &args.frames_dir {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        for (output, frame) in &latest {
            let rgb: Vec<u8> = frame
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect();
            let png = fujimod_recording::Frame::rgb(frame.width, frame.height, rgb)?
                .png()
                .map(|p| p.to_vec())?;
            let path = dir.join(format!("output{output}.png"));
            std::fs::write(&path, png).map_err(|e| e.to_string())?;
            eprintln!("frame output {output}: {}x{} seq {} -> {}", frame.width, frame.height, frame.sequence, path.display());
        }
    }
    eprintln!("frames per output: {frame_count:?}");
    if let Ok(spec) = std::env::var("XT5_DUMP_CORE") {
        // CORE:ADDR:LEN:PATH, read through that core's view.
        let p: Vec<&str> = spec.splitn(4, ':').collect();
        if p.len() != 4 {
            return Err("XT5_DUMP_CORE must be CORE:ADDR:LEN:PATH".into());
        }
        let (core, addr, len) = (parse_u64(p[0])? as usize, parse_u64(p[1])?, parse_u64(p[2])? as usize);
        let mut bytes = vec![0u8; len];
        board.read_memory(core, addr, &mut bytes).map_err(|e| format!("core dump: {e}"))?;
        std::fs::write(p[3], &bytes).map_err(|e| e.to_string())?;
        eprintln!("dumped core {core} {addr:#x}+{len:#x} -> {}", p[3]);
    }
    if let Ok(kinds) = std::env::var("XT5_DEVICE_STATES") {
        for kind in kinds.split(',') {
            eprintln!("device {kind}: {}", serde_json::to_string(&board.device_states(kind)?).unwrap());
        }
    }
    if let Ok(path) = std::env::var("XT5_GIC_OUT") {
        std::fs::write(&path, serde_json::to_string_pretty(&board.gic_state()?).unwrap()).map_err(|e| e.to_string())?;
    }
    if !args.watch_ranges.is_empty() {
        let batch = board.transaction_watch_end()?;
        let path = args.trace.clone().unwrap_or_else(|| "watch.json".into());
        std::fs::write(&path, serde_json::to_string(&batch).unwrap()).map_err(|e| e.to_string())?;
        eprintln!("watch: {} transactions, {} omitted -> {}", batch.transactions.len(), batch.omitted, path.display());
    } else if let Some(path) = &args.trace {
        std::fs::write(path, serde_json::to_string(&board.transactions()).unwrap())
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let Some(path) = &args.state_out {
        std::fs::write(path, serde_json::to_string_pretty(&state).unwrap())
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let Some(path) = &args.save_snapshot {
        if result.is_ok() {
            let bytes = session.checkpoint(None)?.export()?;
            std::fs::write(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            eprintln!("saved snapshot {} ({} bytes)", path.display(), bytes.len());
        } else {
            eprintln!("not saving a snapshot of a failed run");
        }
    }
    Ok(())
}

fn toml_int(v: i64) -> toml::Value {
    toml::Value::Integer(v)
}

fn fujimod_settings_free_flash(update: &[u8], origin: u64) -> Result<Vec<u8>, String> {
    let origin = usize::try_from(origin).map_err(|_| "flash origin overflow")?;
    let installed = update.get(origin..).ok_or("update shorter than flash origin")?;
    Ok(fujimod_firmware::invert(installed))
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
