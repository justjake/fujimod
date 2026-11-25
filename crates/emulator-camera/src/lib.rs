//! Open an emulated camera from a firmware library and a boot choice.
//!
//! A caller names a catalog firmware and a boot. The installed firmware is
//! implemented; the SD card boot is part of the API and reports that it is not
//! implemented yet. Everything else, the run configuration, overlays and
//! execution policy, comes from the catalog.
//!
//! Opening returns only when the camera reaches REC (or its first-start
//! setup). Every other outcome is an OpenError that names the boot stage it
//! reached.
mod library;
mod open;

pub use library::{Library, LibraryError};
pub use open::{Boot, CardSource, OpenError, Opened, Request, Stage, open};

/// The scheduler engine a request may override.
pub use fujimod_catalog_configuration::RunEngine;
/// The CPU execution policy a request selects.
pub use fujimod_emulator_cpu::ExecutionPolicy;
