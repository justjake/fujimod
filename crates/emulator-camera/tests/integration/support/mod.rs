//! Inputs the camera-layer integration tests share.
//!
//! Set \`FUJIMOD_FIRMWARE_LIBRARY\` to a firmware library. A test whose
//! inputs are absent returns without opening a camera. Every test uses a
//! scratch state directory under \`/tmp\`.
#![allow(dead_code)]

use fujimod_emulator_camera::{
    BootCache, Card, Cards, ExecutionPolicy, FirmwareChoice, Library, LoaderImage, Request, Start,
    StateDir, directory_files,
};
use std::path::{Path, PathBuf};

pub const FIRMWARE: &str = "x70-110";
pub const BODY: &str = "x70";

/// How a test starts the camera.
pub enum Boot {
    Stock,
    SdCard(CardSource),
}

/// Where a card boot's files come from.
pub enum CardSource {
    Directory(PathBuf),
    Files(Vec<(String, Vec<u8>)>),
}

pub fn library() -> Option<Library> {
    let library = Library::from_env().unwrap_or_else(|error| panic!("firmware library: {error}"));
    library
        .firmwares()
        .iter()
        .any(|firmware| firmware.id == FIRMWARE)
        .then_some(library)
}

/// A scratch state directory, deleted when the test's process exits.
pub fn state() -> StateDir {
    StateDir::at(std::env::temp_dir().join(format!("fujimod-camera-test-{}", std::process::id())))
}

/// The emulator-layer request a test's start describes.
pub fn request(boot: Boot, image: Option<LoaderImage>, cache: BootCache) -> Request {
    let (start, files) = match boot {
        Boot::Stock => match image {
            Some(image) => (Start::Loader(image), Vec::new()),
            None => (Start::Rom, Vec::new()),
        },
        Boot::SdCard(CardSource::Directory(directory)) => (
            Start::Held {
                combination: "sd".into(),
            },
            directory_files(&directory).expect("SD card directory"),
        ),
        Boot::SdCard(CardSource::Files(files)) => (
            Start::Held {
                combination: "sd".into(),
            },
            files,
        ),
    };
    Request {
        start,
        cards: Cards {
            sd_card: Some(Card::with_files(files)),
            ..Default::default()
        },
        policy: ExecutionPolicy::Interactive,
        cache,
        ..Request::new(BODY, FirmwareChoice::Named(FIRMWARE.into()))
    }
}

/// A path's name, for a test message.
pub fn display(path: &Path) -> String {
    path.display().to_string()
}
