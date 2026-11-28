//! Native boot command: a camera description and a list of timed inputs, for every processor
//! generation.
use boot::host::Memory;
use fujimod_camera::{self as camera, CameraDescription, Library, StateDir};
use fujimod_catalog_configuration::RunCpuBackend;
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_boot::{self as boot, Options, StartupPath, media::CardInsert, xp5};
use fujimod_emulator_camera::{self as emulator, CardContents, CardFile};
use fujimod_emulator_cpu::ExecutionPolicy;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{self, Read as _, Write},
    path::PathBuf,
    process::ExitCode,
    time::{Duration, Instant},
};

const HELP: &str = "Usage: fujimod-emulator boot --camera CAMERA [--actions ACTIONS] [--seconds N] [--wall-seconds N]\n\
  [--output DIR] [--screenshot PNG] [--gui] [run flags]\n\
  fujimod-emulator open --camera CAMERA [--gui]\n\n\
CAMERA is the camera description the MCP server's camera_open and Python's Board.open_camera take\n\
(catalog/schemas/emulator-camera.generated.schema.json): body (required, a catalog body such as\n\
xh2s), firmware (a version such as 7.30, a firmware id, or none; the body's newest when omitted),\n\
settings ({\"language\": \"en\", \"setup\": \"done\"}), sd_card, sd_card_2, cfexpress_card ({present,\n\
write_protected, files | directory | image}), date, scene, engine, execution_policy, firmware_files\n\
({update, boot_rom}), application_plan, face_result_profile and overlays. The body and firmware\n\
select the firmware's run configuration; its processor decides the startup path. The firmware's\n\
files come from its source record under FUJIMOD_FIRMWARE_LIBRARY (or the current directory) unless\n\
firmware_files names them.\n\
ACTIONS is a list of timed inputs (catalog/schemas/emulator-actions.generated.schema.json), each\n\
{\"at_ms\": MS} and one of press ({control, hold_ms}), hold ({control}), release ({control}),\n\
set ({control, position}), gpio ({port, pin, high}), poke ({address, value}),\n\
power_input ({register, value}) or hdmi ({connected: true}).\n\
Each of CAMERA and ACTIONS is a file path, - for standard input (one of them at most), or inline\n\
JSON (an argument starting with { or [). Relative paths inside resolve from that file's directory,\n\
or the current directory for inline or standard-input JSON.\n\n\
Run flags: --seconds N (guest time), --wall-seconds N (host time), --instruction-limit COUNT,\n\
--status-every N (guest time), --output DIR, --screenshot PNG, --trace FILE [--trace-limit N],\n\
--load-snapshot FILE, --save-snapshot FILE, --save-nor FILE (XP4), --quantum N, --mod MANIFEST...,\n\
--patch FILE, --stub NAME@ADDR=VALUE..., --discovery, --cpu-backend unicorn|dynarmic, and for XP5\n\
--manifest FILE, --watch-pcs ADDR[,ADDR...], --watch-writes ADDR:LEN[,...], --dump ADDR:LEN[;...],\n\
--hash ADDR:LEN[;...].\n\
--gui opens the desktop window connected to the session.\n\
boot prints a JSON report; it exits 0 at REC (EXR II) or at the end of the run, 2 at a stopped\n\
frontier.\n\n\
open adds FujiMod to the camera: its boot is stock, sd-card (FujiMod from sd_card, or the\n\
current release card downloaded into FUJIMOD_STATE_DIR when it names no files) or autostart (the\n\
card's loader installed into internal flash once and kept in the state directory). It restores a\n\
saved boot for the same camera when the state directory holds one and saves each new boot unless\n\
boot_cache is false, boots to REC and prints a JSON report of the body, firmware, boot, stage and\n\
engine. --gui opens the desktop window on the opened session.\n";

/// Where a JSON argument comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Source {
    File(PathBuf),
    Stdin,
    Inline(String),
}

impl Source {
    fn parse(argument: OsString, flag: &str) -> Result<Self, String> {
        let text = argument
            .into_string()
            .map_err(|_| format!("{flag} must be UTF-8"))?;
        let trimmed = text.trim_start();
        Ok(if text == "-" {
            Self::Stdin
        } else if trimmed.starts_with('{') || trimmed.starts_with('[') {
            Self::Inline(text)
        } else {
            Self::File(PathBuf::from(text))
        })
    }

    /// The JSON text and the directory its relative paths resolve from.
    fn read(&self, flag: &str) -> Result<(String, PathBuf), String> {
        let current = || std::env::current_dir().map_err(|error| error.to_string());
        match self {
            Self::Inline(text) => Ok((text.clone(), current()?)),
            Self::Stdin => {
                let mut text = String::new();
                io::stdin()
                    .read_to_string(&mut text)
                    .map_err(|error| format!("{flag}: standard input: {error}"))?;
                Ok((text, current()?))
            }
            Self::File(path) => {
                let text = std::fs::read_to_string(path)
                    .map_err(|error| format!("{flag} {}: {error}", path.display()))?;
                let directory = path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .map_or_else(current, |parent| Ok(parent.to_owned()))?;
                Ok((text, directory))
            }
        }
    }

    /// Parse the JSON this source holds.
    fn parse_json<T: serde::de::DeserializeOwned>(
        &self,
        flag: &str,
    ) -> Result<(T, PathBuf), String> {
        let (text, base) = self.read(flag)?;
        let value = serde_json::from_str(&text).map_err(|error| format!("{flag}: {error}"))?;
        Ok((value, base))
    }
}

/// An instruction-entry trace a run records.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TraceFlags {
    path: String,
    limit: Option<usize>,
}

/// The host-local flags of one run: its limits, outputs and debugging aids.
#[derive(Clone, Debug, Default, PartialEq)]
struct RunFlags {
    seconds: Option<f64>,
    wall_seconds: Option<f64>,
    instruction_limit: Option<u64>,
    status_every: Option<f64>,
    output: Option<String>,
    screenshot: Option<String>,
    trace: Option<TraceFlags>,
    load_snapshot: Option<String>,
    save_snapshot: Option<String>,
    save_nor: Option<String>,
    quantum: Option<usize>,
    mods: Vec<String>,
    patch: Option<String>,
    stubs: Vec<String>,
    discovery: bool,
    manifest: Option<String>,
    watch_pcs: Option<String>,
    watch_writes: Option<String>,
    dump: Option<String>,
    hash: Option<String>,
    cpu_backend: Option<String>,
}

/// The arguments of one command.
#[derive(Debug)]
struct Arguments {
    camera: Source,
    actions: Option<Source>,
    gui: bool,
    run: RunFlags,
}

enum Command {
    Boot(Arguments),
    Open(Arguments),
}

/// Flags that take a value; `--gui` and `--discovery` are switches.
const VALUED: [&str; 23] = [
    "--camera",
    "--actions",
    "--seconds",
    "--wall-seconds",
    "--instruction-limit",
    "--status-every",
    "--output",
    "--screenshot",
    "--trace",
    "--trace-limit",
    "--load-snapshot",
    "--save-snapshot",
    "--save-nor",
    "--quantum",
    "--mod",
    "--patch",
    "--stub",
    "--manifest",
    "--watch-pcs",
    "--watch-writes",
    "--dump",
    "--hash",
    "--cpu-backend",
];

fn parse_command(args: impl Iterator<Item = OsString>) -> Result<Option<Command>, String> {
    let mut args = args.peekable();
    let command = match args.next().as_ref().and_then(|argument| argument.to_str()) {
        Some("--help" | "-h") => return Ok(None),
        Some("boot") => "boot",
        Some("open") => "open",
        _ => return Err(HELP.into()),
    };
    if args.peek().is_some_and(|a| a == "--help" || a == "-h") {
        return Ok(None);
    }
    let mut camera = None;
    let mut actions = None;
    let mut gui = false;
    let mut run = RunFlags::default();
    let mut texts: BTreeMap<String, String> = BTreeMap::new();
    while let Some(argument) = args.next() {
        let key = argument
            .to_str()
            .ok_or("option name is not UTF-8")?
            .to_owned();
        if key == "--gui" || key == "--discovery" {
            if command == "open" && key == "--discovery" {
                return Err("--discovery applies to boot, not open".into());
            }
            let flag = if key == "--gui" {
                &mut gui
            } else {
                &mut run.discovery
            };
            if std::mem::replace(flag, true) {
                return Err(format!("duplicate option: {key}"));
            }
            continue;
        }
        if !VALUED.contains(&key.as_str()) {
            return Err(format!(
                "unknown option: {key}; the camera description and the action list carry the camera's inputs"
            ));
        }
        if command == "open" && key != "--camera" {
            return Err(format!("{key} applies to boot, not open"));
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {key}"))?;
        match key.as_str() {
            "--camera" | "--actions" => {
                let source = Source::parse(value, &key)?;
                let slot = if key == "--camera" {
                    &mut camera
                } else {
                    &mut actions
                };
                if slot.replace(source).is_some() {
                    return Err(format!("duplicate option: {key}"));
                }
            }
            "--mod" | "--stub" => {
                let text = value
                    .into_string()
                    .map_err(|_| format!("{key} must be UTF-8"))?;
                if key == "--mod" {
                    run.mods.push(text);
                } else {
                    run.stubs.push(text);
                }
            }
            _ => {
                let text = value
                    .into_string()
                    .map_err(|_| format!("{key} must be UTF-8"))?;
                if texts.insert(key.clone(), text).is_some() {
                    return Err(format!("duplicate option: {key}"));
                }
            }
        }
    }
    let camera = camera.ok_or_else(|| format!("{command} needs --camera\n\n{HELP}"))?;
    if camera == Source::Stdin && actions == Some(Source::Stdin) {
        return Err("only one of --camera and --actions may read standard input".into());
    }
    let mut take = |key: &str| texts.remove(key);
    let seconds = |key: &str, text: Option<String>| -> Result<Option<f64>, String> {
        text.map(|text| {
            text.parse::<f64>()
                .ok()
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
                .ok_or_else(|| format!("{key} takes a nonnegative number of seconds"))
        })
        .transpose()
    };
    let number = |key: &str, text: Option<String>| -> Result<Option<u64>, String> {
        text.map(|text| {
            text.parse::<u64>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| format!("{key} takes a positive integer"))
        })
        .transpose()
    };
    let size = |key: &str, value: Option<u64>| -> Result<Option<usize>, String> {
        value
            .map(|value| usize::try_from(value).map_err(|_| format!("{key} is too large")))
            .transpose()
    };
    run.seconds = seconds("--seconds", take("--seconds"))?;
    run.wall_seconds = seconds("--wall-seconds", take("--wall-seconds"))?;
    run.status_every = seconds("--status-every", take("--status-every"))?;
    run.instruction_limit = number("--instruction-limit", take("--instruction-limit"))?;
    run.quantum = size("--quantum", number("--quantum", take("--quantum"))?)?;
    let trace_limit = size(
        "--trace-limit",
        number("--trace-limit", take("--trace-limit"))?,
    )?;
    run.trace = match (take("--trace"), trace_limit) {
        (Some(path), limit) => Some(TraceFlags { path, limit }),
        (None, Some(_)) => return Err("--trace-limit requires --trace".into()),
        (None, None) => None,
    };
    run.output = take("--output");
    run.screenshot = take("--screenshot");
    run.load_snapshot = take("--load-snapshot");
    run.save_snapshot = take("--save-snapshot");
    run.save_nor = take("--save-nor");
    run.patch = take("--patch");
    run.manifest = take("--manifest");
    run.watch_pcs = take("--watch-pcs");
    run.watch_writes = take("--watch-writes");
    run.dump = take("--dump");
    run.hash = take("--hash");
    run.cpu_backend = take("--cpu-backend");
    let arguments = Arguments {
        camera,
        actions,
        gui,
        run,
    };
    Ok(Some(if command == "boot" {
        Command::Boot(arguments)
    } else {
        Command::Open(arguments)
    }))
}

/// The firmware's files a boot runs, read once.
struct Files {
    update: boot::FirmwareFile,
    boot_rom: Option<boot::FirmwareFile>,
}

/// Everything one boot request resolved to, before the processor's runner takes it.
struct Boot {
    resolved: ResolvedConfiguration,
    files: Files,
    camera: CameraDescription,
    cards: emulator::Cards,
    settings: BTreeMap<String, String>,
    clock_seconds_2000: Option<u32>,
    scene: Option<PathBuf>,
    application_plan: Option<PathBuf>,
    library: Library,
    base: PathBuf,
    actions: Actions,
    run: RunFlags,
    gui: bool,
}

/// A request's timed inputs, in the forms the runners schedule.
#[derive(Default)]
struct Actions {
    /// `t=SECONDS:KEY;-KEY` hold and release edges.
    keys: Vec<String>,
    /// `TIME_MS:KEY:HOLD_MS` presses.
    taps: Vec<String>,
    /// `CONTROL=POSITION@MS`.
    positions: Vec<String>,
    /// `(at_ms, port, pin, high)`.
    gpio: Vec<(u64, u8, u8, bool)>,
    /// `(at_ms, address, value)`.
    pokes: Vec<(u64, u64, u32)>,
    /// `TIME_MS:REGISTER=VALUE`.
    power: Vec<String>,
    hdmi_at_ms: Option<u64>,
}

impl Actions {
    fn from_list(actions: &[fujimod_camera::Action]) -> Result<Self, String> {
        use fujimod_camera::ActionInput;
        let mut built = Self::default();
        for action in actions {
            let at = action.at_ms;
            let seconds = Duration::from_millis(at).as_secs_f64();
            match &action.input {
                ActionInput::Hold(key) => built.keys.push(format!("t={seconds}:{}", key.control)),
                ActionInput::Release(key) => {
                    built.keys.push(format!("t={seconds}:-{}", key.control));
                }
                ActionInput::Press(press) => {
                    let hold = press.hold_ms.unwrap_or(150);
                    if hold == 0 {
                        return Err("a press's hold_ms must be positive".into());
                    }
                    built.taps.push(format!("{at}:{}:{hold}", press.control));
                }
                ActionInput::Set(set) => built
                    .positions
                    .push(format!("{}={}@{at}", set.control, set.position)),
                ActionInput::Gpio(gpio) => built.gpio.push((at, gpio.port, gpio.pin, gpio.high)),
                ActionInput::Poke(poke) => {
                    if poke.address & 3 != 0 {
                        return Err("a poke address needs 4-byte alignment".into());
                    }
                    built.pokes.push((at, poke.address, poke.value));
                }
                ActionInput::PowerInput(power) => built
                    .power
                    .push(format!("{at}:{}={}", power.register, power.value)),
                ActionInput::Hdmi(sink) => {
                    if !sink.connected {
                        return Err("only connecting an HDMI sink is modeled".into());
                    }
                    if built.hdmi_at_ms.replace(at).is_some() {
                        return Err("an HDMI sink is connected once".into());
                    }
                }
            }
        }
        Ok(built)
    }

    fn keys(&self) -> Option<String> {
        (!self.keys.is_empty()).then(|| self.keys.join(";"))
    }

    fn taps(&self) -> Option<String> {
        (!self.taps.is_empty()).then(|| self.taps.join(","))
    }

    /// Refuse the inputs a processor's runner does not schedule.
    fn refuse(&self, generation: &str, timed_writes: bool, power: bool) -> Result<(), String> {
        if !self.positions.is_empty() {
            return Err(format!("set actions need an XP5 run, not {generation}"));
        }
        if self.hdmi_at_ms.is_some() {
            return Err(format!(
                "hdmi actions need an XP5 HDMI transmitter, not {generation}"
            ));
        }
        if !power && !self.power.is_empty() {
            return Err(format!(
                "power_input actions need an XP4 run, not {generation}"
            ));
        }
        if !timed_writes
            && (self.gpio.iter().any(|gpio| gpio.0 != 0)
                || self.pokes.iter().any(|poke| poke.0 != 0))
        {
            return Err(format!(
                "timed gpio and poke actions need an XP5 run; {generation} applies them at 0 ms"
            ));
        }
        Ok(())
    }
}

impl Boot {
    /// A run flag's path, as given on the command line.
    fn path(&self, text: &Option<String>) -> Option<PathBuf> {
        text.as_deref().map(PathBuf::from)
    }

    /// A path the camera description names, relative to its directory.
    fn camera_path(&self, text: &Option<String>) -> Option<PathBuf> {
        text.as_deref()
            .map(|text| emulator::description::resolve(&self.base, text))
    }

    fn duration(seconds: Option<f64>, what: &str) -> Result<Option<Duration>, String> {
        seconds
            .map(|seconds| {
                Duration::try_from_secs_f64(seconds).map_err(|error| format!("--{what}: {error}"))
            })
            .transpose()
    }

    fn pc_trace(&self) -> Option<boot::TraceOutput> {
        self.run.trace.as_ref().map(|trace| boot::TraceOutput {
            path: PathBuf::from(&trace.path),
            limit: trace.limit.unwrap_or(200_000),
        })
    }

    fn execution_policy(&self, exact_default: bool) -> ExecutionPolicy {
        match self.camera.execution_policy {
            Some(fujimod_camera::ExecutionPolicyName::Exact) => ExecutionPolicy::Exact,
            Some(fujimod_camera::ExecutionPolicyName::Interactive) => ExecutionPolicy::Interactive,
            None if exact_default => ExecutionPolicy::Exact,
            None => ExecutionPolicy::Interactive,
        }
    }

    fn mods(&self) -> Vec<String> {
        self.run.mods.clone()
    }

    /// Refuse a run option the processor's runner does not record.
    fn refuse_run(&self, generation: &str, options: &[(&str, bool)]) -> Result<(), String> {
        for (name, set) in options {
            if *set {
                return Err(format!("--{name} does not apply to {generation} runs"));
            }
        }
        Ok(())
    }

    /// A card as the startup path takes it.
    fn card(card: &emulator::Card) -> CardInsert {
        let (image, files) = match &card.contents {
            CardContents::Image(path) => (Some(path.clone()), Vec::new()),
            CardContents::Files(files) => (
                None,
                files
                    .iter()
                    .map(|(name, file)| {
                        (
                            name.clone(),
                            match file {
                                CardFile::Source(path) => boot::media::CardFile::Path(path.clone()),
                                CardFile::Bytes(bytes) => {
                                    boot::media::CardFile::Bytes(bytes.clone())
                                }
                            },
                        )
                    })
                    .collect(),
            ),
        };
        CardInsert {
            present: card.present,
            write_protected: card.write_protected,
            image,
            files,
        }
    }

    /// The cards a run names, by SD slot; an omitted first card keeps the run's own.
    fn named_cards(&self) -> BTreeMap<u8, CardInsert> {
        [(1, &self.cards.sd_card), (2, &self.cards.sd_card_2)]
            .into_iter()
            .filter_map(|(slot, card)| card.as_ref().map(|card| (slot, Self::card(card))))
            .collect()
    }
}

/// Resolve a boot's camera and actions to its run configuration, files and inputs.
fn resolve_boot(arguments: &Arguments) -> Result<Boot, String> {
    let (camera, base): (CameraDescription, PathBuf) = arguments.camera.parse_json("--camera")?;
    let actions = match &arguments.actions {
        Some(source) => source.parse_json::<fujimod_camera::Actions>("--actions")?.0,
        None => fujimod_camera::Actions::default(),
    };
    let camera = &camera;
    for (field, set) in [
        ("boot", camera.boot.is_some()),
        ("hold", camera.hold.is_some()),
        ("loader_image", camera.loader_image.is_some()),
        ("flash_image", camera.flash_image.is_some()),
        ("run_to_rec", camera.run_to_rec.is_some()),
        ("boot_cache", camera.boot_cache.is_some()),
        ("snapshot", camera.snapshot.is_some()),
    ] {
        if set {
            return Err(format!(
                "{field} opens a camera with `open`, the MCP server or Python; a boot run \
                 starts at power-on (use --load-snapshot to restore a checkpoint)"
            ));
        }
    }
    let mut request = emulator::description::request(camera, &base)?;
    // A boot run reads its internal flash image itself; the camera object names it as a path.
    request.nand_image = None;
    let library = match &request.files {
        emulator::FirmwareInput::Files { .. } => Library::without_directory(),
        emulator::FirmwareInput::Library => Library::from_env(),
    }
    .map_err(|error| error.to_string())?;
    let plan = emulator::plan(&library, &request).map_err(|error| error.to_string())?;
    let read_rom = match plan.boot_rom {
        emulator::BootRomUse::Required => true,
        emulator::BootRomUse::Optional => plan.resolved.reference.firmware.boot_rom.is_some(),
        emulator::BootRomUse::Unused => false,
    };
    let files = match &request.files {
        emulator::FirmwareInput::Library => library.files(&plan.firmware, read_rom),
        emulator::FirmwareInput::Files { update, boot_rom } => library.files_from(
            &plan.firmware,
            update,
            if read_rom { boot_rom.as_deref() } else { None },
        ),
    }
    .map_err(|error| error.to_string())?;
    if read_rom && files.boot_rom.is_none() {
        return Err(format!(
            "firmware {} starts from the boot ROM; firmware_files names no boot_rom",
            plan.firmware
        ));
    }
    let mut resolved = plan.resolved;
    if let Some(backend) = &arguments.run.cpu_backend {
        let backend = match backend.as_str() {
            "unicorn" => RunCpuBackend::Unicorn,
            "dynarmic" => RunCpuBackend::Dynarmic,
            _ => return Err("--cpu-backend must be unicorn or dynarmic".into()),
        };
        resolved
            .configuration
            .preset
            .as_mut()
            .ok_or("--cpu-backend requires a run configuration with factory options")?
            .factory_options
            .cpu_backend = Some(backend);
        fujimod_catalog_resolution::validate_selection(
            &resolved.configuration,
            &resolved.reference,
        )?;
    }
    Ok(Boot {
        resolved,
        files: Files {
            update: boot::FirmwareFile::Bytes(files.update),
            boot_rom: files.boot_rom.map(boot::FirmwareFile::Bytes),
        },
        actions: Actions::from_list(&actions.0)?,
        camera: camera.clone(),
        cards: request.cards,
        settings: request.settings,
        clock_seconds_2000: request.clock_seconds_2000,
        scene: request.scene,
        application_plan: request.application_plan,
        library,
        base,
        run: arguments.run.clone(),
        gui: arguments.gui,
    })
}

/// What a finished run reports to the shell.
struct Outcome {
    success: bool,
}

fn prepare_mod(
    resolved: &ResolvedConfiguration,
    model: fujimod_emulator_cpu::Model,
    manifests: &[String],
) -> Result<Option<boot::mod_loader::Loaded>, String> {
    if manifests.is_empty() {
        return Ok(None);
    }
    let firmware = &resolved.reference.firmware;
    let update = firmware
        .update
        .as_ref()
        .ok_or("mod needs a verified firmware update")?;
    boot::mod_loader::prepare(
        boot::mod_loader::Target {
            id: &firmware.id,
            sha256: &update.sha256,
            model,
        },
        manifests,
        |_, _| Ok(()),
    )
    .map(Some)
}

fn run_exr2(boot_options: Boot) -> Result<Outcome, String> {
    let run = boot_options.run.clone();
    boot_options.refuse_run(
        "EXR II",
        &[
            ("manifest", run.manifest.is_some()),
            ("watch_pcs", run.watch_pcs.is_some()),
            ("watch_writes", run.watch_writes.is_some()),
            ("dump", run.dump.is_some()),
            ("hash", run.hash.is_some()),
            ("save_nor", run.save_nor.is_some()),
        ],
    )?;
    boot_options.actions.refuse("EXR II", false, false)?;
    if boot_options.cards.sd_card_2.is_some() {
        return Err("an EXR II body has one SD card slot".into());
    }
    let mut resolved = boot_options.resolved.clone();
    let mut loaded_mod = prepare_mod(
        &resolved,
        fujimod_emulator_cpu::Model::CortexR5F,
        &boot_options.mods(),
    )?;
    if let Some(path) = &boot_options.scene {
        boot::host::set_exr2_scene(&mut resolved, path)?;
    }
    if let Some(quantum) = run.quantum {
        resolved
            .configuration
            .execution
            .as_mut()
            .ok_or("configuration has no execution policy")?
            .quantum = quantum;
    }
    let edges = boot::host::parse_keys(
        boot_options.actions.keys().as_deref(),
        boot_options.actions.taps().as_deref(),
    )?;
    let automatic_setup = edges.is_empty();
    let mut keys = fujimod_emulator_controls::Keys::new(
        &resolved.reference,
        edges.into_iter().map(|edge| {
            (
                boot::host::duration_ticks(edge.delay, boot::EXR2_TICKS_PER_SECOND),
                edge.name,
                edge.pressed,
            )
        }),
        boot::EXR2_TICKS_PER_SECOND,
    )?;
    let first = boot_options.cards.first();
    let camera = emulator::settings_camera(&boot_options.library, &resolved)
        .map_err(|error| error.to_string())?;
    let options = Options {
        update: boot_options.files.update.clone(),
        boot_rom: boot_options
            .files
            .boot_rom
            .clone()
            .ok_or("this camera needs its boot ROM")?,
        nand: boot_options.camera_path(&boot_options.camera.nand_image),
        settings: None,
        clock_seconds_2000: boot_options.clock_seconds_2000,
        preferred_languages: boot_options
            .settings
            .get("language")
            .cloned()
            .into_iter()
            .collect(),
        camera,
        access_point: None,
        capability_reply: None,
        sd_card_transfer_ticks: first.transfer_ticks,
        card_layout: first.layout.clone(),
        card_initialization_fault: first.initialization_fault.clone(),
    };
    let screenshot = boot_options.path(&run.screenshot);
    let limit = run.instruction_limit.unwrap_or(2_000_000_000);
    let pc_trace = boot_options.pc_trace();
    let execution_policy = boot_options.execution_policy(pc_trace.is_some());
    let mode = boot::discovery_mode(run.discovery);
    let stubs = run
        .stubs
        .iter()
        .map(|specification| boot::host::parse_stub(specification))
        .collect::<Result<Vec<_>, _>>()?;
    let mut session = boot::open(resolved, boot::StartupInputs::Exr2 { options, mode })?.session;
    session.board.install_discovery_callee_stubs(&stubs)?;
    if let Some(path) = boot_options.path(&run.load_snapshot) {
        boot::load_snapshot(&mut session, &path)?;
    }
    boot::media::insert(&mut session, &Boot::card(&first))?;
    for (_, port, pin, high) in &boot_options.actions.gpio {
        fujimod_emulator_controls::set_gpio(&mut session, *port, *pin, *high)?;
    }
    for (_, address, word) in &boot_options.actions.pokes {
        session
            .board
            .write_physical(*address, &word.to_le_bytes())?;
    }
    session.board.set_execution_policy(execution_policy)?;
    let trace = pc_trace
        .as_ref()
        .map(|output| output.begin(&session))
        .transpose()?;
    let duration = Boot::duration(run.seconds, "seconds")?;
    let status = Boot::duration(run.status_every, "status_every")?;
    let directory = boot_options.path(&run.output);
    let outputs = boot::output::Outputs::default();
    if let Some(directory) = &directory {
        outputs.add(0, directory.clone())?;
    }
    let mut patches =
        boot::patch::Schedule::from_path(boot_options.path(&run.patch).as_deref(), &[])?;
    let stubbed = stubs.iter().map(|stub| stub.address).collect::<Vec<_>>();
    let start = session.board.now();
    let mut next_status = status.map(|_| start);
    let mut next_frame = session.board.now();
    let report = boot::to_rec_with_actions(
        &mut session,
        limit,
        duration,
        automatic_setup,
        |session| {
            let key_deadline = keys.advance(session, start)?;
            let patch_deadline = patches.advance(
                &session.board,
                session.board.now(),
                start,
                boot::EXR2_TICKS_PER_SECOND,
            )?;
            if let Some(loaded) = &mut loaded_mod {
                loaded.advance(
                    &boot::mod_loader::BoardMachine(&session.board),
                    session.board.now(),
                    &stubbed,
                )?;
            }
            Ok(key_deadline.into_iter().chain(patch_deadline).min())
        },
        |session, status_report| {
            let now = session.board.now();
            if next_status.is_some_and(|tick| now >= tick) {
                let milliseconds = status.unwrap().as_millis();
                eprintln!(
                    "FUJIMOD status {}",
                    serde_json::to_string(status_report).map_err(|error| error.to_string())?
                );
                next_status = (milliseconds != 0).then(|| {
                    now.saturating_add(
                        ((milliseconds * u128::from(boot::EXR2_TICKS_PER_SECOND)) / 1000)
                            .min(u128::from(u64::MAX)) as u64,
                    )
                });
            }
            if directory.is_some()
                && now >= next_frame
                && let Some(instance) = status_report.scanout_instance
            {
                outputs.capture_rec(session, instance, now)?;
                next_frame = now.saturating_add(boot::EXR2_TICKS_PER_SECOND / 30);
            }
            Ok(())
        },
    );
    outputs.finish()?;
    if let Some((output, trace)) = pc_trace.zip(trace) {
        output.write(trace)?;
    }
    session.board.check_discovery_callee_stubs()?;
    let report = report?;
    if let Some(directory) = directory {
        let path = directory.join("board-state.json");
        let state =
            serde_json::to_vec_pretty(&session.board.state()).map_err(|error| error.to_string())?;
        std::fs::write(&path, state).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    if let Some(path) = boot_options.path(&run.save_snapshot) {
        boot::save_snapshot(&session, &path)?;
    }
    let mut receipt = serde_json::to_value(&report).map_err(|error| error.to_string())?;
    if let Some(loaded) = loaded_mod {
        receipt["mod"] = loaded.report(&boot::mod_loader::BoardMachine(&session.board));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&receipt).map_err(|error| error.to_string())?
    );
    if let Some(path) = screenshot {
        std::fs::write(&path, boot::screenshot(&session)?)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(Outcome {
        success: matches!(report.status.as_str(), "rec" | "time-limit"),
    })
}

fn run_xp5(boot_options: Boot) -> Result<Outcome, String> {
    let run_options = boot_options.run.clone();
    boot_options.refuse_run("XP5", &[("save_nor", run_options.save_nor.is_some())])?;
    boot_options.actions.refuse("XP5", true, false)?;
    if boot_options.camera.nand_image.is_some() {
        return Err("camera.nand_image does not apply to XP5 runs".into());
    }
    let pc_trace = boot_options.pc_trace();
    let execution_policy = boot_options.execution_policy(pc_trace.is_some());
    let gpio = (!boot_options.actions.gpio.is_empty()).then(|| {
        boot_options
            .actions
            .gpio
            .iter()
            .map(|(at, port, pin, high)| format!("{port}:{pin}={}@{at}", u8::from(*high)))
            .collect::<Vec<_>>()
            .join(",")
    });
    let gui = boot_options.gui;
    let run = xp5::Run {
        configuration: boot_options.resolved.clone(),
        load_snapshot: boot_options.path(&run_options.load_snapshot),
        save_snapshot: boot_options.path(&run_options.save_snapshot),
        manifest: boot_options.path(&run_options.manifest),
        watch_pcs: run_options.watch_pcs.clone(),
        watch_writes: run_options.watch_writes.clone(),
        pc_trace,
        screenshot: boot_options.path(&run_options.screenshot),
        dump: run_options.dump.clone(),
        hash: run_options.hash.clone(),
        update: boot_options.files.update.clone(),
        boot_rom: boot_options.files.boot_rom.clone(),
        cards: boot_options.named_cards(),
        keys: boot_options.actions.keys(),
        gpio,
        hdmi_connect_at_ms: boot_options.actions.hdmi_at_ms,
        pokes: boot_options
            .actions
            .pokes
            .iter()
            .map(|(at, address, value)| format!("{address:#x}={value:#x}@{at}"))
            .collect(),
        press: boot_options.actions.taps(),
        settings: (!boot_options.actions.positions.is_empty())
            .then(|| boot_options.actions.positions.join(",")),
        camera_settings: boot_options.settings.clone(),
        discovery: run_options.discovery,
        stubs: run_options.stubs.clone(),
        patch_file: boot_options.path(&run_options.patch),
        mods: boot_options.mods(),
        scene: boot_options.scene.clone(),
        quantum: run_options.quantum,
        seconds: Boot::duration(run_options.seconds, "seconds")?,
        instruction_limit: run_options.instruction_limit,
        wall_seconds: Boot::duration(run_options.wall_seconds, "wall_seconds")?,
        status_every: Boot::duration(run_options.status_every, "status_every")?
            .unwrap_or(Duration::from_secs(5)),
        execution_policy,
        output: boot_options
            .path(&run_options.output)
            .ok_or("an XP5 run needs --output")?,
    };
    if gui {
        let room = fujimod_emulator_gui::Room::loopback()?;
        let endpoint = room.endpoint().url.clone();
        let host = xp5::RunHost::open(run)?;
        let mut window = fujimod_emulator_gui::launch::connect(&endpoint)?;
        let report = fujimod_emulator_gui::serve(host, room, &mut || window.open())?;
        window.wait()?;
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
        return Ok(Outcome { success: true });
    }
    let report = xp5::run_boot(run)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
    );
    Ok(Outcome { success: true })
}

fn run_application(options: Boot) -> Result<Outcome, String> {
    let run = options.run.clone();
    options.refuse_run(
        "application-profile",
        &[
            ("manifest", run.manifest.is_some()),
            ("watch_pcs", run.watch_pcs.is_some()),
            ("watch_writes", run.watch_writes.is_some()),
            ("dump", run.dump.is_some()),
            ("hash", run.hash.is_some()),
        ],
    )?;
    options.actions.refuse("application-profile", false, true)?;
    if options.cards.sd_card_2.is_some() {
        return Err("an application-profile start has one SD card slot".into());
    }
    let resolved = options.resolved.clone();
    let card = Boot::card(&options.cards.first());
    if !card.present || card.write_protected {
        return Err("an application-profile start always has a writable card".into());
    }
    let request = boot::application::Startup {
        update: options.files.update.clone(),
        plan: options
            .application_plan
            .clone()
            .ok_or("this camera needs application_plan")?,
        card_image: card.image,
        card_files: card.files,
        camera_settings: options.settings.clone(),
        discovery: run.discovery,
        quantum: run.quantum,
    };
    let instructions = run.instruction_limit;
    let duration = Boot::duration(run.seconds, "seconds")?;
    let output = options.path(&run.output);
    let screenshot = options.path(&run.screenshot);
    let status = Boot::duration(run.status_every, "status_every")?;
    let load_snapshot = options.path(&run.load_snapshot);
    let save_snapshot = options.path(&run.save_snapshot);
    let save_nor = options.path(&run.save_nor);
    let mut power_inputs = std::collections::VecDeque::from(if options.actions.power.is_empty() {
        Vec::new()
    } else {
        boot::host::parse_power_inputs(&options.actions.power.join(","))?
    });
    let trace_output = options.pc_trace();
    let cpu_backend = resolved.configuration.cpu_backend();
    let policy =
        options.execution_policy(trace_output.is_some() || cpu_backend == RunCpuBackend::Dynarmic);
    let mut patches = boot::patch::Schedule::from_path(options.path(&run.patch).as_deref(), &[])?;
    let scene = options
        .scene
        .as_deref()
        .map(boot::host::read_scene)
        .transpose()?;
    let mut keys = boot::host::parse_keys(
        options.actions.keys().as_deref(),
        options.actions.taps().as_deref(),
    )?
    .into_iter()
    .map(|edge| {
        let wire = boot::application::KeyWire::resolve(&resolved.reference, &edge.name)?;
        Ok((edge.delay, wire, edge.pressed))
    })
    .collect::<Result<std::collections::VecDeque<_>, String>>()?;
    let mut loaded_mod = prepare_mod(
        &resolved,
        fujimod_emulator_cpu::Model::CortexA7,
        &options.mods(),
    )?;
    if resolved.reference.soc.id != "xp4" && (!power_inputs.is_empty() || save_nor.is_some()) {
        return Err(
            "power_input actions and --save-nor need an XP4 application-profile board".into(),
        );
    }
    let mut started = boot::application::open(&resolved, request)?;
    let stubs = run
        .stubs
        .iter()
        .map(|specification| boot::host::parse_stub(specification))
        .collect::<Result<Vec<_>, _>>()?;
    let stubbed = stubs.iter().map(|stub| stub.address).collect::<Vec<_>>();
    if let Some(path) = load_snapshot {
        started.load_snapshot(&path)?;
    }
    let boot::application::Board::Shared(session) = &mut started.board;
    session.board.set_execution_policy(policy)?;
    session.board.install_discovery_callee_stubs(&stubs)?;
    let trace = trace_output
        .as_ref()
        .map(|output| output.begin(session))
        .transpose()?;
    if let Some(scene) = scene {
        started.board.set_scene(&scene)?;
    }
    for (_, port, pin, high) in &options.actions.gpio {
        started.board.gpio_input(*port, *pin, *high)?;
    }
    for (_, address, word) in &options.actions.pokes {
        started.board.write(*address, &word.to_le_bytes())?;
    }
    let frames = boot::output::Outputs::default();
    if let Some(directory) = &output {
        frames.add(0, directory.clone())?;
    }
    let (_, start) = started.progress();
    let status_period =
        status.map(|duration| boot::host::duration_ticks(duration, started.ticks_per_second));
    let mut next_status = status_period.map(|_| start);
    let mut next_frame = output.as_ref().map(|_| start);
    let mut sequences = BTreeMap::new();
    let mut state = started.run_with(instructions, duration, |board| {
        let (counts, now) = board.progress();
        let deadline =
            |delay| start.saturating_add(boot::host::duration_ticks(delay, board.ticks_per_second));
        let mut next_key = None;
        while let Some((delay, wire, pressed)) = keys.front() {
            let at = deadline(*delay);
            if at > now {
                next_key = Some(at);
                break;
            }
            board.board.key_input(wire, *pressed)?;
            keys.pop_front();
        }
        let mut next_power = None;
        while let Some(input) = power_inputs.front() {
            let at = deadline(input.delay);
            if at > now {
                next_power = Some(at);
                break;
            }
            board.board.power_input(input.register, input.value)?;
            power_inputs.pop_front();
        }
        let next_patch = patches.advance(&board.board, now, start, board.ticks_per_second)?;
        if let Some(loaded) = &mut loaded_mod {
            loaded.advance(&board.board, now, &stubbed)?;
        }
        if next_status.is_some_and(|at| at <= now) {
            eprintln!(
                "FUJIMOD status {}",
                serde_json::json!({
                    "engine": board.engine,
                    "cpu_backend": cpu_backend,
                    "instructions": counts,
                    "now": now
                })
            );
            next_status = status_period
                .filter(|period| *period != 0)
                .map(|period| now.saturating_add(period));
        }
        if next_frame.is_some_and(|at| at <= now) {
            frames.capture(
                output.as_deref().unwrap(),
                board.display_frames(),
                &mut sequences,
            )?;
            next_frame = Some(now.saturating_add((board.ticks_per_second / 30).max(1)));
        }
        Ok(next_status
            .into_iter()
            .chain(next_frame)
            .chain(next_patch)
            .chain(next_key)
            .chain(next_power)
            .min())
    })?;
    if let Some(directory) = &output {
        frames.capture(directory, started.display_frames(), &mut sequences)?;
        frames.finish()?;
    }
    if let Some(output) = trace_output
        && let Some(trace) = trace
    {
        output.write(trace)?;
    }
    let boot::application::Board::Shared(session) = &started.board;
    session.board.check_discovery_callee_stubs()?;
    started.write_back_card()?;
    if let Some(path) = save_snapshot {
        started.save_snapshot(&path)?;
    }
    if let Some(path) = save_nor {
        std::fs::write(&path, started.board.nor_medium()?)
            .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    if let Some(path) = screenshot {
        let shown = started.display_frames();
        let frame = shown.first().ok_or("no active scanout published a frame")?;
        boot::output::Outputs::write_screenshot(&path, frame)?;
    }
    if let Some(loaded) = loaded_mod {
        state["mod"] = loaded.report(&started.board);
    }
    if !started.camera_settings.is_null() {
        state["camera_settings"] = started.camera_settings.clone();
    }
    state["cpu_backend"] = serde_json::json!(cpu_backend);
    if let Some(output) = output {
        let report = serde_json::to_string_pretty(&state).map_err(|error| error.to_string())?;
        std::fs::create_dir_all(&output).map_err(|error| error.to_string())?;
        std::fs::write(output.join("board-state.json"), &report)
            .map_err(|error| error.to_string())?;
        println!("{report}");
    } else {
        let mut stdout = io::BufWriter::new(io::stdout().lock());
        serde_json::to_writer_pretty(&mut stdout, &state).map_err(|error| error.to_string())?;
        stdout.write_all(b"\n").map_err(|error| error.to_string())?;
        stdout.flush().map_err(|error| error.to_string())?;
    }
    Ok(Outcome { success: true })
}

/// Open any camera from its files and run it live in the desktop window.
fn run_window(options: Boot) -> Result<Outcome, String> {
    let request = boot::Request {
        update: options.files.update.clone(),
        boot_rom: options.files.boot_rom.clone(),
        nand: options.camera_path(&options.camera.nand_image),
        settings: None,
        camera: emulator::settings_camera(&options.library, &options.resolved)
            .map_err(|error| error.to_string())?,
        application_plan: options.application_plan.clone(),
        cards: options.named_cards(),
        discovery: options.run.discovery,
        sd_card_transfer_ticks: None,
        card_layout: None,
        card_initialization_fault: None,
        camera_settings: if boot::startup_path(&options.resolved)? == StartupPath::Exr2 {
            BTreeMap::new()
        } else {
            options.settings.clone()
        },
        clock_seconds_2000: options.clock_seconds_2000,
        scene: options.scene.clone(),
    };
    let selection = options.resolved.reference.clone();
    let camera = boot::open_request(options.resolved.clone(), request)?;
    let room = fujimod_emulator_gui::Room::loopback()?;
    let mut window = fujimod_emulator_gui::launch::connect(&room.endpoint().url)?;
    let report = fujimod_emulator_gui::serve(
        fujimod_emulator_gui::Live::new(camera, selection),
        room,
        &mut || window.open(),
    )?;
    window.wait()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
    );
    Ok(Outcome { success: true })
}

/// Open a camera with FujiMod from its description and report the boot it reached.
fn run_open(arguments: Arguments) -> Result<Outcome, String> {
    let started = Instant::now();
    let (description, base): (CameraDescription, PathBuf) =
        arguments.camera.parse_json("--camera")?;
    if description.snapshot.is_some() {
        return Err("open boots a camera; restore a snapshot with boot --load-snapshot".into());
    }
    let request = camera::Request::from_description(&description, &base)
        .map_err(|error| error.to_string())?;
    let library = match &request.camera.files {
        emulator::FirmwareInput::Files { .. } => Library::without_directory(),
        emulator::FirmwareInput::Library => Library::from_env(),
    }
    .map_err(|error| error.to_string())?;
    let state = StateDir::from_env();
    let preset = request.preset;
    let opened = camera::open(&library, &state, request).map_err(|error| error.to_string())?;
    let mut report = serde_json::json!({
        "body": description.body,
        "firmware": opened.firmware,
        "boot": preset.name(),
        "release": opened.release,
        "execution_policy": opened.policy.name(),
        "engine": opened.engine,
        "stage": opened.stage.name(),
        "rec_unobserved": opened.rec_unobserved,
        "cached": opened.cached,
        "wall_seconds": started.elapsed().as_secs_f64(),
    });
    if arguments.gui {
        let selection = opened.resolved.reference.clone();
        let room = fujimod_emulator_gui::Room::loopback()?;
        let mut window = fujimod_emulator_gui::launch::connect(&room.endpoint().url)?;
        let served = fujimod_emulator_gui::serve(
            fujimod_emulator_gui::Live::new(opened.into_camera(), selection),
            room,
            &mut || window.open(),
        )?;
        window.wait()?;
        report["window"] = served;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
    );
    Ok(Outcome { success: true })
}

fn run() -> Result<Outcome, String> {
    match parse_command(std::env::args_os().skip(1))? {
        None => {
            print!("{HELP}");
            Ok(Outcome { success: true })
        }
        Some(Command::Open(arguments)) => run_open(arguments),
        Some(Command::Boot(arguments)) => run_boot(&arguments),
    }
}

fn run_boot(arguments: &Arguments) -> Result<Outcome, String> {
    let boot = resolve_boot(arguments)?;
    let path = boot::startup_path(&boot.resolved)?;
    if boot.gui && path != StartupPath::Xp5 {
        return run_window(boot);
    }
    match path {
        StartupPath::Exr2 => run_exr2(boot),
        StartupPath::Xp5 => run_xp5(boot),
        StartupPath::Application => run_application(boot),
    }
}

fn main() -> ExitCode {
    fujimod_emulator_exr2_board::register_board_builder();
    fujimod_emulator_models::register();
    match run() {
        Ok(Outcome { success: true }) => ExitCode::SUCCESS,
        Ok(Outcome { success: false }) => ExitCode::from(2),
        Err(error) => {
            eprintln!("fujimod-emulator: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args<'a>(args: &'a [&'a str]) -> impl Iterator<Item = OsString> + 'a {
        args.iter().map(OsString::from)
    }

    fn arguments(command: Option<Command>) -> Arguments {
        match command {
            Some(Command::Boot(arguments) | Command::Open(arguments)) => arguments,
            None => panic!("no command"),
        }
    }

    #[test]
    fn the_camera_and_actions_come_from_files_stdin_or_inline_json() {
        assert!(parse_command(args(&["--help"])).unwrap().is_none());
        assert!(parse_command(args(&["boot", "--help"])).unwrap().is_none());
        assert!(parse_command(args(&["xp5", "boot"])).is_err());
        assert!(parse_command(args(&[])).is_err());
        let file = arguments(parse_command(args(&["boot", "--camera", "xt5.json"])).unwrap());
        assert_eq!(file.camera, Source::File(PathBuf::from("xt5.json")));
        assert!(file.actions.is_none());
        let both = arguments(
            parse_command(args(&[
                "boot",
                "--camera",
                r#"{"body":"x70"}"#,
                "--actions",
                "-",
            ]))
            .unwrap(),
        );
        assert_eq!(both.camera, Source::Inline(r#"{"body":"x70"}"#.into()));
        assert_eq!(both.actions, Some(Source::Stdin));
        let list = arguments(
            parse_command(args(&["boot", "--camera", "a.json", "--actions", "[]"])).unwrap(),
        );
        assert_eq!(list.actions, Some(Source::Inline("[]".into())));
        let stdin = parse_command(args(&["boot", "--camera", "-", "--actions", "-"]));
        assert!(stdin.err().unwrap().contains("standard input"));
        assert!(
            parse_command(args(&["boot"]))
                .err()
                .unwrap()
                .contains("needs --camera")
        );
        assert!(parse_command(args(&["boot", "--camera", "a", "--camera", "b"])).is_err());
        assert!(parse_command(args(&["boot", "--camera", "a", "--gui", "--gui"])).is_err());
    }

    #[test]
    fn per_input_flags_are_gone_and_run_flags_parse() {
        for flag in [
            "--configuration",
            "--press",
            "--card-file",
            "--camera-settings",
            "--body",
            "--request",
        ] {
            let error = parse_command(args(&["boot", "--camera", "a.json", flag, "x"]))
                .err()
                .unwrap();
            assert!(error.contains("unknown option"), "{flag}: {error}");
        }
        let boot = arguments(
            parse_command(args(&[
                "boot",
                "--camera",
                "a.json",
                "--output",
                "out",
                "--seconds",
                "2.5",
                "--wall-seconds",
                "12.5",
                "--trace",
                "trace.bin",
                "--trace-limit",
                "10",
                "--mod",
                "a",
                "--mod",
                "b",
                "--discovery",
            ]))
            .unwrap(),
        );
        assert_eq!(boot.run.output.as_deref(), Some("out"));
        assert_eq!(boot.run.seconds, Some(2.5));
        assert_eq!(boot.run.wall_seconds, Some(12.5));
        assert_eq!(
            boot.run.trace,
            Some(TraceFlags {
                path: "trace.bin".into(),
                limit: Some(10)
            })
        );
        assert_eq!(boot.run.mods, ["a", "b"]);
        assert!(boot.run.discovery);
        assert!(parse_command(args(&["boot", "--camera", "a", "--seconds", "-1"])).is_err());
        assert!(parse_command(args(&["boot", "--camera", "a", "--trace-limit", "3"])).is_err());
        assert!(
            parse_command(args(&[
                "boot", "--camera", "a", "--output", "x", "--output", "y"
            ]))
            .is_err()
        );
        assert!(parse_command(args(&["open", "--camera", "a.json", "--output", "out"])).is_err());
    }

    #[test]
    fn a_camera_file_resolves_paths_from_its_directory() {
        let directory =
            std::env::temp_dir().join(format!("fujimod-cli-camera-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("x70.json");
        std::fs::write(&file, r#"{"body": "x70"}"#).unwrap();
        let (camera, base): (CameraDescription, PathBuf) =
            Source::File(file).parse_json("--camera").unwrap();
        assert_eq!(base, directory);
        assert_eq!(camera.body, "x70");
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn actions_become_the_runners_schedules() {
        let actions: fujimod_camera::Actions = serde_json::from_value(serde_json::json!([
            {"at_ms": 0, "hold": {"control": "MENU_OK"}},
            {"at_ms": 1500, "release": {"control": "MENU_OK"}},
            {"at_ms": 2000, "press": {"control": "UP", "hold_ms": 100}},
            {"at_ms": 2500, "press": {"control": "DOWN"}},
            {"at_ms": 3000, "set": {"control": "iso", "position": "A"}},
            {"at_ms": 0, "gpio": {"port": 1, "pin": 2, "high": true}},
            {"at_ms": 10, "poke": {"address": 4096, "value": 7}},
            {"at_ms": 22300, "power_input": {"register": 0, "value": 33920}},
            {"at_ms": 500, "hdmi": {"connected": true}}
        ]))
        .unwrap();
        let actions = Actions::from_list(&actions.0).unwrap();
        assert_eq!(actions.keys().unwrap(), "t=0:MENU_OK;t=1.5:-MENU_OK");
        assert_eq!(actions.taps().unwrap(), "2000:UP:100,2500:DOWN:150");
        assert_eq!(actions.positions, ["iso=A@3000"]);
        assert_eq!(actions.gpio, [(0, 1, 2, true)]);
        assert_eq!(actions.pokes, [(10, 4096, 7)]);
        assert_eq!(actions.power, ["22300:0=33920"]);
        assert_eq!(actions.hdmi_at_ms, Some(500));
        let edges =
            boot::host::parse_keys(actions.keys().as_deref(), actions.taps().as_deref()).unwrap();
        assert_eq!(edges.len(), 6);
        assert!(
            actions
                .refuse("EXR II", false, false)
                .err()
                .unwrap()
                .contains("XP5")
        );
        let misaligned: fujimod_camera::Actions = serde_json::from_value(serde_json::json!([
            {"at_ms": 0, "poke": {"address": 2, "value": 1}}
        ]))
        .unwrap();
        assert!(Actions::from_list(&misaligned.0).is_err());
    }

    #[test]
    fn a_boot_refuses_the_open_only_camera_fields() {
        for field in ["boot", "snapshot", "hold"] {
            let text = format!(r#"{{"body": "x70", "{field}": "stock"}}"#);
            let error = resolve_boot(&Arguments {
                camera: Source::Inline(text),
                actions: None,
                gui: false,
                run: RunFlags::default(),
            })
            .err()
            .unwrap();
            assert!(error.starts_with(field), "{field}: {error}");
        }
    }
}
