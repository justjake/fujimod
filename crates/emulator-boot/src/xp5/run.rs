//! The XP5 bring-up run: open a session from explicit inputs, drive it in slices and record
//! the outcome in the run directory.
use super::{RunFacts, Startup};
use crate::host::{parse_gpio, parse_number, parse_stub, parse_timed_poke};
use crate::mod_loader;
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator::board::Board;
use fujimod_emulator_cpu::ExecutionPolicy;
use fujimod_emulator_devices::graphics::scanout::Frame;
use fujimod_emulator_session::Session;
use fujimod_emulator_xp5_board::keys::Key;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Write one little-endian word for --poke: a physical address first, else CPU0's virtual view.
fn poke_word(
    board: &fujimod_emulator::board::Board,
    address: u64,
    value: u32,
) -> Result<(), String> {
    let bytes = value.to_le_bytes();
    board
        .write_physical(address, &bytes)
        .or_else(|_| board.write_memory(0, address, &bytes))
        .map_err(|error| format!("poke {address:#x}: {error}"))
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
    /// Physical ranges `ADDR:LEN[,ADDR:LEN...]` whose writes are logged to stderr.
    pub watch_writes: Option<String>,
    pub pc_trace: Option<crate::TraceOutput>,
    pub screenshot: Option<PathBuf>,
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
    /// The loader start's card in SD slot 2: a raw image (changed sectors are written back at
    /// the end of the run) or files for an in-memory volume.
    pub second_card_image: Option<PathBuf>,
    pub second_card_files: Vec<(String, PathBuf)>,
    /// Scripted key presses: `t=SECONDS:KEY;...` (a leading `-` on KEY releases it).
    pub keys: Option<String>,
    pub gpio: Option<String>,
    pub pokes: Vec<String>,
    pub press: Option<String>,
    /// Timed dial and switch positions: `CONTROL=POSITION[@MS][,...]`.
    pub settings: Option<String>,
    /// Named camera settings of the user profile the loader start boots with.
    pub camera_settings: std::collections::BTreeMap<String, String>,
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

impl Run {
    /// A boot of `update` with no caller inputs, recording into `output`.
    pub fn new(
        configuration: ResolvedConfiguration,
        update: PathBuf,
        execution_policy: ExecutionPolicy,
        output: PathBuf,
    ) -> Self {
        Self {
            configuration,
            load_snapshot: None,
            save_snapshot: None,
            update,
            manifest: None,
            watch_pcs: None,
            watch_writes: None,
            pc_trace: None,
            screenshot: None,
            dump: None,
            hash: None,
            boot_rom: None,
            card_image: None,
            card_files: Vec::new(),
            second_card_image: None,
            second_card_files: Vec::new(),
            keys: None,
            gpio: None,
            pokes: Vec::new(),
            press: None,
            settings: None,
            camera_settings: std::collections::BTreeMap::new(),
            discovery: false,
            stubs: Vec::new(),
            patch_file: None,
            mods: Vec::new(),
            scene: None,
            quantum: None,
            seconds: None,
            instruction_limit: None,
            wall_seconds: None,
            status_every: Duration::from_secs(30),
            execution_policy,
            output,
        }
    }
}

/// Host-time cadence of status lines, so a slow run is visibly alive.
const WALL_STATUS_EVERY: Duration = Duration::from_secs(10);

/// The stopped state of one prepared XP5 run, driven by a native frontend.
pub struct RunHost {
    configuration: ResolvedConfiguration,
    configuration_id: String,
    firmware: String,
    policy: ExecutionPolicy,
    session: Session,
    inputs: fujimod_emulator_xp5_board::session::Inputs,
    outputs: crate::output::Outputs,
    ticks_per_second: u64,
    quantum: usize,
    run_directory: PathBuf,
    start: u64,
    next_status: u64,
    status_every: Duration,
    next_wall_status: Instant,
    slice: u64,
    patches: crate::patch::Schedule,
    fujimod: Option<mod_loader::Loaded>,
    interrupted: Arc<AtomicBool>,
    limits: crate::host::Limits,
    save_snapshot: Option<PathBuf>,
    screenshot: Option<PathBuf>,
    pc_trace: Option<(
        crate::TraceOutput,
        fujimod_emulator::board::observation::PcTrace,
    )>,
    dumps: Vec<(u64, usize)>,
    hashes: Vec<(u64, u64)>,
    card_write_back: Vec<super::loader_card::WriteBack>,
    timed_gpio: VecDeque<(u64, u64, (u8, u8, bool))>,
    timed_pokes: VecDeque<(u64, u64, (u64, u32))>,
    watch_pcs: Vec<u64>,
    watch_writes: Vec<(u64, u64)>,
    watch_after: Option<(u64, u64)>,
    frames: Vec<Frame>,
    latest_frame: Option<Frame>,
    phase: super::phase::PhaseRecorder,
}

impl RunHost {
    /// Open and prepare every input of an XP5 run without executing the guest.
    pub fn open(run: Run) -> Result<Self, String> {
        let wall_start = Instant::now();
        let Run {
            configuration,
            load_snapshot,
            save_snapshot,
            update,
            manifest,
            watch_pcs,
            watch_writes,
            pc_trace,
            screenshot,
            dump,
            hash,
            boot_rom: rom_path,
            card_image: sd_image,
            card_files,
            second_card_image,
            second_card_files,
            keys: key_script,
            gpio: gpio_spec,
            pokes,
            press,
            settings,
            camera_settings,
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
        let mut watch_pcs = watch_pcs
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .filter(|item| !item.trim().is_empty())
            .map(|item| parse_number(item.trim()))
            .collect::<Result<Vec<_>, _>>()?;
        watch_pcs.sort_unstable();
        watch_pcs.dedup();
        let watch_writes = parse_ranges(
            "--watch-writes",
            &watch_writes.unwrap_or_default().replace(',', ";"),
        )?;
        let dumps = parse_dumps(dump.as_deref().unwrap_or_default())?;
        let hashes = parse_ranges("--hash", hash.as_deref().unwrap_or_default())?;
        let patches = crate::patch::Schedule::from_path(
            patch_file.as_deref(),
            &[0..0x8000_0000, 0xfa20_0000..0xfa30_0000],
        )?;
        let keys = crate::host::parse_keys(key_script.as_deref(), press.as_deref())?
            .into_iter()
            .map(|edge| Ok((edge.delay, Key::parse(&edge.name)?, edge.pressed)))
            .collect::<Result<Vec<_>, String>>()?;
        let settings = crate::host::parse_settings(settings.as_deref().unwrap_or_default())?;
        let mut gpio = Vec::new();
        let mut timed_gpio = Vec::new();
        for item in gpio_spec
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .filter(|item| !item.trim().is_empty())
        {
            match item.trim().split_once('@') {
                Some((level, at)) => {
                    let at_ms: u64 = at.trim().parse().map_err(|_| {
                        format!("--gpio {item}: the time after @ is whole milliseconds")
                    })?;
                    timed_gpio.push((at_ms, parse_gpio(level)?));
                }
                None => gpio.push(parse_gpio(item.trim())?),
            }
        }
        timed_gpio.sort_by_key(|(at_ms, _)| *at_ms);
        let watch_after_ms = match std::env::var("XT5_WATCH_AFTER_MS") {
            Ok(value) => Some(
                value
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| "XT5_WATCH_AFTER_MS needs whole milliseconds".to_string())?,
            )
            .filter(|ms| *ms > 0),
            Err(_) => None,
        };
        let mut timed_pokes = Vec::new();
        let mut untimed_pokes = Vec::new();
        for spec in &pokes {
            match parse_timed_poke(spec)? {
                (Some(at_ms), address, value) => timed_pokes.push((at_ms, (address, value))),
                (None, address, value) => untimed_pokes.push((address, value)),
            }
        }
        timed_pokes.sort_by_key(|(at_ms, _)| *at_ms);
        let request = Startup {
            update,
            manifest,
            boot_rom: rom_path,
            card_image: sd_image,
            card_files,
            second_card_image,
            second_card_files,
            scene,
            output: Some(run_directory.clone()),
            mode,
            quantum: quantum_override,
            camera_settings,
        };
        let crate::Camera {
            mut session,
            ticks_per_second,
            quantum,
            xp5: Some(facts),
            ..
        } = crate::open(configuration.clone(), crate::StartupInputs::Xp5(request))?
        else {
            return Err("XP5 startup returned another processor generation".into());
        };
        let RunFacts {
            firmware,
            state,
            inputs,
            outputs,
            card_write_back,
        } = facts;
        let inputs = inputs.ok_or("XP5 camera has no physical input handle")?;
        if let Some(path) = load_snapshot {
            // Restoring runs the processors to the snapshot boundary; an HVF
            // processor starts asynchronous runs only under the interactive policy.
            session.board.set_execution_policy(policy)?;
            crate::load_snapshot(&mut session, &path)?;
        }
        session
            .board
            .install_discovery_callee_stubs(&callee_stubs)?;
        session.board.set_execution_policy(policy)?;
        if watch_after_ms.is_none() {
            session.board.watch_pcs(&watch_pcs)?;
            session.board.watch_writes(&watch_writes)?;
        }
        let trace = pc_trace
            .as_ref()
            .map(|output| output.begin(&session))
            .transpose()?;
        let fujimod = if mod_specs.is_empty() {
            None
        } else {
            let target = mod_loader::Target {
                id: &firmware.id,
                sha256: &firmware.source_sha256,
                model: fujimod_emulator_cpu::Model::ArmV8A,
            };
            let mut loaded = mod_loader::prepare(target, &mod_specs, |address, word| {
                firmware.expect_word(address, word, "mod patch site")
            })?;
            let mut excluded = firmware
                .regions
                .iter()
                .map(|region| (region.address, region.bytes.len() as u64))
                .collect::<Vec<_>>();
            for preload in &state.preload {
                excluded.push((preload.address, super::manifest::preload_length(preload)?));
            }
            let stubbed: Vec<u64> = callee_stubs.iter().map(|stub| stub.address).collect();
            loaded.place(
                &mod_loader::BoardMachine(&session.board),
                &excluded,
                &stubbed,
            )?;
            Some(loaded)
        };
        for (port, pin, high) in gpio {
            inputs.gpio(&mut session, usize::from(port), usize::from(pin), high)?;
        }
        for (address, value) in untimed_pokes {
            poke_word(&session.board, address, value)?;
        }
        let start = session.board.now();
        let mut phase = super::phase::PhaseRecorder::open(
            &run_directory,
            wall_start,
            start,
            ticks_per_second,
            &configuration_id,
            &firmware.id,
            policy.name(),
            session.definition["engine"].clone(),
        )?;
        phase.record("guest_start", &session.board, None)?;
        for (delay, key, pressed) in keys {
            let delay_ticks = crate::host::duration_ticks(delay, ticks_per_second);
            inputs.key(
                &mut session,
                start.saturating_add(delay_ticks),
                key,
                pressed,
            )?;
        }
        let millisecond_tick = |ms: u64| {
            start.saturating_add(crate::host::duration_ticks(
                std::time::Duration::from_millis(ms),
                ticks_per_second,
            ))
        };
        for change in settings {
            inputs.set_position(
                &mut session,
                millisecond_tick(change.at_ms),
                &change.control,
                &change.position,
            )?;
            eprintln!(
                "FUJIMOD XP5 set: {}={} at {} ms",
                change.control, change.position, change.at_ms
            );
        }
        let timed_gpio = timed_gpio
            .into_iter()
            .map(|(at_ms, level)| (millisecond_tick(at_ms), at_ms, level))
            .collect();
        let timed_pokes = timed_pokes
            .into_iter()
            .map(|(at_ms, write)| (millisecond_tick(at_ms), at_ms, write))
            .collect();
        let watch_after = watch_after_ms.map(|ms| (millisecond_tick(ms), ms));
        let interrupted = Arc::new(AtomicBool::new(false));
        let signal = interrupted.clone();
        ctrlc::set_handler(move || signal.store(true, Ordering::Release))
            .map_err(|error| error.to_string())?;
        let limits =
            crate::host::Limits::new(&session, instruction_limit, limit, ticks_per_second)?
                .with_wall(wall_seconds);
        eprintln!(
            "FUJIMOD XP5 session configuration={configuration_id} firmware={} policy={} quantum={quantum} run={}",
            firmware.id,
            policy.name(),
            run_directory.display()
        );
        Ok(Self {
            configuration,
            configuration_id,
            firmware: firmware.id,
            policy,
            session,
            inputs,
            outputs,
            ticks_per_second,
            quantum,
            run_directory: run_directory.clone(),
            start,
            next_status: start
                .saturating_add(crate::host::duration_ticks(status_every, ticks_per_second)),
            status_every,
            next_wall_status: Instant::now() + WALL_STATUS_EVERY,
            slice: (ticks_per_second / 1_000).clamp(1, 100_000_000),
            patches,
            fujimod,
            interrupted,
            limits,
            save_snapshot,
            screenshot,
            pc_trace: pc_trace.zip(trace),
            dumps,
            hashes,
            card_write_back,
            timed_gpio,
            timed_pokes,
            watch_pcs,
            watch_writes,
            watch_after,
            frames: Vec::new(),
            latest_frame: None,
            phase,
        })
    }

    /// Advance one scheduler window, then publish the run's output and
    /// observations. Returns false once the run has ended: a budget is
    /// spent, the run was interrupted, or the board went quiescent.
    pub fn step(&mut self) -> Result<bool, String> {
        let Self {
            session,
            inputs,
            outputs,
            patches,
            fujimod,
            interrupted,
            limits,
            start,
            ticks_per_second,
            quantum,
            slice,
            next_status,
            status_every,
            next_wall_status,
            frames,
            latest_frame,
            timed_gpio,
            timed_pokes,
            watch_pcs,
            watch_writes,
            watch_after,
            policy,
            phase,
            ..
        } = self;
        if limits.reached(session) || interrupted.load(Ordering::Acquire) {
            return Ok(false);
        }
        let now = session.board.now();
        let next_patch = patches.advance(&session.board, now, *start, *ticks_per_second)?;
        if let Some(loaded) = fujimod.as_mut() {
            loaded.attach_if_ready(&mod_loader::BoardMachine(&session.board), now)?;
        }
        while let Some(&(at, at_ms, (port, pin, high))) = timed_gpio.front() {
            if at > now {
                break;
            }
            timed_gpio.pop_front();
            inputs.gpio(session, usize::from(port), usize::from(pin), high)?;
            eprintln!(
                "FUJIMOD XP5 gpio: {port}:{pin}={} at {at_ms} ms (tick {now})",
                u8::from(high)
            );
        }
        while let Some(&(at, at_ms, (address, value))) = timed_pokes.front() {
            if at > now {
                break;
            }
            timed_pokes.pop_front();
            poke_word(&session.board, address, value)?;
            eprintln!("FUJIMOD XP5 poke: {address:#x}={value:#x} at {at_ms} ms (tick {now})");
        }
        if let Some((at, after_ms)) = *watch_after
            && at <= now
        {
            *watch_after = None;
            session.board.watch_pcs(watch_pcs)?;
            session.board.watch_writes(watch_writes)?;
            eprintln!("FUJIMOD XP5 watch-pcs armed at {after_ms} ms (tick {now})");
        }
        // Stop the slice at the next scheduled host action, so it applies at its guest time.
        let until = [
            next_patch,
            timed_gpio.front().map(|item| item.0),
            timed_pokes.front().map(|item| item.0),
            watch_after.map(|item| item.0),
        ]
        .into_iter()
        .flatten()
        .fold(now.saturating_add(*slice), u64::min);
        let result = limits.step(session, *quantum, None, Some(until));
        for line in session.board.take_console() {
            println!("{line}");
        }
        print_write_notes(&session.board, *ticks_per_second);
        for frame in session.board.take_frames() {
            let bank = frame.output;
            let modeled_sequence = frame.sequence;
            *latest_frame = Some(frame.clone());
            frames.push(frame.clone());
            let archive_sequence = outputs.frame(frame)?;
            phase.record(
                "frame",
                &session.board,
                Some((bank, archive_sequence, modeled_sequence)),
            )?;
        }
        let advanced = match result {
            Ok(advanced) => advanced,
            Err(error) => return Err(stop_error(&session.board, *policy, error)),
        };
        if !advanced {
            return Err("board is quiescent without an input or device deadline".into());
        }
        session.board.check_discovery_callee_stubs()?;
        let now = session.board.now();
        // A guest-time cadence goes quiet when the guest advances slowly; report on host time too.
        if Instant::now() >= *next_wall_status {
            phase.record("status", &session.board, None)?;
            let state = session.board.state();
            eprintln!(
                "FUJIMOD XP5 status: {}",
                shared_boot_summary(&state, &session.board, *policy)
            );
            *next_wall_status = Instant::now() + WALL_STATUS_EVERY;
        }
        if *status_every != Duration::ZERO && now >= *next_status {
            phase.record("status", &session.board, None)?;
            let state = session.board.state();
            eprintln!(
                "FUJIMOD XP5 status: {}",
                shared_boot_summary(&state, &session.board, *policy)
            );
            *next_status = now.saturating_add(crate::host::duration_ticks(
                *status_every,
                *ticks_per_second,
            ));
        }
        Ok(true)
    }

    pub fn board(&self) -> &Board {
        &self.session.board
    }

    pub fn board_mut(&mut self) -> &mut Board {
        &mut self.session.board
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    pub fn inputs(&self) -> &fujimod_emulator_xp5_board::session::Inputs {
        &self.inputs
    }

    /// The session and its physical input handle, borrowed together for one input change.
    pub fn parts_mut(&mut self) -> (&mut Session, &fujimod_emulator_xp5_board::session::Inputs) {
        (&mut self.session, &self.inputs)
    }

    pub fn take_frames(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.frames)
    }

    /// The last LCD image shown, for a frontend that joins a running session.
    pub fn latest_frame(&self) -> Option<Frame> {
        self.latest_frame.clone()
    }

    pub fn now(&self) -> u64 {
        self.session.board.now()
    }

    pub fn configuration_id(&self) -> &str {
        &self.configuration_id
    }

    pub fn body_id(&self) -> &str {
        &self.configuration.configuration.body_id
    }

    pub fn configuration(&self) -> &ResolvedConfiguration {
        &self.configuration
    }

    pub fn firmware_id(&self) -> &str {
        &self.firmware
    }

    pub fn quantum(&self) -> usize {
        self.quantum
    }

    pub fn ticks_per_second(&self) -> u64 {
        self.ticks_per_second
    }

    /// Finish the board and the outputs, record the board state, then dump
    /// and trace. Every end of a run comes here, so a run that stopped with
    /// `outcome` still leaves its records.
    pub fn finish(self, outcome: Result<(), String>) -> Result<Value, String> {
        let Self {
            configuration_id,
            firmware,
            policy,
            mut session,
            outputs,
            run_directory,
            save_snapshot,
            screenshot,
            pc_trace,
            dumps,
            hashes,
            card_write_back,
            fujimod,
            interrupted,
            limits,
            ticks_per_second,
            mut phase,
            ..
        } = self;
        let stop_reason = if interrupted.load(Ordering::Acquire) {
            crate::host::StopReason::Other
        } else {
            limits
                .stop_reason(&session)
                .unwrap_or(crate::host::StopReason::Other)
        };
        eprintln!("FUJIMOD XP5 end: {}", stop_reason.name());
        let phase_write = phase.record_end(
            &session.board,
            outcome.is_ok(),
            stop_reason.name(),
            outcome.as_ref().err().map(String::as_str),
        );
        print_write_notes(&session.board, ticks_per_second);
        let trace_write = pc_trace.map_or(Ok(()), |(output, trace)| output.write(trace));
        let snapshot_write = save_snapshot
            .as_ref()
            .map_or(Ok(()), |path| crate::save_snapshot(&session, path));
        let board_finish = session.board.finish();
        let card_write = card_write_back
            .iter()
            .try_for_each(|back| back.write(&session));
        let output_finish =
            screenshot.map_or_else(|| outputs.finish(), |path| outputs.screenshot(&path));
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
            if let Err(error) =
                std::fs::write(run_directory.join("mod-status.json"), report.to_string())
            {
                eprintln!("FUJIMOD XP5 mod-status.json failed: {error}");
            }
        }
        let cleanup = [
            ("phase timing", phase_write),
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
            "firmware": firmware,
            "instructions": session.board.instruction_counts(),
            "now": session.board.now(),
            "stop_reason": stop_reason.name(),
            "discovery_callee_stubs": session.board.discovery_callee_stubs(),
            "run_directory": run_directory,
            "state": shared_boot_summary(&state, &session.board, policy),
        }))
    }
}

/// The board's refusal, with where every core stood, for run records.
fn stop_error(board: &Board, policy: ExecutionPolicy, error: String) -> String {
    let state = board.state();
    eprintln!(
        "FUJIMOD XP5 stop: {}",
        shared_boot_summary(&state, board, policy)
    );
    let stop_site = state["bus"]["frontier"]
        .as_array()
        .and_then(|frontier| frontier.last());
    let stop_core = stop_site
        .and_then(|site| site["cpu"].as_u64())
        .and_then(|core| usize::try_from(core).ok());
    let mut error = error;
    if let Some(site) = stop_site
        && let (Some(tick), Some(pc)) = (site["tick"].as_u64(), site["pc"].as_u64())
    {
        error = format!("{error}; tick={tick} PC={pc:#x}");
    }
    if let Some(core) = stop_core {
        let lr_register = if core == 4 {
            fujimod_emulator_cpu::Register::Arm32(fujimod_emulator_cpu::arm::Register::R(14))
        } else {
            fujimod_emulator_cpu::Register::X(30)
        };
        if let Ok(lr) = board.read_register(core, lr_register) {
            error = format!("{error}; CPU{core} LR={lr:#x}");
        }
    }
    error
}

/// Run the boot described by `run` until a budget, an interrupt or a stop.
pub fn run_boot(run: Run) -> Result<Value, String> {
    let mut host = RunHost::open(run)?;
    let outcome = loop {
        match host.step() {
            Ok(true) => drop(host.take_frames()),
            Ok(false) => break Ok(()),
            Err(error) => break Err(error),
        }
    };
    host.finish(outcome)
}

/// Limit the memory allocated for any one physical dump.
const DUMP_LIMIT: u64 = 16 << 20;

fn parse_ranges(option: &str, spec: &str) -> Result<Vec<(u64, u64)>, String> {
    spec.split(';')
        .filter(|item| !item.trim().is_empty())
        .map(|item| {
            let (address, length) = item
                .split_once(':')
                .ok_or_else(|| format!("{option} item {item:?} is not ADDR:LEN"))?;
            let address = parse_number(address.trim())?;
            let length = parse_number(length.trim())?;
            if length == 0 || address.checked_add(length).is_none() {
                return Err(format!("{option} range {item:?} is empty or overflows"));
            }
            Ok((address, length))
        })
        .collect()
}

/// Print the `--watch-writes` notes recorded since the last call.
fn print_write_notes(board: &fujimod_emulator::board::Board, ticks_per_second: u64) {
    let (notes, dropped) = board.take_write_notes();
    for note in notes {
        eprintln!("FUJIMOD XP5 write: {}", note.line(ticks_per_second));
    }
    if dropped > 0 {
        eprintln!(
            "FUJIMOD XP5 write: {dropped} more watched writes were not recorded (note bound)"
        );
    }
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

fn print_hashes(
    board: &fujimod_emulator::board::Board,
    run_directory: &Path,
    ranges: &[(u64, u64)],
) {
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
            Ok(()) => eprintln!(
                "FUJIMOD XP5 dump: {address:#x}+{length:#x} -> {}",
                path.display()
            ),
            Err(error) => eprintln!("FUJIMOD XP5 dump {address:#x}+{length:#x} failed: {error}"),
        }
    }
    for entry in board.pc_entries() {
        eprintln!("FUJIMOD XP5 pc-entry: {entry}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_control_key_names_parse() {
        for (name, key) in [
            ("AEL", Key::AeLock),
            ("AFON", Key::AfOn),
            ("STICK_UL", Key::StickUpLeft),
            ("STICK_UR", Key::StickUpRight),
            ("STICK_DL", Key::StickDownLeft),
            ("STICK_DR", Key::StickDownRight),
            ("REAR_PUSH", Key::RearPush),
            ("FRONT_PUSH", Key::FrontPush),
            ("REAR+", Key::RearPlus),
            ("FRONT-", Key::FrontMinus),
        ] {
            assert_eq!(Key::parse(name), Ok(key), "{name}");
        }
        let edges = crate::host::parse_keys(None, Some("100:REAR+,200:AEL:300")).unwrap();
        let names: Vec<_> = edges
            .iter()
            .map(|edge| (edge.delay.as_millis(), edge.name.as_str(), edge.pressed))
            .collect();
        assert_eq!(
            names,
            [
                (100, "REAR+", true),
                (200, "AEL", true),
                (250, "REAR+", false),
                (500, "AEL", false)
            ]
        );
    }
}
