//! Open an emulated camera from a firmware library and a boot choice.
//!
//! A caller names a catalog firmware and a boot: the installed firmware,
//! FujiMod from an SD card held at power-on, or the card's automatic boot
//! installed into internal flash. Everything else, the run configuration,
//! overlays, boot controls, card geometry and execution policy, comes from the
//! catalog.
//!
//! Opening returns only when the camera reaches REC (or its first-start
//! setup). Every other outcome is an OpenError that names the boot stage it
//! reached. A boot is saved in the firmware library after it reaches REC, and
//! a later open with the same inputs restores that checkpoint instead of
//! booting.
mod automatic;
mod cache;
mod library;
mod open;
mod release;

pub use library::{Library, LibraryError, LibraryFirmware};
pub use open::{
    Boot, BootCache, BootHolds, CardModel, CardSource, KeptMedia, LoaderImage, OpenError, Opened,
    PowerOn, Request, Stage, directory_files, open, open_unbooted, remote_facts, remote_options,
};
pub use release::RELEASES_URL;

/// The scheduler engine a request may override.
pub use fujimod_catalog_configuration::RunEngine;
/// The CPU execution policy a request selects.
pub use fujimod_emulator_cpu::ExecutionPolicy;
