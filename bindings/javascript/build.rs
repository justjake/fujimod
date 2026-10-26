fn main() {
    let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    fujimod_bindgen::Api::build(&root, &out);
    println!(
        "cargo:rerun-if-changed={}",
        root.join("catalog/schemas").display()
    );
    fujimod_bindgen::generate_catalog_typescript(
        &root.join("catalog/schemas"),
        &out.join("catalog-types.generated.ts"),
    )
    .expect("catalog TypeScript generation");
}
