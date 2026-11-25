//! The state directory: everything FujiMod writes and does not commit.
//!
//! It is named by `FUJIMOD_STATE_DIR`, or `/tmp/fujimod` when unset:
//!
//! ```text
//! sd-cards/<release>/<firmware>/     downloaded release SD cards
//! autostart/<firmware>/<key>.flash   kept autostart internal flash
//! boot-cache/                        saved boots
//! ```
//!
//! Everything in it can be made again.
use std::path::{Path, PathBuf};

/// The environment variable naming the state directory.
pub const STATE_ENV: &str = "FUJIMOD_STATE_DIR";
/// The state directory used when the environment variable is unset.
pub const DEFAULT_STATE: &str = "/tmp/fujimod";
/// The state subdirectory holding downloaded release SD cards.
pub const SD_CARDS: &str = "sd-cards";
/// The state subdirectory holding kept autostart internal flash.
pub const AUTOSTART: &str = "autostart";
/// The state subdirectory holding saved boots.
pub const BOOT_CACHE: &str = "boot-cache";

/// Where FujiMod keeps what it writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateDir {
    root: PathBuf,
}

impl StateDir {
    /// The directory named by `FUJIMOD_STATE_DIR`, or the default.
    pub fn from_env() -> Self {
        match std::env::var_os(STATE_ENV) {
            Some(path) if !path.is_empty() => Self::at(PathBuf::from(path)),
            _ => Self::at(DEFAULT_STATE),
        }
    }

    /// A state directory at an explicit path.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The state directory.
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// `<state>/sd-cards`.
    pub fn sd_cards(&self) -> PathBuf {
        self.root.join(SD_CARDS)
    }

    /// `<state>/autostart`.
    pub fn autostart(&self) -> PathBuf {
        self.root.join(AUTOSTART)
    }

    /// `<state>/boot-cache`.
    pub fn boot_cache(&self) -> PathBuf {
        self.root.join(BOOT_CACHE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_state_directory_is_under_tmp() {
        assert_eq!(
            StateDir::at(DEFAULT_STATE).path(),
            Path::new("/tmp/fujimod")
        );
        assert_eq!(
            StateDir::at("/tmp/fujimod").boot_cache(),
            Path::new("/tmp/fujimod/boot-cache")
        );
    }

    #[test]
    fn an_explicit_directory_names_every_subdirectory() {
        let state = StateDir::at("/nonexistent/state");
        assert_eq!(state.sd_cards(), Path::new("/nonexistent/state/sd-cards"));
        assert_eq!(state.autostart(), Path::new("/nonexistent/state/autostart"));
    }
}
