//! Turn a firmware, a boot choice and a request into a camera at REC.
use crate::{Library, LibraryError};
use fujimod_catalog_configuration::{RunEngine, RunInputs};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_boot::{self as boot, Camera};
use fujimod_emulator_cpu::ExecutionPolicy;
use fujimod_emulator_session::Session;
use std::{fmt, path::PathBuf};

/// Largest instruction budget an open run may execute.
const INSTRUCTION_LIMIT: u64 = 2_000_000_000;

/// How a camera starts.
#[derive(Clone, Debug)]
pub enum Boot {
    /// The firmware installed in internal flash.
    Stock,
    /// FujiMod loaded from the SD card by the boot ROM.
    SdCard(CardSource),
}

/// Where an SD card's files come from.
#[derive(Clone, Debug)]
pub enum CardSource {
    /// A folder holding an SD card tree.
    Directory(PathBuf),
    /// Name and bytes pairs, for callers without a host filesystem.
    Files(Vec<(String, Vec<u8>)>),
}

/// One camera to open.
pub struct Request {
    /// Catalog firmware identity, for example x70-110.
    pub firmware: String,
    pub boot: Boot,
    pub policy: ExecutionPolicy,
    /// Scheduler engine override; the run configuration's engine otherwise.
    pub engine: Option<RunEngine>,
    /// Extra files written to the camera's SD card after the boot source's.
    pub card_files: Vec<(String, PathBuf)>,
}

/// The last boot stage a camera reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// The boot ROM ran, or the camera could not start it.
    BootRom,
    /// The stock firmware started but did not present REC.
    FirmwareLoaded,
    /// The boot ROM handed off to the SD card's loader.
    SdLoader,
    /// FujiMod was applied in RAM and the stock firmware started from it.
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

impl fmt::Display for Stage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Opening stopped before REC.
#[derive(Debug)]
pub struct OpenError {
    /// The furthest boot stage confirmed reached.
    pub stage: Stage,
    pub detail: String,
}

impl OpenError {
    fn at(stage: Stage, detail: impl Into<String>) -> Self {
        Self {
            stage,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for OpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "camera open stopped after {}: {}",
            self.stage, self.detail
        )
    }
}

impl std::error::Error for OpenError {}

impl From<LibraryError> for OpenError {
    fn from(error: LibraryError) -> Self {
        Self::at(Stage::BootRom, error.to_string())
    }
}

/// A camera that reached REC, with the facts it was opened with.
pub struct Opened {
    pub camera: Camera,
    pub firmware: String,
    pub boot: Boot,
    pub policy: ExecutionPolicy,
    pub engine: RunEngine,
    pub card_files: Vec<(String, PathBuf)>,
    /// The catalog resolution the camera was built from.
    pub resolved: ResolvedConfiguration,
}

impl Opened {
    pub fn session(&self) -> &Session {
        &self.camera.session
    }

    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.camera.session
    }
}

/// Open a camera and run it to REC or its first-start setup.
pub fn open(library: &Library, request: Request) -> Result<Opened, OpenError> {
    fujimod_emulator_exr2_board::register_board_builder();
    fujimod_emulator_models::register();
    let firmware = request.firmware.clone();
    let mut resolved = library.resolved(&firmware)?;
    if let Some(engine) = request.engine {
        resolved
            .override_engine(engine)
            .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    }
    let engine = resolved
        .configuration
        .execution
        .ok_or_else(|| {
            OpenError::at(
                Stage::BootRom,
                format!("{} declares no execution policy", resolved.configuration.id),
            )
        })?
        .engine;
    if !matches!(request.boot, Boot::Stock) {
        return Err(OpenError::at(
            Stage::BootRom,
            "SD card boots are not implemented yet",
        ));
    }
    let update = library.update(&firmware)?;
    let boot_rom = library.boot_rom(&firmware)?;
    let camera_directory = camera_directory(library, &resolved)?;
    let mut camera = boot::open_request(
        resolved.clone(),
        boot::Request {
            update,
            boot_rom: Some(boot_rom),
            nand: None,
            settings: None,
            camera: camera_directory,
            application_plan: None,
            card_files: Vec::new(),
            discovery: false,
        },
    )
    .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    camera
        .session
        .board
        .set_execution_policy(request.policy)
        .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    if !request.card_files.is_empty() {
        boot::media::attach(&mut camera.session, None, &request.card_files)
            .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    }
    let report = boot::to_rec(&mut camera.session, INSTRUCTION_LIMIT)
        .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    if report.status == "rec" && !report.pending && report.scanout_instance.is_some() {
        fujimod_emulator_controls::connect(
            &mut camera.session,
            &resolved.reference,
            camera.ticks_per_second,
        )
        .map_err(|error| OpenError::at(Stage::Rec, error))?;
        return Ok(Opened {
            camera,
            firmware,
            boot: request.boot,
            policy: request.policy,
            engine,
            card_files: request.card_files,
            resolved,
        });
    }
    Err(OpenError::at(
        reached_stage(&report),
        format!(
            "{} with {} instructions at tick {}: ui_mode {}, system_mode {}, pending {}, scanout {}{}",
            report.status,
            report.instructions,
            report.virtual_ticks,
            report.ui_mode,
            report.system_mode,
            report.pending,
            report.scanout_instance.is_some(),
            report
                .failure
                .as_ref()
                .map(|failure| format!(": {failure}"))
                .unwrap_or_default()
        ),
    ))
}

/// How far a stock boot got before REC.
fn reached_stage(report: &boot::RecReport) -> Stage {
    if report.instructions == 0 || report.ui_mode == u32::MAX {
        Stage::BootRom
    } else {
        Stage::FirmwareLoaded
    }
}

fn camera_directory(
    library: &Library,
    resolved: &ResolvedConfiguration,
) -> Result<Option<PathBuf>, OpenError> {
    let camera = match resolved.configuration.inputs.as_ref() {
        Some(RunInputs::Exr2(inputs)) => inputs
            .settings
            .as_ref()
            .and_then(|settings| settings.camera.clone()),
        Some(_) | None => None,
    };
    let Some(camera) = camera else {
        // A run without a settings camera builds its bank from the update.
        return Ok(None);
    };
    library
        .camera(&resolved.reference.body.id, &camera)
        .map(Some)
        .map_err(OpenError::from)
}
