//! Install the pinned, signed FSKit host on first use. No camera is open here.
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const APP: &str = "FSKitBridge.app";
const URL: &str =
    "https://github.com/debox-network/FSKitBridge/releases/download/v0.1.0/FSKitBridge-0.1.0.zip";
const SHA256: &str = "bb5cf867f4c73e10fb33cb779c7367628eb6a339fcdda745993ef72586fcbe11";
fn run(command: &mut Command) -> Result<()> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!("{command:?}: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(())
}
pub fn prepare() -> Result<()> {
    if !Path::new("/Applications").join(APP).is_dir() {
        eprintln!("Installing FSKitBridge 0.1.0...");
        let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let directory =
            std::env::temp_dir().join(format!("fujimod-fskit-{}-{unique}", std::process::id()));
        fs::create_dir(&directory)?;
        let installed = install(&directory);
        // This directory contains only this invocation's downloaded installer.
        let cleanup = fs::remove_dir_all(&directory);
        installed?;
        cleanup?;
    }
    fskit_rs::activate(APP)?;
    let profile = Path::new("/Applications")
        .join(APP)
        .join("Contents/Extensions/FSKitExt.appex/Contents/embedded.provisionprofile");
    if profile.exists() {
        let decoded = Command::new("/usr/bin/security")
            .args(["cms", "-D", "-i"])
            .arg(profile)
            .output()?;
        if !decoded.status.success() {
            return Err("Could not validate the FSKitBridge provisioning profile".into());
        }
        let value = plist::Value::from_reader(std::io::Cursor::new(decoded.stdout))?;
        let expiration = value
            .as_dictionary()
            .and_then(|d| d.get("ExpirationDate"))
            .and_then(plist::Value::as_date)
            .ok_or("FSKitBridge provisioning profile has no expiration date")?;
        if SystemTime::from(expiration) <= SystemTime::now() {
            return Err(format!("FSKitBridge provisioning profile expired at {}; install a release with a current profile or a locally signed build", expiration.to_xml_format()).into());
        }
    }
    Ok(())
}
fn install(directory: &Path) -> Result<()> {
    let archive: PathBuf = directory.join("bridge.zip");
    run(Command::new("/usr/bin/curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--connect-timeout",
            "15",
            "--max-time",
            "120",
            URL,
            "--output",
        ])
        .arg(&archive))?;
    if format!("{:x}", Sha256::digest(fs::read(&archive)?)) != SHA256 {
        return Err("FSKitBridge archive hash differs from the pinned release".into());
    }
    run(Command::new("/usr/bin/ditto")
        .args(["-xk"])
        .arg(&archive)
        .arg(directory))?;
    // The bridge installer checks the bundle signature before and after copying
    // it to /Applications and registers its embedded filesystem extension.
    fskit_rs::install(directory.join(APP))?;
    Ok(())
}
