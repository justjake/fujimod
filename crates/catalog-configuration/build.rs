use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn json_files(root: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            json_files(&path, files);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            files.push(path);
        }
    }
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let root = manifest.join("../..");
    let configurations = root.join("configurations/emulator");
    println!("cargo:rerun-if-changed={}", configurations.display());
    let mut configuration_files = Vec::new();
    json_files(&configurations, &mut configuration_files);
    configuration_files.sort();
    let mut configuration_source =
        String::from("const BUNDLED_RUN_CONFIGURATIONS: &[(&str, &str)] = &[\n");
    let mut overlay_source = String::from("const BUNDLED_RUN_OVERLAYS: &[(&str, &str)] = &[\n");    for path in configuration_files {
        let Some(relative) = path.strip_prefix(&root).ok() else {
            continue;
        };
        let Some(parent) = path.parent() else {
            continue;
        };
        let include = relative.to_string_lossy().replace('\\', "/");
        if path.file_name().is_some_and(|name| name == "run.json") {
            let Some(id) = parent.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            configuration_source.push_str(&format!(
                "    ({id:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../../{include}\"))),\n"
            ));
            println!("cargo:rerun-if-changed={}", path.display());
        } else if parent.file_name().is_some_and(|name| name == "overlays") {
            let Some(id) = path.file_stem().and_then(|name| name.to_str()) else {
                continue;
            };
            overlay_source.push_str(&format!(
                "    ({id:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../../{include}\"))),\n"
            ));
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    configuration_source.push_str("];\n");
    overlay_source.push_str("];\n");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("output directory"))
        .join("bundled_configurations.rs");
    fs::write(
        &output,
        configuration_source + &overlay_source,
    )
    .expect("write embedded run-configuration index");
}
