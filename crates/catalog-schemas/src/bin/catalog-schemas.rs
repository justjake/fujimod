use fujimod_catalog_schemas::generated_documents;
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let mode = env::args().nth(1).unwrap_or_else(|| "--check".into());
    if mode != "--check" && mode != "--write" {
        eprintln!(
            "usage: cargo run -p fujimod-catalog-schemas --features schema --bin catalog-schemas -- [--check|--write]"
        );
        std::process::exit(2);
    }
    let package = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("package directory"));
    let directory = package.join("../../catalog/schemas");
    if mode == "--write" {
        fs::create_dir_all(&directory).expect("create schema directory");
    }

    let mut failed = false;
    for (name, schema) in generated_documents() {
        let path = directory.join(name);
        let mut generated = serde_json::to_vec_pretty(&schema).expect("serialize schema");
        generated.push(b'\n');
        if mode == "--write" {
            fs::write(&path, generated)
                .unwrap_or_else(|error| panic!("could not write {}: {error}", path.display()));
            println!("wrote {}", path.display());
        } else if fs::read(&path).ok().as_deref() != Some(generated.as_slice()) {
            eprintln!("generated schema differs: {}", path.display());
            failed = true;
        }
    }
    if failed {
        eprintln!("run catalog-schemas -- --write and commit the resulting schemas");
        std::process::exit(1);
    }
}
