use std::{
    env, fs,
    path::{Path, PathBuf},
};
fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let path = root.join("../../catalog/recovery/recipes.generated.json");
    println!("cargo:rerun-if-changed={}", path.display());
    let bytes = fs::read(&path).expect("bundled recovery recipes");
    fs::write(out.join("recipes.generated.json"), bytes).unwrap();

    // The XP5 SD loaders are prebuilt binary assets; their layouts become constants. The default
    // build is the card-resident loader; the internal-flash build serves a startup that keeps
    // the installed flash attached.
    loader(
        &root,
        &out,
        "sd-boot-v1-xp5-loader",
        "xp5_sd_loader.rs",
        &[],
    );
    loader(
        &root,
        &out,
        "sd-boot-v1-xp5-internal-flash-loader",
        "xp5_internal_flash_loader.rs",
        &["setup_sd_internal_flash"],
    );
}

fn loader(root: &Path, out: &Path, stem: &str, output: &str, extra_symbols: &[&str]) {
    let image = root.join(format!("assets/{stem}.generated.bin"));
    let layout = root.join(format!("assets/{stem}.generated.json"));
    println!("cargo:rerun-if-changed={}", image.display());
    println!("cargo:rerun-if-changed={}", layout.display());
    let layout: serde_json::Value =
        serde_json::from_slice(&fs::read(&layout).expect("XP5 SD loader layout")).unwrap();
    assert_eq!(layout["schema"], 1, "XP5 SD loader layout schema");
    let image_bytes = fs::metadata(&image).expect("XP5 SD loader image").len();
    assert_eq!(
        layout["image_bytes"].as_u64(),
        Some(image_bytes),
        "XP5 SD loader image size"
    );
    let mut rust = format!(
        "pub const IMAGE: &[u8] = include_bytes!({:?});
pub const PRELUDE_STACK_MAX: usize = {};
",
        image.display().to_string(),
        layout["prelude_stack_max"].as_u64().unwrap()
    );
    let symbols = layout["symbols"].as_object().unwrap();
    for symbol in extra_symbols {
        assert!(symbols.contains_key(*symbol), "XP5 SD loader lacks {symbol}");
    }
    for (name, offset) in symbols {
        rust += &format!(
            "pub const {}: usize = {:#x};
",
            name.to_uppercase(),
            offset.as_u64().unwrap()
        );
    }
    rust += "pub const EXTERNALS: &[(usize, u32, &str)] = &[
";
    for external in layout["externals"].as_array().unwrap() {
        rust += &format!(
            "    ({:#x}, {}, {:?}),
",
            external["offset"].as_u64().unwrap(),
            external["kind"].as_u64().unwrap(),
            external["symbol"].as_str().unwrap()
        );
    }
    rust += "];
";
    fs::write(out.join(output), rust).unwrap();
}
