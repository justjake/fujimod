//! Inputs the camera-layer integration tests share.
//!
//! Set \`FUJIMOD_FIRMWARE_LIBRARY\` to a library laid out by name. A test whose
//! inputs are absent returns without opening a camera. Every test uses a
//! scratch state directory under \`/tmp\`.
#![allow(dead_code)]

use fujimod_emulator_camera::{
    BootCache, CardInputs, ExecutionPolicy, FirmwareInput, Flash, Library, LibraryError,
    LoaderImage, Request, Start, StateDir, directory_files,
};
use std::path::{Path, PathBuf};

pub const FIRMWARE: &str = "x70-110";

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
    match Library::from_env() {
        Ok(library) => Some(library),
        Err(LibraryError::Environment) => None,
        Err(error) => panic!("firmware library: {error}"),
    }
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
        firmware: FIRMWARE.into(),
        files: FirmwareInput::Library,
        start,
        flash: Flash::Stock,
        card: CardInputs {
            files,
            ..Default::default()
        },
        policy: ExecutionPolicy::Interactive,
        engine: None,
        cache,
        run_to_rec: true,
        kept: Default::default(),
    }
}

/// The directory named by \`FUJIMOD_TEST_SD_CARD\`, when set.
pub fn test_card() -> Option<PathBuf> {
    std::env::var_os("FUJIMOD_TEST_SD_CARD").map(PathBuf::from)
}

/// The release card a preset test downloads, when the state directory holds one.
pub fn release_card(state: &StateDir, firmware: &str) -> Option<PathBuf> {
    let cards: Vec<PathBuf> = std::fs::read_dir(state.sd_cards())
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(firmware))
        .filter(|path| path.is_dir())
        .collect();
    cards
        .into_iter()
        .max_by_key(|path| std::fs::metadata(path).ok().and_then(|m| m.modified().ok()))
}

/// A path's name, for a test message.
pub fn display(path: &Path) -> String {
    path.display().to_string()
}
