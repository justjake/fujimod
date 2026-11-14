//! Offline construction and catalog inspection. No device access.
use fujimod_recovery::{Recipe, StartupOptions, availability};
use std::{env, fs, path::PathBuf};
fn construction_args(
    args: &[String],
    allow_hold: bool,
) -> Result<(&str, &str, &str, u32), Box<dyn std::error::Error>> {
    let mut positional = Vec::new();
    let mut hold = None;
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
        } else if arg.starts_with("--") {
            return Err(format!("Unknown construction option: {arg}").into());
        } else {
            positional.push(arg.as_str());
        }
    }
    if positional.len() != 3 {
        return Err("Expected FIRMWARE DAT NEW_DIRECTORY".into());
    }
    Ok((
        positional[0],
        positional[1],
        positional[2],
        hold.unwrap_or(0),
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
            | "sd-boot-v1-ram-startup" | "sd-card-recovery-baseline" | "sd-boot-v2-sd-card"),
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
            let (firmware, dat, output, success_hold_seconds) =
                construction_args(&inputs, matches!(command, "sd-boot-v1-startup" | "sd-boot-v1-stage-one" | "sd-boot-v1-ram-startup"))?;
            let options = StartupOptions {
                success_hold_seconds,
                internal_flash: command == "sd-boot-v1-ram-startup",
                fallback_wait_seconds,
            };
            let recipe = Recipe::bundled(firmware)?.ok_or("Missing SD card recipe")?;
            let source = fs::read(dat)?;
            let files = if let Some(storage) = storage {
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
            } else if command == "sd-boot-v1-ram-startup" {
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
                "Usage: fujimod-sd-card list | sd-card-recovery-build|green-led|sd-boot-v1-startup|sd-boot-v1-stage-one|sd-boot-v1-ram-startup|sd-card-recovery-baseline|sd-boot-v2-sd-card FIRMWARE DAT NEW_DIRECTORY [--success-hold-seconds N] [--fallback-wait-seconds N (sd-boot-v1-ram-startup)] | sd-boot-v1-sd-card-startup FIRMWARE DAT STORAGE_IMAGE NEW_DIRECTORY"
                    .into(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{construction_args, take_fallback_wait};
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
    fn success_hold_defaults_to_zero_and_accepts_either_position() {
        assert_eq!(
            construction_args(&args(&["camera", "update", "output"]), true).unwrap(),
            ("camera", "update", "output", 0)
        );
        for values in [
            args(&["camera", "update", "output", "--success-hold-seconds", "3"]),
            args(&["--success-hold-seconds", "3", "camera", "update", "output"]),
        ] {
            assert_eq!(
                construction_args(&values, true).unwrap(),
                ("camera", "update", "output", 3)
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
}
