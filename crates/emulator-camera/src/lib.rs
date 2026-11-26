//! Open an emulated camera: the emulator layer.
//!
//! A caller names the firmware files, the internal flash, the SD card's files
//! and options, and how the camera starts. Everything else, the run
//! configuration, overlays, boot controls, card geometry and execution policy,
//! comes from the catalog. Anything that wants a camera without FujiMod opens
//! it here.
//!
//! Opening returns only when the camera reaches REC (or its first-start
//! setup). Every other outcome is an OpenError that names the boot stage it
//! reached. A boot is saved in the state directory's boot cache when the
//! caller asked for it, and a later open with the same inputs restores that
//! checkpoint instead of booting.
mod cache;
mod flash;
mod library;
mod open;
mod state;

pub use flash::{read as read_flash, write as write_flash};
pub use library::{
    BOOT_ROM_FILE, FirmwareFiles, LIBRARY_ENV, Library, LibraryError, LibraryFirmware, UPDATE_FILE,
};
pub use open::{
    BootCache, BootHolds, CardInputs, FirmwareInput, Flash, KeptMedia, LoaderImage, OpenError,
    Opened, PowerOn, Request, Start, directory_files, open, open_unbooted, remote_facts,
    remote_options,
};
pub use state::{AUTOSTART, BOOT_CACHE, DEFAULT_STATE, SD_CARDS, STATE_ENV, StateDir};

/// The last boot stage a camera reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// The boot ROM ran, or the camera could not start it.
    BootRom,
    /// The stock firmware started but did not present REC.
    FirmwareLoaded,
    /// The boot ROM handed off to the SD card's loader.
    SdLoader,
    /// The card's stage 3 applied the firmware patch and the stock firmware
    /// started from RAM.
    FujiModApplied,
    /// The firmware loaded and presented its REC user interface.
    Rec,
}

impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Self::BootRom => "boot ROM",
            Self::FirmwareLoaded => "firmware loaded",
            Self::SdLoader => "SD loader",
            Self::FujiModApplied => "FujiMod applied",
            Self::Rec => "REC",
        }
    }
}

impl std::fmt::Display for Stage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

/// The scheduler engine a request may override.
pub use fujimod_catalog_configuration::RunEngine;
/// The external face detector results a request may select.
pub use fujimod_catalog_run_inputs::RunFaceResultProfile;
/// The camera a boot reaches.
pub use fujimod_emulator_boot::Camera;
/// A declared FAT geometry for a stock SD card.
pub use fujimod_emulator_boot::CardLayout;
/// The CPU execution policy a request selects.
pub use fujimod_emulator_cpu::ExecutionPolicy;
