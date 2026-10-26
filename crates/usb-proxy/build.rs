use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

/// Browser entry points. Each is bundled with everything it imports into one
/// self-contained module in web/build/: wasm-bindgen copies only the file a
/// `#[wasm_bindgen(module)]` names, and the relay serves one script.
const ENTRIES: [&str; 2] = ["peer", "exporter"];

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let web = manifest.join("web");
    println!("cargo:rerun-if-changed=web/tsconfig.json");
    for entry in std::fs::read_dir(&web).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "ts") {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    let wasm = env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32");
    let relay = env::var_os("CARGO_FEATURE_RELAY").is_some();
    if !wasm && !relay {
        return;
    }
    for entry in ENTRIES {
        println!("cargo:rerun-if-changed=web/build/{entry}.js");
    }
    let modules = manifest.join("../../node_modules");
    println!(
        "cargo:rerun-if-changed={}",
        modules.join("typescript/package.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        modules.join("rolldown/package.json").display()
    );
    node(
        &modules.join("typescript/bin/tsc"),
        &[
            "--project",
            &web.join("tsconfig.json").display().to_string(),
            "--noEmit",
        ],
        "USB browser TypeScript check",
    );
    for entry in ENTRIES {
        node(
            &modules.join("rolldown/bin/cli.mjs"),
            &[
                &web.join(format!("{entry}.ts")).display().to_string(),
                "--format",
                "esm",
                "--file",
                &web.join(format!("build/{entry}.js")).display().to_string(),
            ],
            "USB browser bundle",
        );
    }
}

fn node(script: &Path, args: &[&str], what: &str) {
    assert!(
        script.is_file(),
        "{what} requires {}; run npm ci at the repository root",
        script.display()
    );
    let status =
        Command::new("node").arg(script).args(args).status().expect(
            "USB browser assets require Node.js; install the toolchain declared in mise.toml",
        );
    assert!(status.success(), "{what} failed");
}
