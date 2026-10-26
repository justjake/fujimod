use std::path::PathBuf;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("emscripten") {
        let directory = std::env::var("CAMERA_UNICORN_STATIC_DIR")
            .expect("CAMERA_UNICORN_STATIC_DIR must name the Emscripten Unicorn build");
        println!("cargo:rustc-link-search=native={directory}");
        for name in ["unicorn", "arm-softmmu", "unicorn-common"] {
            println!("cargo:rustc-link-lib=static={name}");
            println!("cargo:rerun-if-changed={directory}/lib{name}.a");
        }
        println!("cargo:rerun-if-env-changed=CAMERA_UNICORN_STATIC_DIR");
        return;
    }
    let source = PathBuf::from("../vendor/unicorn");
    if !source.join("CMakeLists.txt").is_file() {
        panic!("the pinned Unicorn source submodule is not initialized");
    }

    let mut config = cmake::Config::new(source);
    config
        // A compiler-path change makes CMake discard its cache and rerun
        // configuration without our -D options. Start fresh so the selected
        // architectures survive toolchain changes in a reused Cargo directory.
        .configure_arg("--fresh")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("UNICORN_LEGACY_STATIC_ARCHIVE", "ON")
        .define("UNICORN_ARCH", "arm;aarch64")
        .define("UNICORN_TCG_INTERPRETER", "OFF")
        .define("UNICORN_BUILD_TESTS", "OFF")
        .define("UNICORN_INSTALL", "OFF")
        .define("UNICORN_FUZZ", "OFF")
        .build_target("unicorn_archive");
    let output = config.build();
    let cache = std::fs::read_to_string(output.join("build/CMakeCache.txt"))
        .expect("read configured Unicorn architecture");
    assert!(
        cache.lines().any(|line| {
            line.starts_with("UNICORN_ARCH:")
                && line
                    .split_once('=')
                    .is_some_and(|(_, value)| value == "arm;aarch64")
        }),
        "Unicorn must be configured for ARM and AArch64"
    );

    println!(
        "cargo:rustc-link-search=native={}",
        output.join("build").display()
    );
    println!("cargo:rustc-link-lib=static=unicorn");
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env != "msvc" {
        println!("cargo:rustc-link-lib=m");
        println!("cargo:rustc-link-lib=pthread");
    }
    println!("cargo:rerun-if-changed=../vendor/unicorn");
}
