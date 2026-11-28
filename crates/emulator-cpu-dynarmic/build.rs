fn main() {
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if !matches!(arch.as_str(), "x86_64" | "aarch64") || !matches!(os.as_str(), "linux" | "macos") {
        return;
    }

    assert!(
        std::path::Path::new("vendor/dynarmic/externals/fmt/CMakeLists.txt").is_file(),
        "initialize the pinned Dynarmic source and its dependencies with git submodule update --init --recursive"
    );
    let output = cmake::Config::new(".")
        .generator("Ninja")
        .profile("Release")
        .build_target("dynarmic_ffi")
        .build();

    let libraries = output.join("build/lib");
    println!("cargo:rustc-link-search=native={}", libraries.display());
    for archive in ["dynarmic_ffi", "dynarmic", "mcl", "fmt", "Zydis", "Zycore"] {
        if libraries.join(format!("lib{archive}.a")).is_file() {
            println!("cargo:rustc-link-lib=static={archive}");
        }
    }
    println!(
        "cargo:rustc-link-lib={}",
        if os == "macos" { "c++" } else { "stdc++" }
    );
    for path in ["CMakeLists.txt", "csrc", "vendor/dynarmic"] {
        println!("cargo:rerun-if-changed={path}");
    }
}
