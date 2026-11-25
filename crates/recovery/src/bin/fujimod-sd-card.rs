//! Offline construction and catalog inspection. No device access.
use fujimod_recovery::ram_patch::RamPatches;
use fujimod_recovery::{CheckedRedirect, Recipe, StartupOptions, StartupStop, availability};
use std::{env, fs, path::PathBuf};
fn construction_args(
    args: &[String],
    allow_hold: bool,
) -> Result<(&str, &str, &str, StartupOptions), Box<dyn std::error::Error>> {
    let mut positional = Vec::new();
    let mut hold = None;
    let mut units = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--success-hold-seconds" {
            if !allow_hold {
                return Err("--success-hold-seconds requires sd-boot-v1-startup or sd-boot-v1-stage-one".into());
            }
            if hold.is_some() {
                return Err("--success-hold-seconds was specified twice".into());
            }
            let value = args.next().ok_or("Missing --success-hold-seconds value")?;
            hold =
                Some(value.parse::<u32>().map_err(
                    |_| "--success-hold-seconds requires a non-negative 32-bit integer",
                )?);
        } else if arg == "--success-hold-units" {
            if !allow_hold {
                return Err("--success-hold-units requires sd-boot-v1-startup or sd-boot-v1-stage-one".into());
            }
            if units.is_some() {
                return Err("--success-hold-units was specified twice".into());
            }
            let value = args.next().ok_or("Missing --success-hold-units value")?;
            units = Some(
                value
                    .parse::<u64>()
                    .map_err(|_| "--success-hold-units requires a non-negative 64-bit integer")?,
            );
        } else if arg.starts_with("--") {
            return Err(format!("Unknown construction option: {arg}").into());
        } else {
            positional.push(arg.as_str());
        }
    }
    if positional.len() != 3 {
        return Err("Expected FIRMWARE DAT NEW_DIRECTORY".into());
    }
    if hold.is_some() && units.is_some() {
        return Err("--success-hold-seconds and --success-hold-units are mutually exclusive".into());
    }
    Ok((
        positional[0],
        positional[1],
        positional[2],
        StartupOptions {
            success_hold_seconds: hold.unwrap_or(0),
            success_hold_units: units,
            ..StartupOptions::default()
        },
    ))
}

/// Remove `--fallback-wait-seconds N` from the arguments (only `sd-boot-v1-ram-startup` takes it).
fn take_fallback_wait(
    args: &mut Vec<String>,
    allowed: bool,
) -> Result<Option<u32>, Box<dyn std::error::Error>> {
    let Some(at) = args.iter().position(|arg| arg == "--fallback-wait-seconds") else {
        return Ok(None);
    };
    if !allowed {
        return Err("--fallback-wait-seconds requires sd-boot-v1-ram-startup".into());
    }
    let value = args
        .get(at + 1)
        .ok_or("Missing --fallback-wait-seconds value")?
        .parse::<u32>()
        .map_err(|_| "--fallback-wait-seconds requires a non-negative 32-bit integer")?;
    args.drain(at..=at + 1);
    if args.iter().any(|arg| arg == "--fallback-wait-seconds") {
        return Err("--fallback-wait-seconds was specified twice".into());
    }
    Ok(Some(value))
}

/// Remove `--destination ADDRESS` from the arguments (the read-only and checked-redirect startups
/// require it, no other command takes it). ADDRESS is decimal or `0x` hexadecimal.
fn take_destination(
    args: &mut Vec<String>,
    allowed: bool,
) -> Result<Option<u64>, Box<dyn std::error::Error>> {
    let Some(at) = args.iter().position(|arg| arg == "--destination") else {
        return if allowed {
            Err("sd-boot-v1-read-only-startup, sd-boot-v1-checked-redirect-startup and sd-boot-v1-copy-stub-startup require --destination ADDRESS".into())
        } else {
            Ok(None)
        };
    };
    if !allowed {
        return Err("--destination requires sd-boot-v1-read-only-startup, sd-boot-v1-checked-redirect-startup or sd-boot-v1-copy-stub-startup".into());
    }
    let text = args.get(at + 1).ok_or("Missing --destination value")?;
    let value = match text.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => text.parse::<u64>(),
    }
    .map_err(|_| "--destination requires an address, decimal or 0x hexadecimal")?;
    args.drain(at..=at + 1);
    if args.iter().any(|arg| arg == "--destination") {
        return Err("--destination was specified twice".into());
    }
    Ok(Some(value))
}

/// Remove `--verify-only` or `--apply` (exactly one, required by `sd-boot-v1-checked-redirect-startup`,
/// taken by no other command).
fn take_redirect_mode(
    args: &mut Vec<String>,
    required: bool,
) -> Result<Option<CheckedRedirect>, Box<dyn std::error::Error>> {
    let flags = ["--verify-only", "--apply"];
    let given: Vec<&str> = flags.into_iter().filter(|f| args.iter().any(|a| a == f)).collect();
    if !required {
        return if given.is_empty() {
            Ok(None)
        } else {
            Err("--verify-only and --apply require sd-boot-v1-checked-redirect-startup or sd-boot-v1-copy-stub-startup".into())
        };
    }
    let count = args.iter().filter(|a| flags.contains(&a.as_str())).count();
    if given.len() != 1 || count != 1 {
        return Err("sd-boot-v1-checked-redirect-startup and sd-boot-v1-copy-stub-startup require exactly one of --verify-only and --apply".into());
    }
    args.retain(|a| !flags.contains(&a.as_str()));
    Ok(Some(if given[0] == "--apply" { CheckedRedirect::Apply } else { CheckedRedirect::VerifyOnly }))
}

/// Remove `--ram-overlay FILE` from the arguments (only the XP5 startup commands take it).
fn take_ram_overlay(
    args: &mut Vec<String>,
    allowed: bool,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    let Some(at) = args.iter().position(|arg| arg == "--ram-overlay") else {
        return Ok(None);
    };
    if !allowed {
        return Err("--ram-overlay requires sd-boot-v1-startup or sd-boot-v1-ram-startup".into());
    }
    let value = args.get(at + 1).ok_or("Missing --ram-overlay value")?.clone();
    args.drain(at..=at + 1);
    if args.iter().any(|arg| arg == "--ram-overlay") {
        return Err("--ram-overlay was specified twice".into());
    }
    Ok(Some(PathBuf::from(value)))
}

/// Remove `--ram-patches FILE` from the arguments (only `sd-boot-v1-startup` takes it).
fn take_ram_patches(
    args: &mut Vec<String>,
    allowed: bool,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    let Some(at) = args.iter().position(|arg| arg == "--ram-patches") else {
        return Ok(None);
    };
    if !allowed {
        return Err("--ram-patches requires sd-boot-v1-startup".into());
    }
    let value = args
        .get(at + 1)
        .ok_or("Missing --ram-patches file")?
        .clone();
    args.drain(at..=at + 1);
    if args.iter().any(|arg| arg == "--ram-patches") {
        return Err("--ram-patches was specified twice".into());
    }
    Ok(Some(PathBuf::from(value)))
}

/// Remove `--prepare-installed-flash` from the arguments (only `sd-boot-v1-startup` and the XP5
/// diagnostic checks take it).
fn take_prepare_installed_flash(
    args: &mut Vec<String>,
    allowed: bool,
) -> Result<bool, Box<dyn std::error::Error>> {
    let flag = "--prepare-installed-flash";
    match args.iter().filter(|arg| *arg == flag).count() {
        0 => Ok(false),
        _ if !allowed => Err(format!(
            "{flag} requires sd-boot-v1-startup or {}",
            CHECKS.join(", ")
        )
        .into()),
        1 => {
            args.retain(|arg| arg != flag);
            Ok(true)
        }
        _ => Err(format!("{flag} was specified twice").into()),
    }
}

/// The XP5 diagnostic checks: the card startup, stopped where the command names.
const CHECKS: [&str; 5] = [
    "sd-boot-v1-card-init-check",
    "sd-boot-v1-card-error-check",
    "sd-boot-v1-setup-check",
    "sd-boot-v1-read-check",
    "sd-boot-v1-card-read-check",
];

fn startup_stop(command: &str) -> StartupStop {
    match command {
        "sd-boot-v1-card-init-check" => StartupStop::AfterCardInit,
        "sd-boot-v1-card-error-check" => StartupStop::AfterCardInitWithErrorBlink,
        "sd-boot-v1-setup-check" => StartupStop::AfterSetup,
        "sd-boot-v1-read-check" => StartupStop::AfterCrc,
        "sd-boot-v1-card-read-check" => StartupStop::AfterCrcWithErrorPackets,
        _ => StartupStop::Continue,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("list") => {
            let recipes: Vec<Recipe> = serde_json::from_str(include_str!(concat!(
                env!("OUT_DIR"),
                "/recipes.generated.json"
            )))?;
            let rows = recipes
                .iter()
                .map(|r| availability(&r.firmware_id))
                .collect::<Result<Vec<_>, _>>()?;
            println!("{}", serde_json::to_string_pretty(&rows)?);
        }
        Some(
            command @ ("sd-card-recovery-build" | "green-led" | "sd-boot-v1-startup" | "sd-boot-v1-sd-card-startup" | "sd-boot-v1-stage-one"
            | "sd-boot-v1-card-init-check" | "sd-boot-v1-card-error-check" | "sd-boot-v1-setup-check" | "sd-boot-v1-read-check" | "sd-boot-v1-card-read-check"
            | "sd-boot-v1-ram-startup" | "sd-boot-v1-read-only-startup" | "sd-boot-v1-checked-redirect-startup" | "sd-boot-v1-copy-stub-startup" | "sd-boot-v1-marker-startup" | "sd-boot-v1-os-startup" | "sd-boot-v1-exif-startup" | "sd-boot-v1-hook-startup" | "sd-card-recovery-baseline" | "sd-boot-v2-sd-card"),
        ) => {
            let mut inputs = args[1..].to_vec();
            let storage = if command == "sd-boot-v1-sd-card-startup" {
                if inputs.len() != 4 || inputs.iter().any(|v| v.starts_with("--")) {
                    return Err("Expected FIRMWARE DAT STORAGE_IMAGE NEW_DIRECTORY".into());
                }
                Some(fs::read(inputs.remove(2))?)
            } else {
                None
            };
            let fallback_wait_seconds =
                take_fallback_wait(&mut inputs, command == "sd-boot-v1-ram-startup")?;
            let marker_command = command == "sd-boot-v1-marker-startup";
            let os_command = command == "sd-boot-v1-os-startup";
            let exif_command = command == "sd-boot-v1-exif-startup";
            let hook_command = command == "sd-boot-v1-hook-startup";
            let stub_command = command == "sd-boot-v1-copy-stub-startup" || marker_command || os_command || exif_command || hook_command;
            let redirect_command = command == "sd-boot-v1-checked-redirect-startup" || stub_command;
            let checked_redirect = take_redirect_mode(&mut inputs, redirect_command)?;
            let read_only_destination = take_destination(
                &mut inputs,
                matches!(command, "sd-boot-v1-read-only-startup" | "sd-boot-v1-checked-redirect-startup" | "sd-boot-v1-copy-stub-startup" | "sd-boot-v1-marker-startup" | "sd-boot-v1-os-startup" | "sd-boot-v1-exif-startup" | "sd-boot-v1-hook-startup"),
            )?;
            let overlay = take_ram_overlay(
                &mut inputs,
                matches!(command, "sd-boot-v1-startup" | "sd-boot-v1-ram-startup"),
            )?
            .map(|path| fujimod_recovery::image::Overlay::read(&path))
            .transpose()?;
            let ram_patches = take_ram_patches(&mut inputs, command == "sd-boot-v1-startup")?
                .map(|path| RamPatches::read(&path))
                .transpose()?;
            if overlay.is_some() && ram_patches.is_some() {
                return Err("--ram-overlay and --ram-patches are mutually exclusive".into());
            }
            let prepare_installed_flash = take_prepare_installed_flash(
                &mut inputs,
                command == "sd-boot-v1-startup" || CHECKS.contains(&command),
            )?;
            let (firmware, dat, output, hold) =
                construction_args(&inputs, matches!(command, "sd-boot-v1-startup" | "sd-boot-v1-stage-one" | "sd-boot-v1-ram-startup" | "sd-boot-v1-read-only-startup" | "sd-boot-v1-checked-redirect-startup" | "sd-boot-v1-copy-stub-startup" | "sd-boot-v1-marker-startup" | "sd-boot-v1-os-startup" | "sd-boot-v1-exif-startup" | "sd-boot-v1-hook-startup"))?;
            let options = StartupOptions {
                internal_flash: command == "sd-boot-v1-ram-startup",
                fallback_wait_seconds,
                read_only_destination,
                checked_redirect,
                copy_stub: stub_command,
                marker: marker_command,
                os: os_command,
                exif: exif_command,
                hook: hook_command,
                stop: startup_stop(command),
                prepare_installed_flash,
                ..hold
            };
            let recipe = Recipe::bundled(firmware)?.ok_or("Missing SD card recipe")?;
            let source = fs::read(dat)?;
            let files = if let Some(overlay) = &overlay {
                recipe.build_startup_with_overlay(&source, options, overlay)?
            } else if let Some(patches) = &ram_patches {
                recipe.build_startup_with_ram_patches(&source, options, patches)?
            } else if let Some(storage) = storage {
                recipe.build_startup_with_storage(&source, &storage, options)?
            } else if command == "sd-card-recovery-build" {
                recipe.build(&source)?
            } else if command == "green-led" {
                fujimod_recovery::green_led::build(&recipe, &source)?
            } else if command == "sd-card-recovery-baseline" {
                recipe.build_rescue_baseline(&source)?
            } else if command == "sd-boot-v2-sd-card" {
                recipe.build_ota_startup(&source)?
            } else if command == "sd-boot-v1-stage-one" {
                recipe.build_stage_one_with_options(&source, options)?
            } else if matches!(command, "sd-boot-v1-ram-startup" | "sd-boot-v1-read-only-startup" | "sd-boot-v1-checked-redirect-startup" | "sd-boot-v1-copy-stub-startup" | "sd-boot-v1-marker-startup" | "sd-boot-v1-os-startup" | "sd-boot-v1-exif-startup" | "sd-boot-v1-hook-startup") {
                // The update stays an independent input and is not written.
                recipe.build_boot_files(&source, options)?
            } else {
                recipe.build_startup_with_options(&source, options)?
            };
            let output = PathBuf::from(output);
            fs::create_dir(&output)?;
            let mut rows = Vec::new();
            for file in files {
                fs::write(output.join(&file.name), &file.bytes)?;
                rows.push(serde_json::json!({"name":file.name,"size":file.bytes.len(),"sha256":fujimod_firmware::sha256(&file.bytes)}));
            }
            println!("{}", serde_json::to_string_pretty(&rows)?);
        }
        _ => {
            return Err(
                "Usage: fujimod-sd-card list | sd-card-recovery-build|green-led|sd-boot-v1-startup|sd-boot-v1-stage-one|sd-boot-v1-ram-startup|sd-boot-v1-read-only-startup|sd-boot-v1-checked-redirect-startup|sd-boot-v1-copy-stub-startup|sd-boot-v1-marker-startup|sd-boot-v1-os-startup|sd-boot-v1-exif-startup|sd-boot-v1-hook-startup|sd-card-recovery-baseline|sd-boot-v2-sd-card|sd-boot-v1-card-init-check|sd-boot-v1-card-error-check|sd-boot-v1-setup-check|sd-boot-v1-read-check|sd-boot-v1-card-read-check FIRMWARE DAT NEW_DIRECTORY [--success-hold-seconds N | --success-hold-units N] [--fallback-wait-seconds N (sd-boot-v1-ram-startup)] [--destination ADDRESS (sd-boot-v1-read-only-startup, sd-boot-v1-checked-redirect-startup and sd-boot-v1-copy-stub-startup, required)] [--verify-only | --apply (sd-boot-v1-checked-redirect-startup, sd-boot-v1-copy-stub-startup, sd-boot-v1-marker-startup, sd-boot-v1-os-startup and sd-boot-v1-exif-startup, one required)] [--ram-overlay FILE (sd-boot-v1-startup, sd-boot-v1-ram-startup)] [--ram-patches FILE (sd-boot-v1-startup)] [--prepare-installed-flash (sd-boot-v1-startup and the checks)] | sd-boot-v1-sd-card-startup FIRMWARE DAT STORAGE_IMAGE NEW_DIRECTORY"
                    .into(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        construction_args, startup_stop, take_destination, take_fallback_wait,
        take_prepare_installed_flash, take_ram_overlay, take_ram_patches,
    };
    use fujimod_recovery::{StartupOptions, StartupStop};
    fn seconds(n: u32) -> StartupOptions {
        StartupOptions {
            success_hold_seconds: n,
            ..StartupOptions::default()
        }
    }
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn fallback_wait_is_taken_out_before_the_positional_arguments() {
        let mut values = args(&["--fallback-wait-seconds", "30", "camera", "update", "output"]);
        assert_eq!(take_fallback_wait(&mut values, true).unwrap(), Some(30));
        assert_eq!(values, args(&["camera", "update", "output"]));
        let mut values = args(&["camera", "update", "output"]);
        assert_eq!(take_fallback_wait(&mut values, true).unwrap(), None);
        assert_eq!(take_fallback_wait(&mut values, false).unwrap(), None);
        for bad in [
            args(&["camera", "--fallback-wait-seconds"]),
            args(&["camera", "--fallback-wait-seconds", "-1"]),
            args(&["camera", "--fallback-wait-seconds", "x"]),
            args(&["--fallback-wait-seconds", "1", "--fallback-wait-seconds", "2"]),
        ] {
            assert!(take_fallback_wait(&mut bad.clone(), true).is_err(), "{bad:?}");
        }
        assert!(
            take_fallback_wait(&mut args(&["--fallback-wait-seconds", "1"]), false).is_err()
        );
    }
    #[test]
    fn the_redirect_mode_is_exactly_one_flag_and_only_for_its_command() {
        use super::take_redirect_mode;
        use fujimod_recovery::CheckedRedirect::{Apply, VerifyOnly};
        let mut values = args(&["--apply", "camera"]);
        assert_eq!(take_redirect_mode(&mut values, true).unwrap(), Some(Apply));
        assert_eq!(values, args(&["camera"]));
        assert_eq!(take_redirect_mode(&mut args(&["camera", "--verify-only"]), true).unwrap(), Some(VerifyOnly));
        assert_eq!(take_redirect_mode(&mut args(&["camera"]), false).unwrap(), None);
        for bad in [args(&["camera"]), args(&["--apply", "--verify-only"]), args(&["--apply", "--apply"])] {
            assert!(take_redirect_mode(&mut bad.clone(), true).is_err(), "{bad:?}");
        }
        assert!(take_redirect_mode(&mut args(&["--apply"]), false).is_err());
    }
    #[test]
    fn destination_is_required_by_the_read_only_command_and_by_no_other() {
        let mut values = args(&["--destination", "0x30000000", "camera", "update", "output"]);
        assert_eq!(take_destination(&mut values, true).unwrap(), Some(0x3000_0000));
        assert_eq!(values, args(&["camera", "update", "output"]));
        let mut values = args(&["camera", "update", "output", "--destination", "805306368"]);
        assert_eq!(take_destination(&mut values, true).unwrap(), Some(0x3000_0000));
        assert_eq!(values, args(&["camera", "update", "output"]));
        assert_eq!(take_destination(&mut args(&["camera", "update", "output"]), false).unwrap(), None);
        assert!(take_destination(&mut args(&["camera", "update", "output"]), true).is_err());
        assert!(take_destination(&mut args(&["--destination", "0x2000"]), false).is_err());
        for bad in [
            args(&["camera", "--destination"]),
            args(&["camera", "--destination", "-1"]),
            args(&["camera", "--destination", "0xzz"]),
            args(&["camera", "--destination", "x"]),
            args(&["--destination", "0x1000", "--destination", "0x2000"]),
        ] {
            assert!(take_destination(&mut bad.clone(), true).is_err(), "{bad:?}");
        }
    }
    #[test]
    fn success_hold_defaults_to_zero_and_accepts_either_position() {
        assert_eq!(
            construction_args(&args(&["camera", "update", "output"]), true).unwrap(),
            ("camera", "update", "output", seconds(0))
        );
        for values in [
            args(&["camera", "update", "output", "--success-hold-seconds", "3"]),
            args(&["--success-hold-seconds", "3", "camera", "update", "output"]),
        ] {
            assert_eq!(
                construction_args(&values, true).unwrap(),
                ("camera", "update", "output", seconds(3))
            );
        }
    }
    #[test]
    fn invalid_or_unsupported_holds_refuse_before_reading_files() {
        for value in ["-1", "1.5", "text", "4294967296"] {
            assert!(
                construction_args(
                    &args(&[
                        "camera",
                        "update",
                        "output",
                        "--success-hold-seconds",
                        value
                    ]),
                    true
                )
                .is_err()
            );
        }
        assert!(
            construction_args(
                &args(&["camera", "update", "output", "--success-hold-seconds"]),
                true
            )
            .is_err()
        );
        assert!(
            construction_args(
                &args(&[
                    "camera",
                    "update",
                    "output",
                    "--success-hold-seconds",
                    "3",
                    "--success-hold-seconds",
                    "0"
                ]),
                true
            )
            .is_err()
        );
        assert!(
            construction_args(
                &args(&["camera", "update", "output", "--success-hold-seconds", "3"]),
                false
            )
            .is_err()
        );
    }
    #[test]
    fn success_hold_units_are_explicit_and_exclusive_with_seconds() {
        let units = |n: u64| StartupOptions {
            success_hold_units: Some(n),
            ..StartupOptions::default()
        };
        for values in [
            args(&["camera", "update", "output", "--success-hold-units", "32"]),
            args(&["--success-hold-units", "32", "camera", "update", "output"]),
        ] {
            assert_eq!(
                construction_args(&values, true).unwrap(),
                ("camera", "update", "output", units(32))
            );
        }
        assert_eq!(units(32).hold_units().unwrap(), 32);
        assert_eq!(seconds(3).hold_units().unwrap(), 6_144);
        assert_eq!(StartupOptions::default().hold_units().unwrap(), 0);
        let refused = [
            // both options, either order
            vec!["--success-hold-seconds", "3", "--success-hold-units", "32"],
            vec!["--success-hold-units", "32", "--success-hold-seconds", "3"],
            // repeated, malformed, missing
            vec!["--success-hold-units", "1", "--success-hold-units", "2"],
            vec!["--success-hold-units", "-1"],
            vec!["--success-hold-units", "1.5"],
            vec!["--success-hold-units", "18446744073709551616"],
            vec!["--success-hold-units"],
        ];
        for extra in refused {
            let mut values = args(&["camera", "update", "output"]);
            values.extend(extra.iter().map(|s| s.to_string()));
            assert!(construction_args(&values, true).is_err(), "{extra:?}");
        }
        // Refused for the commands that take no hold (the recovery build).
        assert!(
            construction_args(
                &args(&["camera", "update", "output", "--success-hold-units", "32"]),
                false
            )
            .is_err()
        );
        // The library refuses both settings at once.
        let both = StartupOptions {
            success_hold_seconds: 3,
            success_hold_units: Some(32),
            ..StartupOptions::default()
        };
        assert!(both.hold_units().is_err());
    }

    #[test]
    fn ram_overlay_is_taken_out_only_for_the_xp5_startup_commands() {
        let mut values = args(&["camera", "--ram-overlay", "overlay.json", "update", "output"]);
        assert_eq!(
            take_ram_overlay(&mut values, true).unwrap(),
            Some(std::path::PathBuf::from("overlay.json"))
        );
        assert_eq!(values, args(&["camera", "update", "output"]));
        assert_eq!(take_ram_overlay(&mut values, false).unwrap(), None);
        for (bad, allowed) in [
            (args(&["camera", "--ram-overlay", "x"]), false),
            (args(&["camera", "--ram-overlay"]), true),
            (args(&["--ram-overlay", "a", "--ram-overlay", "b"]), true),
        ] {
            assert!(take_ram_overlay(&mut bad.clone(), allowed).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn card_read_check_selects_stop_five_while_read_check_stays_stop_two() {
        assert_eq!(
            startup_stop("sd-boot-v1-card-read-check"),
            StartupStop::AfterCrcWithErrorPackets
        );
        assert_eq!(startup_stop("sd-boot-v1-card-read-check") as u64, 5);
        assert_eq!(startup_stop("sd-boot-v1-read-check"), StartupStop::AfterCrc);
        assert_eq!(startup_stop("sd-boot-v1-read-check") as u64, 2);
        assert_eq!(startup_stop("sd-boot-v1-startup"), StartupStop::Continue);
    }

    #[test]
    fn installed_flash_option_is_explicit_and_only_allowed_for_full_xp5_builds() {
        let mut values = args(&["camera", "update", "output", "--prepare-installed-flash"]);
        assert!(take_prepare_installed_flash(&mut values, true).unwrap());
        assert_eq!(values, args(&["camera", "update", "output"]));
        assert!(!take_prepare_installed_flash(&mut values, true).unwrap());
        assert!(!take_prepare_installed_flash(&mut values, false).unwrap());
        assert!(
            take_prepare_installed_flash(
                &mut args(&["camera", "--prepare-installed-flash"]),
                false
            )
            .is_err()
        );
        let mut duplicate = args(&[
            "camera",
            "--prepare-installed-flash",
            "--prepare-installed-flash",
        ]);
        assert!(take_prepare_installed_flash(&mut duplicate, true).is_err());
    }

    #[test]
    fn ram_patches_are_only_accepted_for_startup_and_once() {
        let mut values = args(&[
            "camera",
            "update",
            "output",
            "--ram-patches",
            "patches.json",
        ]);
        assert_eq!(
            take_ram_patches(&mut values, true).unwrap(),
            Some(std::path::PathBuf::from("patches.json"))
        );
        assert_eq!(values, args(&["camera", "update", "output"]));
        assert_eq!(take_ram_patches(&mut values, false).unwrap(), None);
        for (bad, allowed) in [
            (args(&["camera", "--ram-patches", "patches.json"]), false),
            (args(&["camera", "--ram-patches"]), true),
            (
                args(&["--ram-patches", "a.json", "--ram-patches", "b.json"]),
                true,
            ),
        ] {
            assert!(
                take_ram_patches(&mut bad.clone(), allowed).is_err(),
                "{bad:?}"
            );
        }
    }
}
