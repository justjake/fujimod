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
    let mut files = Vec::new();
    json_files(&configurations, &mut files);
    files.sort();
    let mut source = String::from("const BUNDLED_HARDWARE_INPUTS: &[(&str, &str)] = &[\n");
    for path in files {
        let Some(relative) = path.strip_prefix(&root).ok() else {
            continue;
        };
        let Some(parent) = path.parent() else {
            continue;
        };
        let include = relative.to_string_lossy().replace('\\', "/");
        if path
            .file_name()
            .is_some_and(|name| name == "hardware-inputs.json")
        {
            let Some(id) = parent.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            source.push_str(&format!(
                "    ({id:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../../{include}\"))),\n"
            ));
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    source.push_str("];\n");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("output directory"))
        .join("bundled_hardware_inputs.rs");
    fs::write(&output, source).expect("write embedded hardware-input index");
}
