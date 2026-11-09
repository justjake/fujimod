use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

/// QEMU's configure, which Unicorn's CMake runs, rejects source and build
/// directories containing spaces or colons and word-splits the compiler path.
fn breaks_configure(path: &Path) -> bool {
    path.to_str()
        .is_none_or(|path| path.contains(|c: char| c.is_whitespace() || c == ':'))
}

#[cfg(unix)]
fn link(target: &Path, alias: &Path) {
    let _ = std::fs::remove_file(alias);
    std::os::unix::fs::symlink(target, alias).expect("link Unicorn build alias");
}

#[cfg(windows)]
fn link(target: &Path, alias: &Path) {
    let _ = std::fs::remove_dir(alias);
    let _ = std::fs::remove_file(alias);
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, alias)
    } else {
        std::os::windows::fs::symlink_file(target, alias)
    }
    .expect("link Unicorn build alias");
}

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
    let source = std::env::current_dir().unwrap().join("../vendor/unicorn");
    if !source.join("CMakeLists.txt").is_file() {
        panic!("the pinned Unicorn source submodule is not initialized");
    }

    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let compiler = cc::Build::new().get_compiler().path().to_path_buf();
    let compiler = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join(&compiler))
        .find(|path| path.is_file())
        .unwrap_or(compiler);
    let mut config;
    if [&source, &out, &compiler].into_iter().any(|path| breaks_configure(path)) {
        let mut hasher = DefaultHasher::new();
        out.hash(&mut hasher);
        let alias = std::env::temp_dir()
            .join(format!("fujimod-unicorn-{:016x}", hasher.finish()));
        assert!(!breaks_configure(&alias), "no space-free directory for the Unicorn build");
        std::fs::create_dir_all(&alias).expect("create Unicorn build alias");
        link(&source, &alias.join("source"));
        link(&compiler, &alias.join("cc"));
        config = cmake::Config::new(alias.join("source"));
        config.out_dir(&alias).define("CMAKE_C_COMPILER", alias.join("cc"));
        // The system may clear the temporary directory between builds.
        println!("cargo:rerun-if-changed={}", alias.join("build/libunicorn.a").display());
    } else {
        config = cmake::Config::new(&source);
    }
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
