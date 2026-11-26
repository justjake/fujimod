use std::{env, ffi::OsString, fs, path::PathBuf, process::Command};
fn run(command: &mut Command) {
    let result = command.output().expect("run payload tool");
    assert!(
        result.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}
/// The clang that compiles the ARM payloads: `CLANG`, else the first `clang`
/// on PATH whose registered targets include ARM, which is the pinned toolchain
/// under mise on every host. A PATH can put a WebAssembly-only clang
/// (Emscripten's) first.
fn arm_clang() -> OsString {
    if let Some(clang) = env::var_os("CLANG").filter(|clang| !clang.is_empty()) {
        return clang;
    }
    println!("cargo:rerun-if-env-changed=PATH");
    let path = env::var_os("PATH").unwrap_or_default();
    env::split_paths(&path)
        .map(|directory| directory.join("clang"))
        .find(|clang| {
            Command::new(clang)
                .arg("-print-targets")
                .output()
                .is_ok_and(|output| {
                    String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .any(|line| line.trim_start().starts_with("arm "))
                })
        })
        .map(OsString::from)
        .expect("no clang on PATH targets ARM; set CLANG")
}

fn main() {
    println!("cargo:rerun-if-env-changed=CLANG");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let script = out.join("payload.ld");
    fs::write(&script,"SECTIONS { . = 0; .text : { *(.text.entry) *(.text*) } .rodata : { *(.rodata*) } .data : { *(.data*) *(.bss*) } /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) *(.comment*) *(.note*) *(.eh_frame*) } }").unwrap();
    let clang = arm_clang();
    for name in ["call", "card"] {
        let source = format!("payload/{name}.c");
        println!("cargo:rerun-if-changed={source}");
        let object = out.join(format!("{name}.o"));
        let image = out.join(format!("{name}.bin"));
        run(Command::new(&clang)
            .args([
                "--target=armv7-none-eabi",
                "-mcpu=cortex-a9",
                "-marm",
                "-mfloat-abi=soft",
                "-Oz",
                "-fpie",
                "-fvisibility=hidden",
                "-ffreestanding",
                "-fno-builtin",
                "-fno-stack-protector",
                "-c",
                &source,
                "-o",
            ])
            .arg(&object));
        run(
            Command::new(env::var_os("LD_LLD").unwrap_or_else(|| "ld.lld".into()))
                .args(["-m", "armelf", "-Bsymbolic", "--oformat=binary", "-T"])
                .arg(&script)
                .arg(&object)
                .arg("-o")
                .arg(&image),
        );
    }
}
