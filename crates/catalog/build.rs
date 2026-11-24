use std::env;
use std::fs;
use std::io::Write;
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
        ("ip", "Ip"),
        ("soc", "Soc"),
        ("parts", "Part"),
        ("bodies", "Body"),
        ("boot-roms", "BootRom"),
        ("firmware", "Firmware"),
    ];
    let mut files = Vec::new();
    for (directory, _) in layers {
        let path = catalog.join(directory);
        println!("cargo:rerun-if-changed={}", path.display());
        json_files(&path, &mut files);
    }
    files.sort();
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("output directory"));
    let browser = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("emscripten");
    // Stream the JSON records from one compressed frame in the browser build.
    let mut packed = browser.then(|| {
        let file = fs::File::create(out.join("bundled_catalog.zst")).expect("create packed catalog");
        zstd::stream::write::Encoder::new(file, 9).expect("start packed catalog")
    });
    let document_type = if browser { "usize" } else { "&str" };
    let mut source = format!("const BUNDLED_CATALOG: &[(ReferenceKind, &str, {document_type})] = &[\n");
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
            ("Firmware", "ui-scene-names.generated.json") => "UiSceneNames",
            ("Firmware", "ui-setting-names.generated.json") => "UiSettingNames",
            ("Firmware", "ui-event-codes.generated.json") => "UiEventCodes",
            ("Firmware", "bulk.generated.json") => "Bulk",
            ("Firmware", "sd-boot-v2-autostart.generated.json") => "NandBoot",
            ("Firmware", "boot-patch.generated.json") => "BootPatch",
            ("Firmware", "rom-start.generated.json") => "RomStart",
            ("BootRom", name) if name.ends_with(".nand-boot.generated.json") => "NandBootRom",
            ("BootRom", _) => "BootRom",
            ("Firmware", "card-files.generated.json") => "CardFiles",
            ("Firmware", "execution.generated.json") => "Execution",
            ("Firmware", "restart.generated.json") => "Restart",
            ("Firmware", "memory-reservations.generated.json") => "MemoryReservations",
            ("Firmware", "settings.generated.json") => "Settings",
            ("Firmware", "flash.generated.json") => "Flash",
            ("Firmware", "state.generated.json") => "State",
            ("Body", "body.json") | ("Firmware", "firmware.json") => kind,
            ("Body" | "Firmware", _) => panic!("unknown catalog document {}", path.display()),
            ("Soc", "soc.json") => kind,
            ("Soc", _)
                if path
                    .parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str())
                    == Some("blocks") =>
            {
                "SocBlock"
            }
            // Generated evidence beside a processor's records is not a
            // reference document; readers take it from disk directly.
            ("Soc", _) => continue,
            _ => kind,
        };
        let relative = path
            .strip_prefix(&root)
            .expect("catalog file is under root");
        let include = relative.to_string_lossy().replace('\\', "/");
        let label = relative.to_string_lossy();
        let document = if let Some(packed) = &mut packed {
            let bytes = fs::read(&path).expect("read catalog document");
            packed.write_all(&bytes).expect("compress catalog document");
            bytes.len().to_string()
        } else {
            format!("include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../../{include}\"))")
        };
        source.push_str(&format!("    (ReferenceKind::{kind}, {label:?}, {document}),\n"));
        println!("cargo:rerun-if-changed={}", path.display());
    }
    source.push_str("];\n");
    if let Some(packed) = packed {
        packed.finish().expect("finish packed catalog");
    }
    fs::write(out.join("bundled_catalog.rs"), source).expect("write embedded catalog index");
}
