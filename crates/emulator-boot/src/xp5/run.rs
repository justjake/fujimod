//! The XP5 bring-up run: open a session from explicit inputs, drive it in slices and record
//! the outcome in the run directory.
use super::{
    Startup, Started, open as session_open,
};
use crate::mod_loader;
use crate::host::{parse_gpio, parse_number, parse_poke, parse_stub};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_cpu::ExecutionPolicy;
use fujimod_emulator_parts_controls::key_input::Key;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

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
        "loops_by_core": board
            .loop_statistics()
            .unwrap_or_else(|error| json!({"error": error})),
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
    pub configuration: ResolvedConfiguration,
    pub load_snapshot: Option<PathBuf>,
    pub save_snapshot: Option<PathBuf>,
    /// The original update file.
    pub update: PathBuf,
    pub manifest: Option<PathBuf>,
    pub watch_pcs: Option<String>,
    pub pc_trace: Option<crate::TraceOutput>,
    pub screenshot: Option<PathBuf>,
    pub viewer_port: Option<u16>,
    pub dump: Option<String>,
    pub hash: Option<String>,
    /// The boot ROM, for a configuration that starts from it.
    pub boot_rom: Option<PathBuf>,
    /// A raw SD card image in place of the in-memory volume. ROM and SD starts load it read-only;
    /// the loader start puts it in slot 1 and writes changed sectors back at the end of the run.
    pub card_image: Option<PathBuf>,
    /// Files on the SD card, as `(name, path)`: the startup card of a ROM or SD start, or the
    /// slot-1 card of the loader start.
    pub card_files: Vec<(String, PathBuf)>,
    /// Scripted key presses: `t=SECONDS:KEY;...` (a leading `-` on KEY releases it).
    pub keys: Option<String>,
    pub gpio: Option<String>,
    pub pokes: Vec<String>,
    pub press: Option<String>,
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
    /// Optional additional instructions across all processors.
    pub instruction_limit: Option<u64>,
    /// Stop after this much host time, whatever the guest budgets have reached.
    pub wall_seconds: Option<Duration>,
    pub status_every: Duration,
    pub execution_policy: ExecutionPolicy,
    /// Directory for the run's records (display frames, board state, dumps).
    pub output: PathBuf,
}

/// Host-time cadence of status lines, so a slow run is visibly alive.
const WALL_STATUS_EVERY: Duration = Duration::from_secs(10);

/// Run the boot described by `run`.
pub fn run_boot(run: Run) -> Result<Value, String> {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let Run {
        configuration,
        load_snapshot,
        save_snapshot,
        update,
        manifest,
        watch_pcs,
        pc_trace,
        screenshot,
        viewer_port,
        dump,
        hash,
        boot_rom: rom_path,
        card_image: sd_image,
        card_files,
        keys: key_script,
        gpio: gpio_spec,
        pokes,
        press,
        discovery,
        stubs,
        patch_file,
        mods: mod_specs,
        scene,
        quantum: quantum_override,
        seconds: limit,
        instruction_limit,
        wall_seconds,
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
    let configuration_id = configuration.configuration.id.clone();
    // Input-only parsing comes first, so a malformed spec fails before the run directory exists
    // and before the board is opened. Patch times stay in milliseconds and the key delays stay
    // durations until the run starts and its tick rate is known.
    let mut watch_pcs = watch_pcs.as_deref().unwrap_or_default()
        .split(',')
        .filter(|item| !item.trim().is_empty())
        .map(|item| parse_number(item.trim()))
        .collect::<Result<Vec<_>, _>>()?;
    watch_pcs.sort_unstable();
    watch_pcs.dedup();
    let dumps = parse_dumps(dump.as_deref().unwrap_or_default())?;
    let hashes = parse_ranges("--hash", hash.as_deref().unwrap_or_default())?;
    let mut patches = crate::patch::Schedule::from_path(
        patch_file.as_deref(), &[0..0x8000_0000, 0xfa20_0000..0xfa30_0000],
    )?;
    let keys = crate::host::parse_keys(key_script.as_deref(), press.as_deref())?.into_iter()
        .map(|edge| Ok((edge.delay, Key::parse(&edge.name)?, edge.pressed)))
        .collect::<Result<Vec<_>, String>>()?;
    // `PORT:PIN=LEVEL` applies before the run; `PORT:PIN=LEVEL@MS` applies MS milliseconds of
    // guest time after the run starts (after a snapshot load, like `--press`). The X-T5 shutter
    // release has no key name: half press is `0:0=0@MS`, full press `0:1=0@MS`, release `=1@MS`.
    let mut gpio = Vec::new();
    let mut timed_gpio = Vec::new();
    for item in gpio_spec.as_deref().unwrap_or_default().split(',').filter(|item| !item.trim().is_empty()) {
        match item.trim().split_once('@') {
            Some((level, at)) => {
                let at_ms: u64 = at.trim().parse()
                    .map_err(|_| format!("--gpio {item}: the time after @ is whole milliseconds"))?;
                timed_gpio.push((at_ms, parse_gpio(level)?));
            }
            None => gpio.push(parse_gpio(item.trim())?),
        }
    }
    // A stable sort keeps the written order of levels that share a time.
    timed_gpio.sort_by_key(|(at_ms, _)| *at_ms);
    // XT5_WATCH_AFTER_MS=N arms the --watch-pcs witnesses only after N ms of guest time from the
    // run start, so the log shows the first hits after an event (a key press), not earlier ones.
    let watch_after_ms = match std::env::var("XT5_WATCH_AFTER_MS") {
        Ok(value) => Some(value.trim().parse::<u64>()
            .map_err(|_| "XT5_WATCH_AFTER_MS needs whole milliseconds".to_string())?)
            .filter(|ms| *ms > 0),
        Err(_) => None,
    };
    let pokes = pokes.iter().map(|spec| parse_poke(spec)).collect::<Result<Vec<_>, _>>()?;
    let request = Startup {
        update,
        manifest,
        boot_rom: rom_path,
        card_image: sd_image,
        card_files,
        scene,
        output: Some(run_directory.clone()),
        viewer_port,
        mode,
        quantum: quantum_override,
    };
    let crate::Opened::Xp5(started) =
        crate::open(configuration, crate::StartupInputs::Xp5(request))?
    else {
        return Err("XP5 startup returned another processor generation".into());
    };
    let Started { firmware, state, opened, card_write_back } = *started;
    let session_open::Opened {
        mut session,
        inputs,
        outputs,
        ticks_per_second,
        quantum,
        ..
    } = opened;
    if let Some(path) = load_snapshot {
        crate::load_snapshot(&mut session, &path)?;
    }
    session
        .board
        .install_discovery_callee_stubs(&callee_stubs)?;
    session.board.set_execution_policy(policy)?;
    if watch_after_ms.is_none() {
        session.board.watch_pcs(&watch_pcs)?;
    }
    let trace = pc_trace.as_ref().map(|output| output.begin(&session)).transpose()?;
    // Emulator-only FujiMod (--mod): built and placed before the run, hook words attached between
    // run slices once the stock UILIB-ready flag holds. See mod_loader.rs.
    let mut fujimod = if mod_specs.is_empty() {
        None
    } else {
        let target = mod_loader::Target {
            id: &firmware.id, sha256: &firmware.source_sha256, model: fujimod_emulator_cpu::Model::ArmV8A,
        };
        let mut loaded = mod_loader::prepare(target, &mod_specs,
            |address, word| firmware.expect_word(address, word, "mod patch site"))?;
        let mut excluded = firmware.regions.iter().map(|region| (region.address, region.bytes.len() as u64))
            .collect::<Vec<_>>();
        for preload in &state.preload {
            excluded.push((preload.address, super::manifest::preload_length(preload)?));
        }
        let stubbed: Vec<u64> = callee_stubs.iter().map(|stub| stub.address).collect();
        loaded.place(&mod_loader::BoardMachine(&session.board), &excluded, &stubbed)?;
        Some(loaded)
    };
    for (port, pin, high) in gpio {
        inputs.gpio(&mut session, usize::from(port), usize::from(pin), high)?;
    }
    for (address, value) in pokes {
        let bytes = value.to_le_bytes();
        session.board.write_physical(address, &bytes)
            .or_else(|_| session.board.write_memory(0, address, &bytes))
            .map_err(|error| format!("poke {address:#x}: {error}"))?;
    }
    let start = session.board.now();
    for (delay, key, pressed) in keys {
        let delay_ticks = crate::host::duration_ticks(delay, ticks_per_second);
        inputs.key(&mut session, start.saturating_add(delay_ticks), key, pressed)?;
    }
    let millisecond_tick = |ms: u64| {
        start.saturating_add(crate::host::duration_ticks(std::time::Duration::from_millis(ms), ticks_per_second))
    };
    let mut timed_gpio: std::collections::VecDeque<_> = timed_gpio.into_iter()
        .map(|(at_ms, level)| (millisecond_tick(at_ms), at_ms, level))
        .collect();
    let mut watch_after = watch_after_ms.map(|ms| (millisecond_tick(ms), ms));
    let interrupted = Arc::new(AtomicBool::new(false));
    let signal = interrupted.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Release))
        .map_err(|error| error.to_string())?;
    let limits = crate::host::Limits::new(&session, instruction_limit, limit, ticks_per_second)?
        .with_wall(wall_seconds);
    let mut next_wall_status = std::time::Instant::now() + WALL_STATUS_EVERY;
    let slice = ((ticks_per_second / 1_000).max(1)).min(100_000_000);
    let mut next_status =
        start.saturating_add(crate::host::duration_ticks(status_every, ticks_per_second));
    eprintln!(
        "FUJIMOD XP5 session configuration={configuration_id} firmware={} policy={} quantum={quantum} run={}",
        firmware.id,
        policy.name(),
        run_directory.display()
    );
    let outcome = limits.drive(&mut session, quantum,
        |session| {
            if interrupted.load(Ordering::Acquire) {
                return Ok(std::ops::ControlFlow::Break(()));
            }
            let now = session.board.now();
            let next_patch = patches.advance(&session.board, now, start, ticks_per_second)?;
            if let Some(loaded) = fujimod.as_mut() {
                loaded.attach_if_ready(&mod_loader::BoardMachine(&session.board), now)?;
            }
            while let Some(&(at, at_ms, (port, pin, high))) = timed_gpio.front() {
                if at > now {
                    break;
                }
                timed_gpio.pop_front();
                inputs.gpio(session, usize::from(port), usize::from(pin), high)?;
                eprintln!("FUJIMOD XP5 gpio: {port}:{pin}={} at {at_ms} ms (tick {now})", u8::from(high));
            }
            if let Some((at, after_ms)) = watch_after
                && at <= now
            {
                watch_after = None;
                session.board.watch_pcs(&watch_pcs)?;
                eprintln!("FUJIMOD XP5 watch-pcs armed at {after_ms} ms (tick {now})");
            }
            // Stop the slice at the next scheduled host action, so it applies at its guest time.
            let until = [next_patch, timed_gpio.front().map(|item| item.0), watch_after.map(|item| item.0)]
                .into_iter()
                .flatten()
                .fold(now.saturating_add(slice), u64::min);
            Ok(std::ops::ControlFlow::Continue(Some(until)))
        },
        |session, result| {
            for line in session.board.take_console() {
                println!("{line}");
            }
            for frame in session.board.take_frames() {
                outputs.frame(frame)?;
            }
            if result == &Ok(false) {
                return Err("board is quiescent without an input or device deadline".into());
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
                let mut error = error.clone();
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
            // A guest-time cadence goes quiet when the guest advances slowly; report on host time too.
            if std::time::Instant::now() >= next_wall_status {
                let state = session.board.state();
                eprintln!(
                    "FUJIMOD XP5 status: {}",
                    shared_boot_summary(&state, &session.board, policy)
                );
                next_wall_status = std::time::Instant::now() + WALL_STATUS_EVERY;
            }
            if status_every != Duration::ZERO && now >= next_status {
                let state = session.board.state();
                eprintln!(
                    "FUJIMOD XP5 status: {}",
                    shared_boot_summary(&state, &session.board, policy)
                );
                next_status =
                    now.saturating_add(crate::host::duration_ticks(status_every, ticks_per_second));
            }
            Ok(())
        },
    );
    // One exit path for the run loop: a stop, a refused patch, an interrupt and a normal end all
    // finish the board and the outputs, record the board state, then dump and trace, so a run
    // that dies still leaves them. Setup errors above (opening the board, discovery stubs, the
    // execution policy, watch hooks, key scheduling, the Ctrl-C handler) return before anything
    // has run and leave no board state; a panic would also bypass this path.
    let stop_reason = if interrupted.load(Ordering::Acquire) {
        crate::host::StopReason::Other
    } else {
        limits.stop_reason(&session).unwrap_or(crate::host::StopReason::Other)
    };
    eprintln!("FUJIMOD XP5 end: {}", stop_reason.name());
    let trace_write = pc_trace.zip(trace).map_or(Ok(()), |(output, trace)| output.write(trace));
    let snapshot_write = save_snapshot.as_ref().map_or(Ok(()), |path| crate::save_snapshot(&session, path));
    let board_finish = session.board.finish();
    let card_write = card_write_back.as_ref().map_or(Ok(()), |back| back.write(&session));
    let output_finish = screenshot.map_or_else(|| outputs.finish(), |path| outputs.screenshot(&path));
    let state = session.board.state();
    let state_write = serde_json::to_vec_pretty(&state)
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            std::fs::write(run_directory.join("board-state.json"), bytes)
                .map_err(|error| format!("board-state.json: {error}"))
        });
    post_mortem(&session.board, &run_directory, &dumps, &hashes);
    if let Some(loaded) = &fujimod {
        let report = loaded.report(&mod_loader::BoardMachine(&session.board));
        eprintln!("FUJIMOD XP5 mod status: {report}");
        if let Err(error) = std::fs::write(run_directory.join("mod-status.json"), report.to_string()) {
            eprintln!("FUJIMOD XP5 mod-status.json failed: {error}");
        }
    }
    let cleanup = [
        ("snapshot", snapshot_write),
        ("PC trace", trace_write),
        ("board", board_finish),
        ("outputs", output_finish),
        ("board state", state_write),
        ("card image", card_write),
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
        "engine": session.definition["engine"],
        "configuration": configuration_id,
        "firmware": firmware.id,
        "instructions": session.board.instruction_counts(),
        "now": session.board.now(),
        "stop_reason": stop_reason.name(),
        "discovery_callee_stubs": session.board.discovery_callee_stubs(),
        "run_directory": run_directory,
        "state": shared_boot_summary(&state, &session.board, policy),
    }))
}

/// Limit the memory allocated for any one physical dump.
const DUMP_LIMIT: u64 = 16 << 20;

fn parse_ranges(option: &str, spec: &str) -> Result<Vec<(u64, u64)>, String> {
    spec.split(';').filter(|item| !item.trim().is_empty()).map(|item| {
        let (address, length) = item.split_once(':')
            .ok_or_else(|| format!("{option} item {item:?} is not ADDR:LEN"))?;
        let address = parse_number(address.trim())?;
        let length = parse_number(length.trim())?;
        if length == 0 || address.checked_add(length).is_none() {
            return Err(format!("{option} range {item:?} is empty or overflows"));
        }
        Ok((address, length))
    }).collect()
}

fn parse_dumps(spec: &str) -> Result<Vec<(u64, usize)>, String> {
    let mut dumps = Vec::new();
    for (address, length) in parse_ranges("--dump", spec)? {
        if length > DUMP_LIMIT {
            return Err(format!("--dump length {length:#x} exceeds {DUMP_LIMIT:#x}"));
        }
        if dumps.iter().any(|(known, _)| *known == address) {
            return Err(format!("--dump address {address:#x} repeats"));
        }
        dumps.push((address, length as usize));
    }
    Ok(dumps)
}

fn print_hashes(board: &fujimod_emulator::board::Board, run_directory: &Path, ranges: &[(u64, u64)]) {
    if ranges.is_empty() {
        return;
    }
    let mut lines = String::new();
    for &(address, length) in ranges {
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
    hashes: &[(u64, u64)],
) {
    print_hashes(board, run_directory, hashes);
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
