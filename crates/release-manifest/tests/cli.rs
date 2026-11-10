use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_fujimod-release-manifest");
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn test_dir(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("release-manifest-tests")
        .join(name);
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn command() -> Command {
    Command::new(BIN)
}

fn run(command: &mut Command) -> Output {
    command.output().expect("run fujimod-release-manifest")
}

fn assert_success(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_sources(directory: &Path) -> (PathBuf, PathBuf) {
    let abc = directory.join("abc.bin");
    let empty = directory.join("empty.bin");
    fs::write(&abc, b"abc").unwrap();
    fs::write(&empty, b"").unwrap();
    (abc, empty)
}

fn release_command(directory: &Path, abc: &Path, empty: &Path) -> Command {
    let mut command = command();
    command
        .arg("--firmware")
        .arg("example-cam")
        .arg("--version")
        .arg("1.2.3")
        .arg("--output")
        .arg(directory.join("out"))
        .arg(format!("/firmware.patch={}", abc.display()))
        .arg(format!("/etc/ssl/cert.pem={}", empty.display()));
    command
}

#[test]
fn writes_the_manifest_and_store_files() {
    let directory = test_dir("writes");
    let (abc, empty) = write_sources(&directory);
    assert_success(run(&mut release_command(&directory, &abc, &empty)));

    let output = directory.join("out");
    assert_eq!(fs::read(output.join("BA7816BF.8F0")).unwrap(), b"abc");
    assert_eq!(fs::read(output.join("E3B0C442.98F")).unwrap(), b"");
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output.join("latest.json")).unwrap()).unwrap();
    assert_eq!(manifest["firmware"], "example-cam");
    assert_eq!(manifest["version"], "1.2.3");
    assert_eq!(manifest["fs"]["/firmware.patch"]["sha256"], ABC_SHA256);
    assert_eq!(manifest["fs"]["/firmware.patch"]["size"], 3);
    assert_eq!(manifest["fs"]["/etc/ssl/cert.pem"]["sha256"], EMPTY_SHA256);
    assert_eq!(manifest["fs"]["/etc/ssl/cert.pem"]["size"], 0);
}

#[test]
fn writes_optional_release_metadata() {
    let directory = test_dir("metadata");
    let (abc, empty) = write_sources(&directory);
    let mut command = release_command(&directory, &abc, &empty);
    command
        .arg("--date")
        .arg("2026-11-10")
        .arg("--commit")
        .arg("abcdef0123456789")
        .arg("--note")
        .arg("First note")
        .arg("--note")
        .arg("Second note");
    assert_success(run(&mut command));
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(directory.join("out/latest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["date"], "2026-11-10");
    assert_eq!(manifest["commit"], "abcdef0123456789");
    assert_eq!(manifest["notes"][0], "First note");
    assert_eq!(manifest["notes"][1], "Second note");
}

#[test]
fn rejects_out_of_bounds_release_metadata() {
    let directory = test_dir("invalid-metadata");
    let (abc, empty) = write_sources(&directory);
    for (name, value) in [
        ("--date", "2026-1-10"),
        ("--date", "2026-02-30"),
        ("--commit", "ABCDEF0"),
        ("--note", &"x".repeat(65)),
    ] {
        let mut command = release_command(&directory, &abc, &empty);
        command.arg(name).arg(value);
        assert!(!run(&mut command).status.success(), "{name}={value}");
    }
}

#[test]
fn running_again_reuses_identical_store_files() {
    let directory = test_dir("idempotent");
    let (abc, empty) = write_sources(&directory);
    assert_success(run(&mut release_command(&directory, &abc, &empty)));
    assert_success(run(&mut release_command(&directory, &abc, &empty)));
}

#[test]
fn refuses_a_store_file_with_different_contents() {
    let directory = test_dir("conflict");
    let (abc, empty) = write_sources(&directory);
    let output = directory.join("out");
    fs::create_dir_all(&output).unwrap();
    fs::write(output.join("BA7816BF.8F0"), b"different").unwrap();
    let result = run(&mut release_command(&directory, &abc, &empty));
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("BA7816BF.8F0"));
    assert!(!output.join("latest.json").exists());
}

#[test]
fn refuses_duplicate_paths_and_bad_arguments() {
    let directory = test_dir("arguments");
    let (abc, _) = write_sources(&directory);
    let mut duplicate = command();
    duplicate
        .arg("--firmware")
        .arg("example-cam")
        .arg("--version")
        .arg("1.2.3")
        .arg("--output")
        .arg(directory.join("out"))
        .arg(format!("/firmware.patch={}", abc.display()))
        .arg(format!("/firmware.patch={}", abc.display()));
    assert!(!run(&mut duplicate).status.success());

    let mut missing = command();
    missing
        .arg("--firmware")
        .arg("example-cam")
        .arg("--version");
    assert!(!run(&mut missing).status.success());

    let help = run(command().arg("--help"));
    assert_success(help);
}

#[test]
fn refuses_an_invalid_manifest_path() {
    let directory = test_dir("path");
    let (abc, _) = write_sources(&directory);
    let mut command = command();
    command
        .arg("--firmware")
        .arg("example-cam")
        .arg("--version")
        .arg("1.2.3")
        .arg("--output")
        .arg(directory.join("out"))
        .arg(format!("/etc/../firmware.patch={}", abc.display()));
    assert!(!run(&mut command).status.success());
}
