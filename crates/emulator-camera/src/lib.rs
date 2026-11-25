//! Open an emulated camera from a firmware library and a boot choice.
//!
//! A caller names a catalog firmware and a boot: the installed firmware, or
//! FujiMod from an SD card. Everything else, the run configuration, overlays,
//! boot controls, card geometry and execution policy, comes from the catalog.
//!
//! Opening returns only when the camera reaches REC (or its first-start
//! setup). Every other outcome is an OpenError that names the boot stage it
//! reached.
mod library;
mod open;
mod release;

pub use library::{Library, LibraryError, LibraryFirmware};
pub use open::{Boot, CardSource, OpenError, Opened, Request, Stage, open, open_unbooted};
pub use release::RELEASES_URL;

/// The scheduler engine a request may override.
pub use fujimod_catalog_configuration::RunEngine;
/// The CPU execution policy a request selects.
pub use fujimod_emulator_cpu::ExecutionPolicy;
