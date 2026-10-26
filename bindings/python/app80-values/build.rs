fn main() {
    let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .ancestors()
        .nth(3)
        .unwrap()
        .to_path_buf();
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    fujimod_bindgen::Api::build(&root, &out);
    println!("cargo:rerun-if-changed=build.rs");
}
