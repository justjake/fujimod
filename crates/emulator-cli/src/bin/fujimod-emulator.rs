//! Native boot command; every binary input is selected by path.
use fujimod_emulator_boot::{self as boot, Options};
use fujimod_emulator_cpu::ExecutionPolicy;
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf, process::ExitCode};

const HELP: &str = "Usage: fujimod-emulator boot --configuration ID \\\n  --update UPDATE.DAT --boot-rom ROM.BIN [--camera DIR] \\\n  [--overlay ID] [--nand IMAGE] [--settings MCU.BIN] [--screenshot LCD.PNG] \\\n  [--instruction-limit COUNT] [--execution-policy interactive|exact]\n\n\
Boot configuration: x70-110-rec. The ROM dump is required.\n\
x70-110-rec starts from the reference camera's settings bank with setup done;\n\
--camera names its directory of nand-<section>.bin.zst sections\n\
(catalog/bodies/x70/cameras/reference).\n\
A run that declares no settings takes --nand or --settings instead.\n\
NAND accepts a committed image or 128 MiB main-area image without OOB/spare.\n\
Settings accepts a sixteen-byte MCU persistent packet.\n\
First-start menus are accepted through MENU/OK ADC input.\n\
Prints a JSON boot report; exits 0 at REC, 2 at a stopped frontier.\n";

fn parse(
    args: impl Iterator<Item = OsString>,
) -> Result<Option<(Options, Option<PathBuf>, u64, ExecutionPolicy)>, String> {
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
    let mut values = BTreeMap::new();
    let mut overlays = Vec::new();
    let mut preferred_languages = Vec::new();
    while let Some(key) = args.next() {
        let key = key.to_str().ok_or("option name is not UTF-8")?.to_owned();
        if ![
            "--configuration",
            "--overlay",
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
        ]
        .contains(&key.as_str())
        {
            return Err(format!("unknown option: {key}"));
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {key}"))?;
        if key == "--language" {
            preferred_languages.push(
                value
                    .into_string()
                    .map_err(|_| "language tag must be UTF-8")?,
            );
            continue;
        }
        if key == "--overlay" {
            let id = value
                .into_string()
                .map_err(|_| "overlay ID must be UTF-8")?;
            if overlays.contains(&id) {
                return Err(format!("duplicate overlay: {id}"));
            }
            overlays.push(id);
            continue;
        }
        if values.insert(key.clone(), value).is_some() {
            return Err(format!("duplicate option: {key}"));
        }
    }
    let clock_seconds_2000 = values
        .remove("--clock-seconds-2000")
        .map(|v| {
            v.to_str()
                .ok_or("clock seconds must be UTF-8")?
                .parse::<u32>()
                .map_err(|_| "clock seconds must be an unsigned 32-bit integer")
        })
        .transpose()?;
    let mut take = |key: &str| {
        values
            .remove(key)
            .ok_or_else(|| format!("required option: {key}"))
    };
    let options = Options {
        configuration_id: take("--configuration")?
            .into_string()
            .map_err(|_| "configuration ID must be UTF-8")?,
        overlays,
        update: take("--update")?.into(),
        boot_rom: take("--boot-rom")?.into(),
        nand: None,
        settings: None,
        clock_seconds_2000: None,
        preferred_languages,
        camera: None,
    };
    let options = Options {
        nand: values.remove("--nand").map(PathBuf::from),
        settings: values.remove("--settings").map(PathBuf::from),
        clock_seconds_2000,
        camera: values.remove("--camera").map(PathBuf::from),
        ..options
    };
    let screenshot = values.remove("--screenshot").map(PathBuf::from);
    let limit = values
        .remove("--instruction-limit")
        .map(|v| {
            v.to_str()
                .ok_or("instruction limit must be UTF-8")?
                .parse::<u64>()
                .map_err(|_| "instruction limit must be an unsigned integer")
        })
        .transpose()?
        .unwrap_or(2_000_000_000);
    if limit == 0 {
        return Err("instruction limit must be positive".into());
    }
    let execution_policy = values
        .remove("--execution-policy")
        .map(|value| {
            value
                .to_str()
                .ok_or_else(|| "execution policy must be UTF-8".to_owned())
                .and_then(ExecutionPolicy::parse)
        })
        .transpose()?
        .unwrap_or(ExecutionPolicy::Interactive);
    Ok(Some((options, screenshot, limit, execution_policy)))
}
fn run() -> Result<bool, String> {
    let Some((options, path, limit, execution_policy)) = parse(std::env::args_os().skip(1))? else {
        print!("{HELP}");
        return Ok(true);
    };
    let mut session = boot::open(options)?;
    session.board.set_execution_policy(execution_policy)?;
    let report = boot::to_rec(&mut session, limit)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
    );
    if let Some(path) = path {
        std::fs::write(&path, boot::screenshot(&session)?)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(report.status == "rec")
}
fn main() -> ExitCode {
    fujimod_emulator_exr2_board::register_board_builder();
    fujimod_emulator_models::register();
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(2),
        Err(error) => {
            eprintln!("{error}");
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
    fn explicit_inputs_and_unique_options_are_required() {
        assert!(parse(args(&["--help"])).unwrap().is_none());
        assert!(parse(args(&["boot", "--help"])).unwrap().is_none());
        assert!(parse(args(&["boot", "--configuration", "x70-110-rec"])).is_err());
        assert!(
            parse(args(&[
                "boot",
                "--configuration",
                "x70-110-rec",
                "--configuration",
                "x70-110-rec"
            ]))
            .is_err()
        );
        assert!(
            parse(args(&[
                "boot",
                "--configuration",
                "x70-110-rec",
                "--update",
                "a.dat",
                "--boot-rom",
                "rom.bin",
                "--instruction-limit",
                "0"
            ]))
            .is_err()
        );
        let (options, _, limit, execution_policy) = parse(args(&[
            "boot",
            "--configuration",
            "x70-110-rec",
            "--update",
            "a.dat",
            "--boot-rom",
            "rom.bin",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(options.configuration_id, "x70-110-rec");
        assert!(options.overlays.is_empty());
        assert_eq!(options.boot_rom, PathBuf::from("rom.bin"));
        assert_eq!(limit, 2_000_000_000);
        assert_eq!(execution_policy, ExecutionPolicy::Interactive);
        assert!(options.nand.is_none());
        assert!(parse(args(&["boot", "--body", "x70", "--firmware", "x70-110"])).is_err());
    }

    #[test]
    fn execution_policy_accepts_interactive_and_exact_and_rejects_unknown_values() {
        let base = [
            "boot",
            "--configuration",
            "x70-110-rec",
            "--update",
            "a.dat",
            "--boot-rom",
            "rom.bin",
        ];
        for (value, expected) in [
            ("interactive", ExecutionPolicy::Interactive),
            ("exact", ExecutionPolicy::Exact),
        ] {
            let mut input = base.to_vec();
            input.extend(["--execution-policy", value]);
            let (_, _, _, policy) = parse(args(&input)).unwrap().unwrap();
            assert_eq!(policy, expected);
        }

        let mut input = base.to_vec();
        input.extend(["--execution-policy", "approximate"]);
        assert_eq!(
            parse(args(&input)).unwrap_err(),
            "unknown CPU execution policy: approximate"
        );
    }

    #[test]
    fn overlay_ids_preserve_order_and_duplicates_refuse() {
        let base = [
            "boot",
            "--configuration",
            "x70-110-rec",
            "--update",
            "a.dat",
            "--boot-rom",
            "rom.bin",
        ];
        let mut input = base.to_vec();
        input.extend(["--overlay", "usb", "--overlay", "service"]);
        let (options, _, _, _) = parse(args(&input)).unwrap().unwrap();
        assert_eq!(options.overlays, ["usb", "service"]);
        input.extend(["--overlay", "usb"]);
        assert!(parse(args(&input)).is_err());
    }
}
