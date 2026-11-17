//! Native boot command; every binary input is selected by path, for every processor generation.
use fujimod_catalog_configuration::{RunEngine, RunInputs};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_boot::{self as boot, Options, xp5};
use fujimod_emulator_cpu::ExecutionPolicy;
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf, process::ExitCode, time::Duration};

const HELP: &str = "Usage: fujimod-emulator boot --configuration ID --update UPDATE.DAT [--boot-rom ROM.BIN] \\\n  [--overlay ID]... [--engine unicorn|hypervisor] [--screenshot LCD.PNG] [--instruction-limit COUNT] [--execution-policy interactive|exact]\n\n\
The run configuration selects the camera, its firmware and how the run starts; the processor\n\
generation only decides which of these options apply.\n\n\
Boot configuration: x70-110-rec. The ROM dump is required.\n\
x70-110-rec starts from the reference camera's settings bank with setup done;\n\
--camera names its directory of nand-<section>.bin.zst sections\n\
(catalog/bodies/x70/cameras/reference).\n\
A run that declares no settings takes --nand or --settings instead.\n\
NAND accepts a committed image or 128 MiB main-area image without OOB/spare.\n\
Settings accepts a sixteen-byte MCU persistent packet.\n\
First-start menus are accepted through MENU/OK ADC input.\n\
EXR II options: --nand IMAGE --settings MCU.BIN --camera DIR --language TAG --clock-seconds-2000 N.\n\
XP5 options: --output DIR (required), --seconds N, --status-every N, --quantum N, --keys SCRIPT,\n\
--discovery, --stub NAME@ADDR=VALUE, --patch FILE, --mod MANIFEST, --scene PATH,\n\
--card-image IMAGE, --card-file NAME=PATH (ROM start only).\n\
--manifest FILE, --watch-pcs ADDR[,ADDR...], --dump ADDR:LEN[;ADDR:LEN...], --hash ADDR:LEN[;ADDR:LEN...]\n\
Prints a JSON boot report; exits 0 at REC (EXR II) or at the end of the run (XP5), 2 at a stopped frontier.\n";

/// Options that take a value, in help order; `--discovery` is the only flag.
const VALUED: [&str; 28] = [
    "--configuration",
    "--overlay",
    "--engine",
    "--language",
    "--update",
    "--boot-rom",
    "--nand",
    "--settings",
    "--camera",
    "--screenshot",
    "--instruction-limit",
    "--execution-policy",
    "--clock-seconds-2000",
    "--output",
    "--seconds",
    "--status-every",
    "--quantum",
    "--keys",
    "--stub",
    "--patch",
    "--mod",
    "--scene",
    "--card-image",
    "--card-file",
    "--manifest",
    "--watch-pcs",
    "--dump",
    "--hash",
];

/// The options of one `boot` command, before the processor generation is known.
struct Boot {
    values: BTreeMap<String, OsString>,
    overlays: Vec<String>,
    preferred_languages: Vec<String>,
    stubs: Vec<String>,
    mods: Vec<String>,
    card_files: Vec<String>,
    discovery: bool,
}

fn parse(args: impl Iterator<Item = OsString>) -> Result<Option<Boot>, String> {
    let mut args = args.peekable();
    if args.peek().is_some_and(|a| a == "--help" || a == "-h") {
        return Ok(None);
    }
    if args.next().as_deref() != Some(std::ffi::OsStr::new("boot")) {
        return Err(HELP.into());
    }
    if args.peek().is_some_and(|a| a == "--help" || a == "-h") {
        return Ok(None);
    }
    let mut boot = Boot {
        values: BTreeMap::new(),
        overlays: Vec::new(),
        preferred_languages: Vec::new(),
        stubs: Vec::new(),
        mods: Vec::new(),
        card_files: Vec::new(),
        discovery: false,
    };
    while let Some(key) = args.next() {
        let key = key.to_str().ok_or("option name is not UTF-8")?.to_owned();
        if key == "--discovery" {
            if std::mem::replace(&mut boot.discovery, true) {
                return Err("duplicate option: --discovery".into());
            }
            continue;
        }
        if !VALUED.contains(&key.as_str()) {
            return Err(format!("unknown option: {key}"));
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {key}"))?;
        let text = |value: OsString, what: &str| {
            value.into_string().map_err(|_| format!("{what} must be UTF-8"))
        };
        match key.as_str() {
            "--language" => boot.preferred_languages.push(text(value, "language tag")?),
            "--stub" => boot.stubs.push(text(value, "stub")?),
            "--mod" => boot.mods.push(text(value, "mod manifest")?),
            "--card-file" => boot.card_files.push(text(value, "card file")?),
            "--overlay" => {
                let id = text(value, "overlay ID")?;
                if boot.overlays.contains(&id) {
                    return Err(format!("duplicate overlay: {id}"));
                }
                boot.overlays.push(id);
            }
            _ => {
                if boot.values.insert(key.clone(), value).is_some() {
                    return Err(format!("duplicate option: {key}"));
                }
            }
        }
    }
    Ok(Some(boot))
}

impl Boot {
    fn take(&mut self, key: &str) -> Option<OsString> {
        self.values.remove(key)
    }
    fn text(&mut self, key: &str) -> Result<Option<String>, String> {
        self.take(key)
            .map(|value| value.into_string().map_err(|_| format!("{key} must be UTF-8")))
            .transpose()
    }
    fn path(&mut self, key: &str) -> Option<PathBuf> {
        self.take(key).map(PathBuf::from)
    }
    fn required(&mut self, key: &str) -> Result<OsString, String> {
        self.take(key).ok_or_else(|| format!("required option: {key}"))
    }
    /// Refuse the options of another generation that the run set.
    fn refuse(&self, generation: &str, keys: &[&str]) -> Result<(), String> {
        for key in keys {
            if self.values.contains_key(*key) {
                return Err(format!("{key} does not apply to {generation} runs"));
            }
        }
        Ok(())
    }
    fn resolve(&mut self, configuration_id: &str) -> Result<ResolvedConfiguration, String> {
        let engine = self
            .text("--engine")?
            .map(|value| {
                serde_json::from_value::<RunEngine>(serde_json::Value::String(value))
                    .map_err(|error| format!("--engine: {error}"))
            })
            .transpose()?;
        let mut resolved = ResolvedConfiguration::with_overlays(configuration_id, &self.overlays)?;
        if let Some(engine) = engine {
            resolved.configuration.execution.as_mut()
                .ok_or_else(|| format!("configuration {configuration_id} declares no execution policy"))?
                .engine = engine;
        }
        Ok(resolved)
    }
    fn instruction_limit(&mut self) -> Result<Option<u64>, String> {
        self.text("--instruction-limit")?
            .map(|value| match value.parse::<u64>() {
                Ok(0) => Err("instruction limit must be positive".into()),
                Ok(limit) => Ok(limit),
                Err(_) => Err("instruction limit must be an unsigned integer".into()),
            })
            .transpose()
    }

    fn execution_policy(&mut self) -> Result<ExecutionPolicy, String> {
        Ok(self
            .text("--execution-policy")?
            .map(|value| ExecutionPolicy::parse(&value))
            .transpose()?
            .unwrap_or(ExecutionPolicy::Interactive))
    }
}

/// What a finished run reports to the shell.
struct Outcome {
    success: bool,
}

fn run_exr2(mut boot_options: Boot, resolved: ResolvedConfiguration) -> Result<Outcome, String> {
    boot_options.refuse(
        "EXR II",
        &["--output", "--seconds", "--status-every", "--quantum", "--keys", "--patch", "--scene", "--card-image", "--manifest", "--watch-pcs", "--dump", "--hash"],
    )?;
    if boot_options.discovery || !boot_options.stubs.is_empty() || !boot_options.mods.is_empty() || !boot_options.card_files.is_empty() {
        return Err("--discovery, --stub, --mod and --card-file do not apply to EXR II runs".into());
    }
    let clock_seconds_2000 = boot_options
        .text("--clock-seconds-2000")?
        .map(|v| {
            v.parse::<u32>()
                .map_err(|_| "clock seconds must be an unsigned 32-bit integer".to_owned())
        })
        .transpose()?;
    let options = Options {
        update: boot_options.required("--update")?.into(),
        boot_rom: boot_options.required("--boot-rom")?.into(),
        nand: boot_options.path("--nand"),
        settings: boot_options.path("--settings"),
        clock_seconds_2000,
        preferred_languages: std::mem::take(&mut boot_options.preferred_languages),
        camera: boot_options.path("--camera"),
        access_point: None,
        capability_reply: None,
    };
    let screenshot = boot_options.path("--screenshot");
    let limit = boot_options.instruction_limit()?.unwrap_or(2_000_000_000);
    let execution_policy = boot_options.execution_policy()?;
    let mut session = boot::open(resolved, boot::StartupInputs::Exr2(options))?.into_exr2()?;
    session.board.set_execution_policy(execution_policy)?;
    let report = boot::to_rec(&mut session, limit)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
    );
    if let Some(path) = screenshot {
        std::fs::write(&path, boot::screenshot(&session)?)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(Outcome { success: report.status == "rec" })
}

fn run_xp5(mut boot_options: Boot, resolved: ResolvedConfiguration) -> Result<Outcome, String> {
    boot_options.refuse(
        "XP5",
        &["--nand", "--settings", "--camera", "--clock-seconds-2000", "--screenshot"],
    )?;
    if !boot_options.preferred_languages.is_empty() {
        return Err("--language does not apply to XP5 runs".into());
    }
    let card_files = boot_options
        .card_files
        .iter()
        .map(|specification| {
            specification
                .split_once('=')
                .filter(|(name, path)| !name.is_empty() && !path.is_empty())
                .map(|(name, path)| (name.to_owned(), PathBuf::from(path)))
                .ok_or("--card-file needs NAME=PATH")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let run = xp5::Run {
        configuration: resolved,
        manifest: boot_options.path("--manifest"),
        watch_pcs: boot_options.text("--watch-pcs")?,
        dump: boot_options.text("--dump")?,
        hash: boot_options.text("--hash")?,
        update: boot_options.required("--update")?.into(),
        boot_rom: boot_options.path("--boot-rom"),
        card_image: boot_options.path("--card-image"),
        card_files,
        keys: boot_options.text("--keys")?,
        discovery: boot_options.discovery,
        stubs: std::mem::take(&mut boot_options.stubs),
        patch_file: boot_options.path("--patch"),
        mods: std::mem::take(&mut boot_options.mods),
        scene: boot_options.path("--scene"),
        quantum: boot_options
            .text("--quantum")?
            .map(|value| xp5::parse_quantum(&value))
            .transpose()?,
        seconds: boot_options
            .text("--seconds")?
            .map(|value| xp5::parse_seconds(&value))
            .transpose()?,
        instruction_limit: boot_options.instruction_limit()?,
        status_every: boot_options
            .text("--status-every")?
            .map(|value| xp5::parse_seconds(&value))
            .transpose()?
            .unwrap_or(Duration::from_secs(5)),
        execution_policy: boot_options.execution_policy()?,
        output: boot_options.path("--output").ok_or("required option: --output")?,
    };
    let report = xp5::run_boot(run)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
    );
    Ok(Outcome { success: true })
}

fn run() -> Result<Outcome, String> {
    let Some(mut boot_options) = parse(std::env::args_os().skip(1))? else {
        print!("{HELP}");
        return Ok(Outcome { success: true });
    };
    let configuration_id = boot_options
        .text("--configuration")?
        .ok_or("required option: --configuration")?;
    let resolved = boot_options.resolve(&configuration_id)?;
    match resolved.configuration.inputs.as_ref() {
        Some(RunInputs::Exr2(_)) => run_exr2(boot_options, resolved),
        Some(RunInputs::Xp5(_)) => run_xp5(boot_options, resolved),
        Some(RunInputs::Xp4(_)) => Err("boot does not support XP4 configurations yet".into()),
        None => Err(format!("configuration {configuration_id} declares no startup inputs")),
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

    #[test]
    fn options_are_unique_and_known() {
        assert!(parse(args(&["--help"])).unwrap().is_none());
        assert!(parse(args(&["boot", "--help"])).unwrap().is_none());
        assert!(parse(args(&["xp5", "boot"])).is_err());
        assert!(parse(args(&["boot", "--configuration", "a", "--configuration", "b"])).is_err());
        assert!(parse(args(&["boot", "--discovery", "--discovery"])).is_err());
        assert!(parse(args(&["boot", "--body", "x70"])).is_err());
        let boot = parse(args(&["boot", "--overlay", "usb", "--overlay", "service", "--discovery"]))
            .unwrap()
            .unwrap();
        assert_eq!(boot.overlays, ["usb", "service"]);
        assert!(boot.discovery);
        assert!(parse(args(&["boot", "--overlay", "usb", "--overlay", "usb"])).is_err());
    }

    #[test]
    fn explicit_engine_applies_after_overlays() {
        let id = "x100vi-131";
        let flags = ["boot", "--overlay", "x100vi-131-rom-start"];
        let mut boot = parse(args(&flags)).unwrap().unwrap();
        let original = boot.resolve(id).unwrap().configuration;
        assert_eq!(original.execution.unwrap().engine, RunEngine::Hypervisor);
        let mut boot = parse(args(&[&flags[..], &["--engine", "unicorn"]].concat()))
            .unwrap().unwrap();
        let overridden = boot.resolve(id).unwrap().configuration;
        assert_eq!(overridden.start, original.start);
        let mut expected = original.execution.unwrap();
        expected.engine = RunEngine::Unicorn;
        assert_eq!(overridden.execution, Some(expected));
        let mut boot = parse(args(&["boot", "--engine", "hvf"])).unwrap().unwrap();
        assert!(boot.resolve(id).err().unwrap().starts_with("--engine:"));
    }

    #[test]
    fn options_of_another_generation_are_refused() {
        let boot = parse(args(&["boot", "--nand", "image.bin"])).unwrap().unwrap();
        assert!(boot.refuse("XP5", &["--nand"]).unwrap_err().contains("does not apply to XP5"));
        assert!(boot.refuse("XP5", &["--settings"]).is_ok());
    }

    #[test]
    fn execution_policy_accepts_interactive_and_exact_and_rejects_unknown_values() {
        for (value, expected) in [
            ("interactive", ExecutionPolicy::Interactive),
            ("exact", ExecutionPolicy::Exact),
        ] {
            let mut boot = parse(args(&["boot", "--execution-policy", value])).unwrap().unwrap();
            assert_eq!(boot.execution_policy().unwrap(), expected);
        }
        let mut boot = parse(args(&["boot", "--execution-policy", "approximate"])).unwrap().unwrap();
        assert_eq!(boot.execution_policy().unwrap_err(), "unknown CPU execution policy: approximate");
    }
}
