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
    let catalog = root.join("catalog");
    let layers = [
        ("soc", "Soc"),
        ("parts", "Part"),
        ("bodies", "Body"),
        ("firmware", "Firmware"),
    ];
    let mut files = Vec::new();
    for (directory, _) in layers {
        let path = catalog.join(directory);
        println!("cargo:rerun-if-changed={}", path.display());
        json_files(&path, &mut files);
    }
    files.sort();
    let mut source = String::from("const BUNDLED_CATALOG: &[(ReferenceKind, &str, &str)] = &[\n");
    for path in files {
        let Some(layer) = path.strip_prefix(&catalog).ok().and_then(|relative| {
            relative
                .components()
                .next()
                .and_then(|component| component.as_os_str().to_str())
        }) else {
            continue;
        };
        let Some((_, kind)) = layers.iter().find(|(directory, _)| *directory == layer) else {
            continue;
        };
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let kind = match (*kind, name) {
            ("Body", "controls.json") => "PhysicalControls",
            ("Firmware", "control-ids.generated.json") => "ControlIds",
            ("Firmware", "first-start.generated.json") => "FirstStart",
            ("Firmware", "bulk.generated.json") => "Bulk",
            ("Firmware", "settings.generated.json") => "Settings",
            ("Firmware", "flash.generated.json") => "Flash",
            ("Firmware", "state.generated.json") => "State",
            ("Body", "body.json") | ("Firmware", "firmware.json") => kind,
            ("Body" | "Firmware", _) => panic!("unknown catalog document {}", path.display()),
            _ => kind,
        };
        let relative = path
            .strip_prefix(&root)
            .expect("catalog file is under root");
        let include = relative.to_string_lossy().replace('\\', "/");
        let label = relative.to_string_lossy();
        source.push_str(&format!(
            "    (ReferenceKind::{kind}, {label:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../../{include}\"))),\n"
        ));
        println!("cargo:rerun-if-changed={}", path.display());
    }
    source.push_str("];\n");
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("output directory")).join("bundled_catalog.rs");
    fs::write(output, source).expect("write embedded catalog index");
}
