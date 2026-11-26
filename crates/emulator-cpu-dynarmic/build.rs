use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let vendor_dir = manifest_dir.join("vendor/dynarmic");
    let csrc_dir = manifest_dir.join("csrc");
    let lib_dir = manifest_dir.join("lib");

    cc::Build::new()
        .cpp(true)
        .std("c++20")
        .file(csrc_dir.join("dynarmic_ffi.cpp"))
        .include(&csrc_dir)
        .include(vendor_dir.join("src"))
        .include(vendor_dir.join("externals/fmt/include"))
        .include(vendor_dir.join("externals/mcl/include"))
        .include(vendor_dir.join("externals/oaknut/include"))
        .include("/opt/homebrew/include")
        .compile("dynarmic_ffi");

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=dynarmic");
    println!("cargo:rustc-link-lib=static=mcl");
    println!("cargo:rustc-link-lib=static=fmt");
    println!("cargo:rustc-link-lib=c++");

    println!("cargo:rerun-if-changed=csrc/dynarmic_ffi.h");
    println!("cargo:rerun-if-changed=csrc/dynarmic_ffi.cpp");
}
