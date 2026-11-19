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
    pub boot_rom: Option<PathBuf>,
    pub card_image: Option<PathBuf>,
    pub card_files: Vec<(String, PathBuf)>,
    pub scene: Option<PathBuf>,
    pub engine: Option<String>,
    pub manifest: Option<PathBuf>,
    pub application_plan: Option<PathBuf>,
    pub discovery: bool,
    pub quantum: Option<usize>,
    pub instruction_rate: Option<(u64, u64)>,
    pub nand: Option<PathBuf>,
    pub settings: Option<PathBuf>,
    pub clock_seconds_2000: Option<u32>,
    pub preferred_languages: Vec<String>,
    pub camera: Option<PathBuf>,
    /// One virtual access point for the MT5931 endpoint, as JSON text.
    pub access_point: Option<String>,
    /// Explicit 20-byte MT5931 capability fixture.
    pub capability_reply: Option<Vec<u8>>,
    /// Named camera settings of the user profile an XP5 loader start boots with.
    pub camera_settings: std::collections::BTreeMap<String, String>,
}

/// A native catalog session with its processor-specific physical input handle.
pub enum Opened {
    Graph(Session),
    Xp4(Box<dyn crate::xp4_backend::Xp4Backend>),
    Xp5 {
        session: Session,
        inputs: fujimod_emulator_xp5_board::session::Inputs,
        read_limit: usize,
    },
}

pub trait Boot: Sync {
    /// The prepared run document as JSON.
    fn prepare(&self, configuration_id: &str, overlays: &[String]) -> Result<String, String>;
    /// An unexecuted session for the run.
    fn open(&self, inputs: Inputs) -> Result<Opened, String>;
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
