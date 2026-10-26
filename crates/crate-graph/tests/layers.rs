//! The crate layers that keep a device model edit from rebuilding the
//! emulator, its bindings or unrelated models (`crates/emulator/CRATES.md`).
//!
//! - Every workspace crate has a layer in `layers.json`.
//! - Below the final layer, only model crates depend on model crates.
//! - A model crate has at most `model_line_budget` lines outside its tests.
//! - The interface crate implements no device.
//!
//! Remaining violations are listed in `layers.json`. An entry that is no
//! longer a violation fails too, so the list only shrinks.
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Layers {
    model_line_budget: usize,
    layers: BTreeMap<String, Vec<String>>,
    remaining: Remaining,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Remaining {
    model_dependencies: BTreeMap<String, BTreeSet<String>>,
    oversize_models: BTreeSet<String>,
    interface_devices: BTreeSet<String>,
    /// PyO3 libraries whose unit tests cannot link a native test harness.
    untested_libraries: BTreeSet<String>,
}

struct Package {
    root: PathBuf,
    dependencies: BTreeSet<String>,
}

const LAYERS: [&str; 8] = [
    "interface",
    "model",
    "plan",
    "board",
    "engine",
    "library",
    "final",
    "other",
];

fn layers() -> Layers {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("layers.json");
    serde_json::from_slice(&std::fs::read(&path).unwrap()).expect("layers.json")
}

/// Workspace packages and their normal and build dependencies on other
/// workspace packages. Dev-dependencies are free: tests may link models.
fn packages() -> BTreeMap<String, Package> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo metadata");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let packages = metadata["packages"].as_array().unwrap();
    let names: BTreeSet<_> = packages
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    packages
        .iter()
        .map(|p| {
            let manifest = PathBuf::from(p["manifest_path"].as_str().unwrap());
            let dependencies = p["dependencies"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|d| d["kind"].as_str() != Some("dev"))
                .map(|d| d["name"].as_str().unwrap())
                .filter(|name| names.contains(name))
                .map(str::to_owned)
                .collect();
            (
                p["name"].as_str().unwrap().to_owned(),
                Package {
                    root: manifest.parent().unwrap().to_owned(),
                    dependencies,
                },
            )
        })
        .collect()
}

fn layer_of(layers: &Layers) -> BTreeMap<&str, &str> {
    let mut of = BTreeMap::new();
    for (layer, crates) in &layers.layers {
        assert!(LAYERS.contains(&layer.as_str()), "unknown layer {layer}");
        for name in crates {
            assert!(
                of.insert(name.as_str(), layer.as_str()).is_none(),
                "{name} has two layers"
            );
        }
    }
    of
}

/// Source files outside tests: no `tests` directories, `tests.rs` or
/// `*_tests.rs` files, and no trailing inline `#[cfg(test)] mod … { … }`.
fn source(root: &Path) -> Vec<(PathBuf, String)> {
    fn walk(directory: &Path, files: &mut Vec<(PathBuf, String)>) {
        let mut entries: Vec<_> = std::fs::read_dir(directory)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                if name != "tests" {
                    walk(&path, files);
                }
            } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                let lines: Vec<&str> = text.lines().collect();
                let end = lines
                    .windows(2)
                    .position(|w| {
                        w[0].trim() == "#[cfg(test)]"
                            && w[1].starts_with("mod ")
                            && w[1].trim_end().ends_with('{')
                    })
                    .unwrap_or(lines.len());
                files.push((path, lines[..end].join("\n")));
            }
        }
    }
    let mut files = Vec::new();
    walk(&root.join("src"), &mut files);
    files
}

#[test]
fn every_workspace_crate_has_one_layer() {
    let layers = layers();
    let of = layer_of(&layers);
    let packages = packages();
    let missing: Vec<_> = packages
        .keys()
        .filter(|name| !of.contains_key(name.as_str()))
        .collect();
    let stale: Vec<_> = of
        .keys()
        .filter(|name| !packages.contains_key(**name))
        .collect();
    assert!(
        missing.is_empty(),
        "add these crates to crates/crate-graph/layers.json: {missing:?}"
    );
    assert!(
        stale.is_empty(),
        "remove these crates from crates/crate-graph/layers.json: {stale:?}"
    );
}

#[test]
fn only_models_and_final_crates_depend_on_models() {
    let layers = layers();
    let of = layer_of(&layers);
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (name, package) in packages() {
        let layer = of.get(name.as_str()).copied().unwrap_or("other");
        if matches!(layer, "model" | "final") {
            continue;
        }
        for dependency in &package.dependencies {
            if of.get(dependency.as_str()) == Some(&"model") {
                found
                    .entry(name.clone())
                    .or_default()
                    .insert(dependency.clone());
            }
        }
    }
    let remaining = &layers.remaining.model_dependencies;
    let new: BTreeMap<_, _> = found
        .iter()
        .map(|(name, deps)| {
            (
                name,
                deps.difference(remaining.get(name).unwrap_or(&BTreeSet::new()))
                    .cloned()
                    .collect::<BTreeSet<_>>(),
            )
        })
        .filter(|(_, deps)| !deps.is_empty())
        .collect();
    let gone: BTreeMap<_, _> = remaining
        .iter()
        .map(|(name, deps)| {
            (
                name,
                deps.difference(found.get(name).unwrap_or(&BTreeSet::new()))
                    .cloned()
                    .collect::<BTreeSet<_>>(),
            )
        })
        .filter(|(_, deps)| !deps.is_empty())
        .collect();
    assert!(
        new.is_empty(),
        "these crates below the final layer now depend on model crates; reach models through \
         the interface, a registered builder or a final crate instead: {new:?}"
    );
    assert!(
        gone.is_empty(),
        "remove these resolved entries from remaining.model_dependencies: {gone:?}"
    );
}

#[test]
fn model_crates_stay_within_the_line_budget() {
    let layers = layers();
    let packages = packages();
    let mut over = BTreeMap::new();
    for name in &layers.layers["model"] {
        let lines: usize = source(&packages[name].root)
            .iter()
            .map(|(_, text)| text.lines().count())
            .sum();
        if lines > layers.model_line_budget {
            over.insert(name.clone(), lines);
        }
    }
    let new: BTreeMap<_, _> = over
        .iter()
        .filter(|(name, _)| !layers.remaining.oversize_models.contains(*name))
        .collect();
    let gone: Vec<_> = layers
        .remaining
        .oversize_models
        .iter()
        .filter(|name| !over.contains_key(*name))
        .collect();
    assert!(
        new.is_empty(),
        "split these model crates by subsystem; each may have at most {} lines outside tests: {new:?}",
        layers.model_line_budget
    );
    assert!(
        gone.is_empty(),
        "remove these split crates from remaining.oversize_models: {gone:?}"
    );
}

#[test]
fn the_interface_implements_no_device() {
    let layers = layers();
    let packages = packages();
    let device = |text: &str| {
        text.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with("impl") && line.contains("Device for ")
        })
    };
    let mut found = BTreeSet::new();
    for name in &layers.layers["interface"] {
        let root = &packages[name].root;
        for (path, text) in source(root) {
            if device(&text) {
                found.insert(format!(
                    "{name}/{}",
                    path.strip_prefix(root).unwrap().display()
                ));
            }
        }
    }
    let new: Vec<_> = found
        .difference(&layers.remaining.interface_devices)
        .collect();
    let gone: Vec<_> = layers
        .remaining
        .interface_devices
        .difference(&found)
        .collect();
    assert!(
        new.is_empty(),
        "move these device implementations into model crates: {new:?}"
    );
    assert!(
        gone.is_empty(),
        "remove these moved files from remaining.interface_devices: {gone:?}"
    );
}

/// Every `.rs` file under `directory`, skipping `bin` directories, which
/// belong to binary targets.
fn library_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().unwrap() != "bin" {
                library_files(&path, files);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
}

/// `cargo test --workspace` compiles and links a test harness for every
/// library whose manifest leaves `test` on, so a library without unit tests
/// sets `test = false`. A library that sets it may not contain tests, which
/// would otherwise never run.
#[test]
fn libraries_build_a_test_harness_exactly_when_they_have_unit_tests() {
    let untested = layers().remaining.untested_libraries;
    let mut skipped = BTreeSet::new();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo metadata");
    assert!(output.status.success());
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let mut wrong = vec![];
    for package in metadata["packages"].as_array().unwrap() {
        for target in package["targets"].as_array().unwrap() {
            let kinds = target["kind"].as_array().unwrap();
            if !kinds.iter().any(|k| {
                matches!(
                    k.as_str(),
                    Some("lib" | "rlib" | "cdylib" | "staticlib" | "proc-macro")
                )
            }) {
                continue;
            }
            let mut files = vec![];
            library_files(
                Path::new(target["src_path"].as_str().unwrap())
                    .parent()
                    .unwrap(),
                &mut files,
            );
            let tests = files.iter().any(|path| {
                let text = std::fs::read_to_string(path).unwrap();
                ["#[test]", "#[tokio::test", "#[wasm_bindgen_test"]
                    .iter()
                    .any(|marker| text.contains(marker))
            });
            let name = package["name"].as_str().unwrap();
            if tests && untested.contains(name) {
                skipped.insert(name.to_owned());
                continue;
            }
            if target["test"].as_bool().unwrap() != tests {
                wrong.push(format!(
                    "{name}: test = {}, unit tests: {tests}",
                    target["test"]
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "set [lib] test to match:\n{}",
        wrong.join("\n")
    );
    assert_eq!(
        skipped, untested,
        "untested_libraries lists only libraries with skipped tests"
    );
}
