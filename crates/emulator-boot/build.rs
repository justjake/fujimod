//! Bundle the per-firmware boot manifests and stream layouts under `data/`.
use std::{env, fmt::Write, fs, path::Path};

fn main() {
    if env::var_os("CARGO_FEATURE_XP5_STARTUP").is_none()
        || env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32")
    {
        return;
    }
    let data = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("xp5-data");
    println!("cargo:rerun-if-changed=xp5-data");
    let mut ids: Vec<_> = fs::read_dir(&data)
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().into_string().unwrap())
        .collect();
    ids.sort();
    let mut source = String::from("pub(crate) static BUNDLED: &[Bundled] = &[\n");
    for id in ids {
        let directory = data.join(&id);
        let mut fragments: Vec<_> = fs::read_dir(directory.join("manifest.d"))
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        fragments.sort();
        let mut list = String::new();
        for fragment in fragments {
            write!(list, "include_str!({:?}), ", fragment.display().to_string()).unwrap();
        }
        writeln!(
            source,
            "    Bundled {{ firmware: {id:?}, manifest: include_str!({:?}), fragments: &[{list}], streams: include_str!({:?}) }},",
            directory.join("manifest.toml").display().to_string(),
            directory.join("streams.json").display().to_string(),
        )
        .unwrap();
    }
    source.push_str("];\n");
    fs::write(Path::new(&env::var("OUT_DIR").unwrap()).join("bundled.rs"), source).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
