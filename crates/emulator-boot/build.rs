//! Bundle native XP5 startup facts alongside each firmware catalog record.
use std::{env, fmt::Write, fs, path::Path};

fn directories(path: &Path) -> Vec<std::path::PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    paths.sort();
    paths
}

fn main() {
    if env::var_os("CARGO_FEATURE_XP5_STARTUP").is_none()
        || env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32")
    {
        return;
    }
    let catalog = Path::new("../../catalog/firmware");
    println!("cargo:rerun-if-changed={}", catalog.display());
    let mut source = String::from("pub(crate) static BUNDLED: &[Bundled] = &[\n");
    for body in directories(catalog) {
        for version in directories(&body) {
            let directory = version.join("xp5-boot");
            if !directory.is_dir() {
                continue;
            }
            let directory = directory.canonicalize().unwrap();
            let manifest = directory.join("manifest.toml");
            let manifest_text = fs::read_to_string(&manifest).unwrap();
            let id = manifest_text.lines()
                .find_map(|line| line.strip_prefix("firmware = \"").and_then(|id| id.strip_suffix('"')))
                .expect("XP5 manifest must declare its firmware ID");
            writeln!(
                source,
                "    Bundled {{ firmware: {id:?}, manifest: include_str!({:?}), streams: include_str!({:?}) }},",
                manifest.display().to_string(),
                directory.join("streams.toml").display().to_string(),
            )
            .unwrap();
        }
    }
    source.push_str("];\n");
    fs::write(Path::new(&env::var("OUT_DIR").unwrap()).join("bundled.rs"), source).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
