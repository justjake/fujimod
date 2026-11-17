//! The XP5 bring-up run: open a session from explicit inputs, drive it in slices and record
//! the outcome in the run directory.
use crate::{
    emulator_patch,
    firmware::Firmware,
    manifest::BootState,
    mod_loader, open as session_open, rom_start,
};
use fujimod_emulator_cpu::ExecutionPolicy;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn parse_number(s: &str) -> Result<u64, String> {
    if let Some(s) = s.strip_prefix("0x") {
        u64::from_str_radix(s, 16).map_err(|e| e.to_string())
    } else {
        s.parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())
    }
}
/// Instructions per scheduler turn for `--quantum`; replaces the run
/// configuration's `execution.quantum` for this run only.
fn quantum(s: &str) -> Result<usize, String> {
    let value = usize::try_from(parse_number(s)?).map_err(|error| error.to_string())?;
    if value == 0 {
        return Err("--quantum needs a positive instruction count".into());
    }
    Ok(value)
}
fn seconds(s: &str) -> Result<Duration, String> {
    let n = s.parse::<f64>().map_err(|e| e.to_string())?;
    Duration::try_from_secs_f64(n).map_err(|e| e.to_string())
}

fn shared_boot_summary(
    state: &Value,
    board: &fujimod_emulator::board::Board,
    policy: ExecutionPolicy,
) -> Value {
    let pcs = state["released"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(core, _)| {
            let (pc, lr, x0_name, x0) = if core == 4 {
                (
                    fujimod_emulator_cpu::Register::Arm32(fujimod_emulator_cpu::arm::Register::R(
                        15,
                    )),
                    fujimod_emulator_cpu::Register::Arm32(fujimod_emulator_cpu::arm::Register::R(
                        14,
                    )),
                    "r0",
                    fujimod_emulator_cpu::Register::Arm32(fujimod_emulator_cpu::arm::Register::R(
                        0,
                    )),
                )
            } else {
                (
                    fujimod_emulator_cpu::Register::Pc,
                    fujimod_emulator_cpu::Register::X(30),
                    "x0",
                    fujimod_emulator_cpu::Register::X(0),
                )
            };
            let mut summary = serde_json::Map::new();
            summary.insert("cpu".into(), json!(core));
            for (name, register) in [("pc", pc), ("lr", lr), (x0_name, x0)] {
                let (key, value) = match board.read_register(core, register) {
                    Ok(value) => (name.to_owned(), json!(format!("{value:#x}"))),
                    Err(error) => (format!("{name}_error"), json!(error)),
                };
                summary.insert(key, value);
            }
            Value::Object(summary)
        })
        .collect::<Vec<_>>();
    json!({
        "execution_policy": policy.name(),
        "execution_mode": state["execution_mode"],
        "counts_as_boot": state["counts_as_boot"],
        "now": state["timeline"]["now"],
        "instructions": state["instructions"],
        "serviced_mmio_trap_callbacks_by_core": board.mmio_trap_callback_counts(),
        "interactive_window_count": board.interactive_window_count(),
        "mean_interactive_window_host_duration_ns":
            board.mean_interactive_window_host_duration_ns(),
        "released": state["released"],
        "waiting": state["waiting"],
        "pcs": pcs,
        "discovery_callee_stubs": state["discovery_callee_stubs"],
        "unmodeled": state["bus"]["unmodeled"],
        "frontier": state["bus"]["frontier"],
    })
}


/// One XP5 boot: the run configuration plus the caller's input files.
pub struct Run {
    pub configuration: String,
    pub overlays: Vec<String>,
    /// The original update file.
    pub update: PathBuf,
    /// The boot ROM, for a configuration that starts from it.
    pub boot_rom: Option<PathBuf>,
    /// A raw SD card image in place of the in-memory volume (ROM start only).
    pub card_image: Option<PathBuf>,
    /// Files on the in-memory SD card, as `(name, path)` (ROM start only).
    pub card_files: Vec<(String, PathBuf)>,
    /// Scripted key presses: `t=SECONDS:KEY;...` (a leading `-` on KEY releases it).
    pub keys: Option<String>,
    /// Route unmodeled register accesses to the bus instead of stopping.
    pub discovery: bool,
    /// Discovery callee stubs: `NAME@ADDRESS=VALUE`.
    pub stubs: Vec<String>,
    /// Guest-memory patch file.
    pub patch_file: Option<PathBuf>,
    /// Built mod manifests.
    pub mods: Vec<String>,
    /// Scene file replacing the configuration's scene.
    pub scene: Option<PathBuf>,
    /// Instructions per scheduler turn, replacing the configuration's.
    pub quantum: Option<usize>,
    /// Stop after this much guest time.
    pub seconds: Option<Duration>,
    pub status_every: Duration,
    pub execution_policy: ExecutionPolicy,
    /// Directory for the run's records (display frames, board state, dumps).
    pub output: PathBuf,
}

pub fn parse_stub(specification: &str) -> Result<fujimod_emulator::board::discovery::CalleeStub, String> {
    let (name, rest) = specification
        .split_once('@')
        .ok_or("--stub needs NAME@ADDR=VALUE")?;
    let (address, value) = rest.split_once('=').ok_or("--stub needs NAME@ADDR=VALUE")?;
    if name.is_empty() {
        return Err("discovery callee stub name is empty".into());
    }
    Ok(fujimod_emulator::board::discovery::CalleeStub {
        name: name.to_owned(),
        core: 0,
        address: parse_number(address)?,
        value: parse_number(value)?,
    })
}

pub fn parse_quantum(text: &str) -> Result<usize, String> {
    quantum(text)
}

pub fn parse_seconds(text: &str) -> Result<Duration, String> {
    seconds(text)
}

/// Run the boot described by `run`.
pub fn run_boot(run: Run) -> Result<Value, String> {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let Run {
        configuration,
        overlays,
        update,
        boot_rom: rom_path,
        card_image: sd_image,
        card_files,
        keys: key_script,
        discovery,
        stubs,
        patch_file,
        mods: mod_specs,
        scene,
        quantum: quantum_override,
        seconds: limit,
        status_every,
        execution_policy: policy,
        output: run_directory,
    } = run;
    let mode = if discovery {
        fujimod_emulator::board::bus::Mode::Discovery
    } else {
        fujimod_emulator::board::bus::Mode::Strict
    };
    let callee_stubs = stubs
        .iter()
        .map(|stub| parse_stub(stub))
        .collect::<Result<Vec<_>, _>>()?;
    let resolved_start = fujimod_catalog_resolution::ResolvedConfiguration::with_overlays(&configuration, &overlays)?;
    let start_rom = matches!(
        resolved_start.configuration.start,
        fujimod_catalog_configuration::RunStart::Rom
    );
    let dials_held = true;
    let card_present = true;
    let scene = scene;
    // Input-only parsing comes first, so a malformed spec fails before the run directory exists
    // and before the board is opened. Patch times stay in milliseconds and the key delays stay
    // durations until the run starts and its tick rate is known.
    // FUJIMOD_XP5_WATCH_PCS=ADDR,ADDR,...: even instruction addresses whose first entry on each
    // core (tick, r0..r3, backtrace) is printed when the run ends or stops. The hooks are
    // installed on every core, so read each entry's "core" field to tell an M4 hit from an A53
    // one (core 4 is the M4). A Thumb function at an odd address is named by its even address.
    let watch_pcs = match std::env::var("FUJIMOD_XP5_WATCH_PCS") {
        Ok(spec) => {
            let mut addresses = spec
                .split(',')
                .filter(|item| !item.trim().is_empty())
                .map(|item| parse_number(item.trim()))
                .collect::<Result<Vec<_>, _>>()?;
            addresses.sort_unstable();
            addresses.dedup();
            addresses
        }
        Err(_) => Vec::new(),
    };
    let dumps = parse_dumps()?;
    // Emulator-only guest-memory patches (--emulator-patch FILE), the way the X-T5 runner patches
    // the M4 image in memory. Each entry names the bytes it expects; the run stops if the guest
    // holds anything else. They change guest RAM of this run only and never produce an update file.
    let mut patches = match &patch_file {
        Some(path) => emulator_patch::load(path)?,
        None => Vec::new(),
    };
    let mut keys = Vec::new();
    if let Some(script) = &key_script {
        for entry in script.split(';').filter(|entry| !entry.trim().is_empty()) {
            let (time, name) = entry
                .trim()
                .split_once(':')
                .ok_or("keys need t=SECONDS:KEY")?;
            let delay = seconds(time.strip_prefix("t=").ok_or("keys need t=SECONDS:KEY")?)?;
            let (pressed, name) = name
                .strip_prefix('-')
                .map_or((true, name), |name| (false, name));
            let key = fujimod_emulator_parts_controls::resistor_keys::Key::parse(name)?;
            keys.push((delay, key, pressed));
        }
    }
    let firmware_id = resolved_start.configuration.firmware_id.clone();
    let state = BootState::for_firmware(&firmware_id)?;
    let update_bytes = std::fs::read(&update).map_err(|e| format!("{}: {e}", update.display()))?;
    let firmware = Firmware::from_update(&firmware_id, update_bytes, true)?;
    std::fs::create_dir_all(&run_directory).map_err(|error| error.to_string())?;
    let rom = if start_rom {
        let start = rom_start::RomStart::load(
            rom_path.as_deref().ok_or("--start rom needs --rom FILE")?,
            dials_held,
            card_files,
            card_present,
        )?;
        Some(match &sd_image {
            Some(image) => {
                let start = start.with_card_image(image)?;
                if let Some(card) = &start.card_image {
                    for line in &card.layout {
                        eprintln!("FUJIMOD card image {}: {line}", image.display());
                    }
                    eprintln!("FUJIMOD card image root names: {:?}", card.names);
                }
                start
            }
            None => start,
        })
    } else if rom_path.is_some()
        || sd_image.is_some()
        || !card_files.is_empty()
        || !card_present
        || !dials_held
    {
        return Err("--rom, --sd-file, --sd-image, --sd-absent and --dials apply to --start rom only".into());
    } else {
        None
    };
    let opened = session_open::open_from(
        &firmware,
        &state,
        &configuration,
        &overlays,
        scene.as_deref(),
        &run_directory,
        mode,
        quantum_override,
        rom.as_ref(),
    )?;
    let session_open::Opened {
        flash_shadow,
        mut session,
        inputs,
        outputs,
        ticks_per_second,
        quantum,
    } = opened;
    session
        .board
        .install_discovery_callee_stubs(&callee_stubs)?;
    session.board.set_execution_policy(policy)?;
    session.board.watch_pcs(&watch_pcs)?;
    // Emulator-only FujiMod (--mod): built and placed before the run, hook words attached between
    // run slices once the stock UILIB-ready flag holds. See mod_loader.rs.
    let mut fujimod = if mod_specs.is_empty() {
        None
    } else {
        let mut loaded = mod_loader::prepare(&firmware, &mod_specs)?;
        let stubbed: Vec<u64> = callee_stubs.iter().map(|stub| stub.address).collect();
        loaded.place(&mod_loader::BoardMachine(&session.board), &firmware, &state.preload, &stubbed)?;
        Some(loaded)
    };
    let start = session.board.now();
    for (delay, key, pressed) in keys {
        let delay_ticks = (delay.as_secs_f64() * ticks_per_second as f64) as u64;
        inputs.key(&mut session, start.saturating_add(delay_ticks), key, pressed)?;
    }
    let interrupted = Arc::new(AtomicBool::new(false));
    let signal = interrupted.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Release))
        .map_err(|error| error.to_string())?;
    let stop_at = limit.map(|duration| {
        start.saturating_add((duration.as_secs_f64() * ticks_per_second as f64) as u64)
    });
    let slice = ((ticks_per_second / 1_000).max(1)).min(100_000_000);
    let mut next_status =
        start.saturating_add((status_every.as_secs_f64() * ticks_per_second as f64) as u64);
    eprintln!(
        "FUJIMOD XP5 session configuration={configuration} firmware={} policy={} quantum={quantum} run={}",
        firmware.id,
        policy.name(),
        run_directory.display()
    );
    let mut drive = || -> Result<(), String> {
        loop {
            if interrupted.load(Ordering::Acquire) {
                break;
            }
            let now = session.board.now();
            if stop_at.is_some_and(|deadline| now >= deadline) {
                break;
            }
            while patches
                .first()
                .is_some_and(|patch| now.saturating_sub(start) >= patch.at_ticks(ticks_per_second))
            {
                let patch = patches.remove(0);
                patch.apply(&mut session.board, now)?;
            }
            if let Some(loaded) = fujimod.as_mut() {
                loaded.attach_if_ready(&mod_loader::BoardMachine(&session.board), now)?;
            }
            let mut until = now.saturating_add(slice);
            if let Some(deadline) = stop_at {
                until = until.min(deadline);
            }
            let result = session.board.run_until(until, quantum);
            if let Some(storage) = &flash_shadow {
                session_open::sync_flash_aperture(storage, &mut session)?;
            }
            for line in session.board.take_console() {
                println!("{line}");
            }
            for frame in session.board.take_frames() {
                outputs.frame(frame)?;
            }
            if let Err(error) = result {
                let state = session.board.state();
                // Where every core stood when the board refused, for run records.
                eprintln!(
                    "FUJIMOD XP5 stop: {}",
                    shared_boot_summary(&state, &session.board, policy)
                );
                let stop_site = state["bus"]["frontier"]
                    .as_array()
                    .and_then(|frontier| frontier.last());
                let stop_core = stop_site
                    .and_then(|site| site["cpu"].as_u64())
                    .and_then(|core| usize::try_from(core).ok());
                let mut error = error;
                if let Some(site) = stop_site {
                    if let (Some(tick), Some(pc)) = (site["tick"].as_u64(), site["pc"].as_u64()) {
                        error = format!("{error}; tick={tick} PC={pc:#x}");
                    }
                }
                if let Some(core) = stop_core {
                    let lr_register = if core == 4 {
                        fujimod_emulator_cpu::Register::Arm32(
                            fujimod_emulator_cpu::arm::Register::R(14),
                        )
                    } else {
                        fujimod_emulator_cpu::Register::X(30)
                    };
                    if let Ok(lr) = session
                        .board
                        .read_register(core, lr_register)
                    {
                        error = format!("{error}; CPU{core} LR={lr:#x}");
                    }
                }
                return Err(error);
            }
            session.board.check_discovery_callee_stubs()?;
            let now = session.board.now();
            if status_every != Duration::ZERO && now >= next_status {
                let state = session.board.state();
                eprintln!(
                    "FUJIMOD XP5 status: {}",
                    shared_boot_summary(&state, &session.board, policy)
                );
                next_status =
                    now.saturating_add((status_every.as_secs_f64() * ticks_per_second as f64) as u64);
            }
        }
        Ok(())
    };
    // One exit path for the run loop: a stop, a refused patch, an interrupt and a normal end all
    // finish the board and the outputs, record the board state, then dump and trace, so a run
    // that dies still leaves them. Setup errors above (opening the board, discovery stubs, the
    // execution policy, watch hooks, key scheduling, the Ctrl-C handler) return before anything
    // has run and leave no board state; a panic would also bypass this path.
    let outcome = drive();
    let board_finish = session.board.finish();
    let output_finish = outputs.finish();
    let state = session.board.state();
    let state_write = serde_json::to_vec_pretty(&state)
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            std::fs::write(run_directory.join("board-state.json"), bytes)
                .map_err(|error| format!("board-state.json: {error}"))
        });
    post_mortem(&session.board, &run_directory, &dumps);
    if let Some(loaded) = &fujimod {
        let report = loaded.report(&mod_loader::BoardMachine(&session.board));
        eprintln!("FUJIMOD XP5 mod status: {report}");
        if let Err(error) = std::fs::write(run_directory.join("mod-status.json"), report.to_string()) {
            eprintln!("FUJIMOD XP5 mod-status.json failed: {error}");
        }
    }
    let cleanup = [
        ("board", board_finish),
        ("outputs", output_finish),
        ("board state", state_write),
    ]
    .into_iter()
    .filter_map(|(name, result)| result.err().map(|error| format!("{name}={error}")))
    .collect::<Vec<_>>();
    if let Err(error) = outcome {
        return Err(if cleanup.is_empty() {
            error
        } else {
            format!("{error}; cleanup: {}", cleanup.join(", "))
        });
    }
    if !cleanup.is_empty() {
        return Err(format!("cleanup: {}", cleanup.join(", ")));
    }
    Ok(json!({
        "engine": "fujimod-xp5-session",
        "configuration": configuration,
        "firmware": firmware.id,
        "instructions": session.board.instruction_counts(),
        "now": session.board.now(),
        "discovery_callee_stubs": session.board.discovery_callee_stubs(),
        "run_directory": run_directory,
        "state": shared_boot_summary(&state, &session.board, policy),
    }))
}

/// FUJIMOD_XP5_DUMP=ADDR:LEN[;ADDR:LEN...] names guest physical ranges (the BIS core's local
/// addresses are 0xfa200000 + offset) to write, after the run ends or stops, to
/// `<run_directory>/dump-<address>.bin`. Parsed before the run so bad input is refused up front.
const DUMP_LIMIT: u64 = 16 << 20;

fn parse_dumps() -> Result<Vec<(u64, usize)>, String> {
    let Ok(spec) = std::env::var("FUJIMOD_XP5_DUMP") else {
        return Ok(Vec::new());
    };
    let mut dumps: Vec<(u64, usize)> = Vec::new();
    for item in spec.split(';').filter(|item| !item.is_empty()) {
        let (address, length) = item
            .split_once(':')
            .filter(|(_, length)| !length.contains(':'))
            .ok_or_else(|| format!("FUJIMOD_XP5_DUMP item {item:?} is not ADDR:LEN"))?;
        let (address, length) = (parse_number(address)?, parse_number(length)?);
        if length == 0 || length > DUMP_LIMIT {
            return Err(format!("FUJIMOD_XP5_DUMP length {length:#x} is outside 1..={DUMP_LIMIT:#x}"));
        }
        if address.checked_add(length).is_none() || dumps.iter().any(|(known, _)| *known == address) {
            return Err(format!("FUJIMOD_XP5_DUMP address {address:#x} overflows or repeats"));
        }
        dumps.push((address, length as usize));
    }
    Ok(dumps)
}

/// FUJIMOD_XP5_HASH=ADDR:LEN[;ADDR:LEN...] (decimal or 0x) names guest physical ranges of any size whose
/// SHA-256 and nonzero 4 KiB pages are printed (and written to `<run_directory>/hash.jsonl`) when
/// the run ends or stops: the check that a large range holds what it should without dumping it.
fn print_hashes(board: &fujimod_emulator::board::Board, run_directory: &Path) {
    let Ok(spec) = std::env::var("FUJIMOD_XP5_HASH") else { return };
    let mut lines = String::new();
    for item in spec.split(';').filter(|item| !item.trim().is_empty()) {
        let parsed = item
            .split_once(':')
            .and_then(|(address, length)| Some((parse_number(address.trim()).ok()?, parse_number(length.trim()).ok()?)));
        let Some((address, length)) = parsed else {
            eprintln!("FUJIMOD XP5 hash: {item:?} is not ADDR:LEN");
            continue;
        };
        let line = board.range_digest(address, length).to_string();
        eprintln!("FUJIMOD XP5 hash: {line}");
        lines += &(line + "\n");
    }
    if let Err(error) = std::fs::write(run_directory.join("hash.jsonl"), lines) {
        eprintln!("FUJIMOD XP5 hash: {error}");
    }
}

/// Writes the requested dumps and prints the recorded PC entries; failures are logged, not returned.
fn post_mortem(
    board: &fujimod_emulator::board::Board,
    run_directory: &Path,
    dumps: &[(u64, usize)],
) {
    print_hashes(board, run_directory);
    for &(address, length) in dumps {
        let mut bytes = vec![0u8; length];
        let path = run_directory.join(format!("dump-{address:x}.bin"));
        match board
            .inspect_physical(address, &mut bytes)
            .map_err(|error| error.to_string())
            .and_then(|()| std::fs::write(&path, &bytes).map_err(|error| error.to_string()))
        {
            Ok(()) => eprintln!("FUJIMOD XP5 dump: {address:#x}+{length:#x} -> {}", path.display()),
            Err(error) => eprintln!("FUJIMOD XP5 dump {address:#x}+{length:#x} failed: {error}"),
        }
    }
    for entry in board.pc_entries() {
        eprintln!("FUJIMOD XP5 pc-entry: {entry}");
    }
}
