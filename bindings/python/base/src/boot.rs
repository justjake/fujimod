//! What the emulator bindings call to prepare and boot catalog runs. The extension module
//! implements it and installs it, so the emulator bindings depend on no catalog crate and do not
//! wait for the boot assembly to rebuild.
use fujimod_emulator_session::Session;
use pyo3::{exceptions::PyRuntimeError, prelude::*};
use std::{path::PathBuf, sync::OnceLock};

/// Caller-selected files and settings for one catalog run.
pub struct Inputs {
    pub configuration_id: String,
    pub overlays: Vec<String>,
    pub update: PathBuf,
    pub boot_rom: PathBuf,
    pub nand: Option<PathBuf>,
    pub settings: Option<PathBuf>,
    pub clock_seconds_2000: Option<u32>,
    pub preferred_languages: Vec<String>,
    pub camera: Option<PathBuf>,
}

pub trait Boot: Sync {
    /// The prepared run document as JSON.
    fn prepare(&self, configuration_id: &str, overlays: &[String]) -> Result<String, String>;
    /// An unexecuted session for the run.
    fn open(&self, inputs: Inputs) -> Result<Session, String>;
    /// Run to REC and return the boot report as JSON.
    fn to_rec(&self, session: &mut Session, limit: u64) -> Result<String, String>;
    /// The composed LCD as PNG bytes.
    fn screenshot(&self, session: &Session) -> Result<Vec<u8>, String>;
}

static BOOT: OnceLock<&'static dyn Boot> = OnceLock::new();

/// Install the boot implementation; later installations are ignored.
pub fn install(boot: &'static dyn Boot) {
    let _ = BOOT.set(boot);
}

pub fn boot() -> PyResult<&'static dyn Boot> {
    BOOT.get()
        .copied()
        .ok_or_else(|| PyRuntimeError::new_err("catalog boots are not installed"))
}
