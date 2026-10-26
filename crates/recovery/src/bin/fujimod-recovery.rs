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
                return Err("--success-hold-seconds requires startup or stage-one".into());
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
        Some(command @ ("build" | "startup" | "card-startup" | "stage-one" | "rescue-baseline")) => {
            let mut inputs = args[1..].to_vec();
            let storage = if command == "card-startup" {
                if inputs.len() != 4 || inputs.iter().any(|v| v.starts_with("--")) {
                    return Err("Expected FIRMWARE DAT STORAGE_IMAGE NEW_DIRECTORY".into());
                }
                Some(fs::read(inputs.remove(2))?)
            } else { None };
            let (firmware, dat, output, success_hold_seconds) =
                construction_args(&inputs, matches!(command, "startup" | "stage-one"))?;
            let options = StartupOptions {
                success_hold_seconds,
            };
            let recipe = Recipe::bundled(firmware)?.ok_or("Missing recovery recipe")?;
            let source = fs::read(dat)?;
            let files = if let Some(storage) = storage {
                recipe.build_startup_with_storage(&source, &storage, options)?
            } else if command == "build" {
                recipe.build(&source)?
            } else if command == "rescue-baseline" {
                recipe.build_rescue_baseline(&source)?
            } else if command == "stage-one" {
                recipe.build_stage_one_with_options(&source, options)?
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
                "Usage: fujimod-recovery list | build|startup|stage-one|rescue-baseline FIRMWARE DAT NEW_DIRECTORY [--success-hold-seconds N] | card-startup FIRMWARE DAT STORAGE_IMAGE NEW_DIRECTORY"
                    .into(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::construction_args;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
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
