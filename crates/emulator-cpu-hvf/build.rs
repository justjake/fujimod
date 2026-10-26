fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos")
        && std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64")
    {
        cc::Build::new()
            .file("src/simd.c")
            .compile("camera-hvf-simd");
        println!("cargo:rerun-if-changed=src/simd.c");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
