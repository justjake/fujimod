//! apple-cf links a static Swift bridge whose objects autolink the Swift
//! runtime (`-lswiftCore`). Linkers other than swiftc search for it only in
//! the SDK's `usr/lib/swift`, which no crate passes, so binaries that are not
//! dynamic Python modules (tests, workers) fail to link without this path.
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let output = Command::new("xcrun")
        .args(["--sdk", "macosx", "--show-sdk-path"])
        .output()
        .expect("xcrun must locate the macOS SDK");
    assert!(
        output.status.success(),
        "xcrun could not locate the macOS SDK"
    );
    let sdk = String::from_utf8(output.stdout).expect("SDK path must be UTF-8");
    println!(
        "cargo:rustc-link-search=native={}/usr/lib/swift",
        sdk.trim()
    );
}
