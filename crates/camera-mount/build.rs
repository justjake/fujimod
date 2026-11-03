use std::{env, fs, path::PathBuf, process::Command};
fn run(command: &mut Command) {
    let result = command.output().expect("run payload tool");
    assert!(
        result.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}
fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let script = out.join("payload.ld");
    fs::write(&script,"SECTIONS { . = 0; .text : { *(.text.entry) *(.text*) } .rodata : { *(.rodata*) } .data : { *(.data*) *(.bss*) } /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) *(.comment*) *(.note*) *(.eh_frame*) } }").unwrap();
    for name in ["call", "card"] {
        let source = format!("payload/{name}.c");
        println!("cargo:rerun-if-changed={source}");
        let object = out.join(format!("{name}.o"));
        let image = out.join(format!("{name}.bin"));
        run(Command::new(env::var_os("CLANG").unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "/usr/bin/clang".into()
            } else {
                "clang".into()
            }
        }))
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
