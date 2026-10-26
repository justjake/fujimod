use jsonschema::validator_for;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn json_files(directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).expect("catalog directory") {
        let path = entry.expect("catalog entry").path();
        if path.is_dir() {
            files.extend(json_files(&path));
        } else if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            files.push(path);
        }
    }
    files.sort();
    files
}

#[test]
fn reference_and_run_configuration_documents_validate_generated_schemas() {
    let package = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let catalog = package.join("../../catalog");
    let schema_root = catalog.join("schemas");

    for (name, generated) in fujimod_catalog_schemas::generated_documents() {
        let path = schema_root.join(name);
        let mut expected = serde_json::to_vec_pretty(&generated).expect("generated schema");
        expected.push(b'\n');
        let actual = fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        assert!(
            actual == expected,
            "{} differs from the Rust source types",
            path.display()
        );
    }

    let mut files = vec![catalog.join("firmware-versions.generated.json")];
    for layer in ["soc", "parts", "bodies", "firmware", "progress"] {
        files.extend(json_files(&catalog.join(layer)));
    }
    files.extend(json_files(&package.join("../../configurations/emulator")));

    for path in files {
        let document: Value = serde_json::from_slice(&fs::read(&path).expect("catalog document"))
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let schema_reference = document
            .get("$schema")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{}: missing $schema", path.display()));
        let schema_name = schema_reference
            .strip_prefix("https://fujimod.com/schemas/")
            .unwrap_or_else(|| {
                panic!(
                    "{}: schema must use a stable schema identity",
                    path.display()
                )
            });
        assert!(
            schema_name.ends_with(".schema.json") && !schema_name.contains('/'),
            "{}: schema identity is not a named generated schema",
            path.display()
        );
        let schema_path = schema_root.join(schema_name);
        let schema: Value = serde_json::from_slice(
            &fs::read(&schema_path)
                .unwrap_or_else(|error| panic!("{}: {error}", schema_path.display())),
        )
        .unwrap_or_else(|error| panic!("{}: {error}", schema_path.display()));
        let validator = validator_for(&schema)
            .unwrap_or_else(|error| panic!("{}: {error}", schema_path.display()));
        let errors: Vec<_> = validator.iter_errors(&document).collect();
        assert!(
            errors.is_empty(),
            "{}: {}",
            path.display(),
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
}

#[test]
fn bundled_reference_records_resolve_by_identity() {
    let catalog = fujimod_catalog::ReferenceCatalog::bundled().expect("bundled references");
    for (body, firmware) in [
        ("x100s", "x100s-121"),
        ("x100t", "x100t-111"),
        ("x20", "x20-103"),
        ("x30", "x30-103"),
        ("xe2", "xe2-411"),
        ("xe2s", "xe2s-111"),
        ("xm1", "xm1-150"),
        ("xq", "xq1-121"),
        ("xq", "xq2-101"),
        ("xt1", "xt1-552"),
        ("xt10", "xt10-131"),
    ] {
        catalog
            .resolve(body, firmware)
            .unwrap_or_else(|error| panic!("{body} / {firmware}: {error}"));
    }
}

#[test]
fn firmware_versions_load_without_execution_profiles_and_agree_with_located_bulk_identities() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../catalog");
    let disk = fujimod_catalog::ReferenceCatalog::load(&root).unwrap();
    let bundled = fujimod_catalog::ReferenceCatalog::bundled().unwrap();
    assert_eq!(disk.firmware_versions(), bundled.firmware_versions());
    assert!(
        bundled
            .firmware_versions()
            .iter()
            .any(|v| v.firmware_id == "xh1-215")
    );
    for version in bundled.firmware_versions() {
        if let Some(firmware) = bundled.firmware(&version.firmware_id) {
            assert_eq!(version.version, firmware.version);
            assert_eq!(
                version.update_sha256,
                firmware.update.as_ref().unwrap().sha256
            );
            if let Some(bulk) = bundled.resolve_firmware(&version.firmware_id).unwrap().bulk {
                assert_eq!(version.product, bulk.product);
                assert_eq!(version.internal_version, bulk.internal_version);
            }
        }
    }
}

#[test]
fn controls_only_directories_validate_without_hardware_records() {
    let package = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = package.join("../../catalog/bodies/xa10/controls.json");
    let document: Value = serde_json::from_slice(&fs::read(source).unwrap()).unwrap();
    let root = std::env::temp_dir().join(format!(
        "catalog-manual-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let directory = root.join("bodies/xa10");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("controls.json");
    fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    let catalog = fujimod_catalog::ReferenceCatalog::load(&root).unwrap();
    assert!(catalog.body("xa10").is_none());
    let controls = fujimod_catalog_controls::PhysicalControls::decode_document(
        &serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap(),
        "xa10",
    )
    .unwrap();
    assert_eq!(controls.analog_levels, None);
    assert!(
        controls
            .controls
            .values()
            .all(|control| !control.mechanical)
    );
    assert!(controls.controls["shutter_button"].pushable);
    for kind in ["connection", "binding", "manual"] {
        let mut invalid = document.clone();
        match kind {
            "connection" => {
                invalid["controls"]["shutter_button"]["connection_ids"] =
                    serde_json::json!(["absent"])
            }
            "binding" => {
                invalid["controls"]["shutter_button"]["behavior"]["stages"]["half_press"] =
                    serde_json::json!([{"kind":"debug_key","name":"UNLOCATED"}])
            }
            _ => invalid["manual"]["pages"] = serde_json::json!([]),
        }
        fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        if kind == "manual" {
            let controls = fujimod_catalog_controls::PhysicalControls::decode_document(
                &invalid,
                &path.display().to_string(),
            )
            .unwrap();
            assert!(controls.validate().is_err(), "{kind}");
        } else {
            assert!(
                fujimod_catalog::ReferenceCatalog::load(&root).is_err(),
                "{kind}"
            );
        }
    }
    fs::remove_dir_all(root).unwrap();
}
