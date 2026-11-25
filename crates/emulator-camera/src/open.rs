//! Turn a firmware, a boot choice and a request into a camera at REC.
use crate::{Library, LibraryError, automatic};
use fujimod_catalog_configuration::{RunEngine, RunInputs};
use fujimod_catalog_controls::{BootCombination, BootMode, PhysicalControls};
use fujimod_catalog_remote::{Facts, Selection};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_boot::{self as boot, Camera};
use fujimod_emulator_cpu::ExecutionPolicy;
use fujimod_emulator_session::Session;
use sha2::{Digest, Sha256};
use std::{
    fmt, fs,
    path::{Path, PathBuf},
};

/// Largest instruction budget an open run may execute.
const INSTRUCTION_LIMIT: u64 = 2_000_000_000;

/// How a camera starts.
#[derive(Clone, Debug)]
pub enum Boot {
    /// The firmware installed in internal flash.
    Stock,
    /// FujiMod loaded from the SD card by the boot ROM.
    SdCard(CardSource),
    /// FujiMod from the SD card, started by the automatic-boot installer.
    AutomaticBoot(CardSource),
}

/// Where an SD card's files come from.
#[derive(Clone, Debug)]
pub enum CardSource {
    /// The current release's SD card, fetched once into the firmware library.
    Release,
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
    pub(crate) fn at(stage: Stage, detail: impl Into<String>) -> Self {
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
    /// The boot stage the camera reached: `Stage::Rec` after a boot, or
    /// `Stage::BootRom` from `open_unbooted`. An SD card boot that reached REC
    /// has necessarily passed `Stage::FujiModApplied`.
    pub stage: Stage,
    pub card_files: Vec<(String, PathBuf)>,
    /// The catalog resolution the camera was built from.
    pub resolved: ResolvedConfiguration,
    /// The release tag when the boot used the release card.
    pub release: Option<String>,
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
    if matches!(request.boot, Boot::AutomaticBoot(_)) {
        return open_automatic(library, request);
    }
    open_booted(library, request)
}

fn open_booted(library: &Library, request: Request) -> Result<Opened, OpenError> {
    let mut opened = construct(library, request)?;
    let holds = boot_holds(&opened)?;
    boot_camera(&mut opened, &holds)?;
    Ok(opened)
}

/// The controls a boot holds from power-on, from the catalog's boot entry.
fn boot_holds(opened: &Opened) -> Result<Vec<fujimod_catalog_controls::BootHold>, OpenError> {
    match &opened.boot {
        Boot::Stock | Boot::AutomaticBoot(_) => Ok(Vec::new()),
        Boot::SdCard(_) => sd_holds(&opened.resolved),
    }
}

/// Run a constructed camera to REC, holding and releasing `holds` around it.
fn boot_camera(
    opened: &mut Opened,
    holds: &[fujimod_catalog_controls::BootHold],
) -> Result<(), OpenError> {
    hold_boot_inputs(
        &mut opened.camera.session,
        &opened.resolved,
        holds,
        opened.camera.ticks_per_second,
    )?;
    let card_before = card_revision(&opened.camera.session, &opened.boot);
    let report = boot::to_rec(&mut opened.camera.session, INSTRUCTION_LIMIT)
        .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    if report.status == "rec" && !report.pending && report.scanout_instance.is_some() {
        finish_boot(
            &mut opened.camera.session,
            &opened.resolved,
            holds,
            opened.camera.ticks_per_second,
        )?;
        opened.stage = Stage::Rec;
        return Ok(());
    }
    let stage = reached_stage(
        &opened.camera.session,
        &opened.resolved,
        &opened.boot,
        card_before,
        &report,
    );
    Err(OpenError::at(
        stage,
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

/// Open the installed automatic boot, creating its internal flash once with
/// the card's own installer.
fn open_automatic(library: &Library, request: Request) -> Result<Opened, OpenError> {
    let Boot::AutomaticBoot(source) = &request.boot else {
        return Err(OpenError::at(Stage::BootRom, "not an automatic boot"));
    };
    let facts = library.nand_boot(&request.firmware)?;
    let contents = card_contents(library, &request.firmware, source)?;
    let key = automatic_key(&request.firmware, &contents);
    let directory = library
        .path()
        .join(crate::library::AUTOMATIC_DIRECTORY)
        .join(&request.firmware);
    let flash = directory.join(format!("{key}.flash"));
    let kept = match fs::read(&flash) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let installed = install_automatic(library, &request, &contents, facts)?;
            fs::create_dir_all(&directory)
                .map_err(|error| OpenError::at(Stage::Rec, error.to_string()))?;
            fs::write(&flash, &installed)
                .map_err(|error| OpenError::at(Stage::Rec, error.to_string()))?;
            installed
        }
        Err(error) => {
            return Err(OpenError::at(
                Stage::BootRom,
                format!("{}: {error}", flash.display()),
            ));
        }
    };
    let mut opened = construct(library, request)?;
    automatic::write_kept_flash(opened.session_mut(), facts, &kept)?;
    boot_camera(&mut opened, &[])?;
    fujimod_resident(&opened, facts.load_range.address)?;
    Ok(opened)
}

/// Boot the card through the body's SD holds and let its installer write the
/// stage-1 blocks, returning the blocks it kept.
fn install_automatic(
    library: &Library,
    request: &Request,
    contents: &[(String, Vec<u8>)],
    facts: &fujimod_catalog::nand_boot::FirmwareNandBoot,
) -> Result<Vec<u8>, OpenError> {
    let mut installer = construct(
        library,
        Request {
            firmware: request.firmware.clone(),
            boot: Boot::SdCard(CardSource::Files(automatic::stock_menus(contents)?)),
            policy: request.policy,
            engine: request.engine,
            card_files: request.card_files.clone(),
        },
    )?;
    let holds = sd_holds(&installer.resolved)?;
    boot_camera(&mut installer, &holds)?;
    automatic::install(&mut installer, facts)
}

/// The `fujimod-remote` facts for a resolved camera: the menu, control and
/// readiness tables a remote reads.
pub fn remote_facts(resolved: &ResolvedConfiguration) -> Result<Facts, String> {
    let selection = &resolved.reference;
    let input = Selection {
        firmware_id: &selection.firmware.id,
        body_id: &selection.body.id,
        firmware_sha256: selection
            .firmware
            .update
            .as_ref()
            .map(|content| content.sha256.as_str()),
        remote_facts: selection
            .firmware
            .generated_facts
            .as_ref()
            .and_then(|facts| facts.remote.as_ref()),
        physical_controls: selection.physical_controls.as_ref(),
        control_ids: selection.control_ids.as_ref(),
        connections: selection
            .body
            .connections
            .iter()
            .map(|connection| connection.id.as_str())
            .collect(),
    };
    Facts::from_selection(&input, None)
}

/// How `fujimod-remote` reads this session's screen and completed frames.
pub fn remote_options(
    session: &Session,
    quantum: usize,
) -> Result<fujimod_remote::board::Options, String> {
    Ok(fujimod_remote::board::Options {
        quantum,
        window_instructions: 100_000,
        scanout_node: boot::scanout_node(session)?,
        output: "lcd".into(),
        // The EXR II display-timing model counts completed LCD frames here.
        frame_counter_kind: "display-timing".into(),
        frame_counter_pointer: "/0/banks/1/frames".into(),
    })
}

/// Every file on a card source, sorted by its name.
fn card_contents(
    library: &Library,
    firmware: &str,
    source: &CardSource,
) -> Result<Vec<(String, Vec<u8>)>, OpenError> {
    let mut files = match source {
        CardSource::Release => {
            let directory = library.release_card(firmware)?;
            read_files(&directory_files(&directory)?)?
        }
        CardSource::Directory(directory) => read_files(&directory_files(directory)?)?,
        CardSource::Files(files) => files.clone(),
    };
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

/// Hash the firmware and the card's names and bytes, so a changed card gets
/// its own installed flash.
fn automatic_key(firmware: &str, contents: &[(String, Vec<u8>)]) -> String {
    let mut hash = Sha256::new();
    hash.update(firmware.as_bytes());
    for (name, bytes) in contents {
        hash.update([0]);
        hash.update(name.as_bytes());
        hash.update([0]);
        hash.update(bytes);
    }
    format!("{:x}", hash.finalize())
}

fn read_files(files: &[(String, PathBuf)]) -> Result<Vec<(String, Vec<u8>)>, OpenError> {
    files
        .iter()
        .map(|(name, path)| {
            fs::read(path)
                .map(|bytes| (name.clone(), bytes))
                .map_err(|error| {
                    OpenError::at(Stage::BootRom, format!("{}: {error}", path.display()))
                })
        })
        .collect()
}

/// The FujiMod stage-2 loader carries its own UI strings; finding one proves
/// FujiMod is resident rather than the stock firmware.
fn fujimod_resident(opened: &Opened, start: u64) -> Result<(), OpenError> {
    const MARKER: &[u8] = b"FUJIMOD SET-UP";
    const BYTES: u64 = 1 << 24;
    const CHUNK: u64 = 1 << 20;
    let mut chunk = vec![0u8; CHUNK as usize];
    for base in (start..start + BYTES).step_by(CHUNK as usize) {
        if opened
            .session()
            .board
            .read_physical(base, &mut chunk)
            .is_ok()
            && chunk.windows(MARKER.len()).any(|window| window == MARKER)
        {
            return Ok(());
        }
    }
    Err(OpenError::at(
        Stage::Rec,
        "FujiMod is not resident after the automatic boot",
    ))
}

/// Build a camera's board and media without running its guest. A checkpoint
/// restores into the session this returns, built from the same catalog facts
/// the boot would use.
pub fn open_unbooted(library: &Library, request: Request) -> Result<Opened, OpenError> {
    construct(library, request)
}

fn construct(library: &Library, request: Request) -> Result<Opened, OpenError> {
    let Request {
        firmware,
        boot,
        policy,
        engine,
        card_files,
    } = request;
    fujimod_emulator_exr2_board::register_board_builder();
    fujimod_emulator_models::register();
    let mut resolved = library.resolved(&firmware)?;
    if let Some(engine) = engine {
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
    let (boot_card_files, release) = match &boot {
        Boot::Stock => (Vec::new(), None),
        Boot::SdCard(source) | Boot::AutomaticBoot(source) => {
            source_card_files(library, &firmware, source)?
        }
    };
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
            card_files: boot_card_files,
            discovery: false,
        },
    )
    .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    camera
        .session
        .board
        .set_execution_policy(policy)
        .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    if let Boot::SdCard(CardSource::Files(files)) | Boot::AutomaticBoot(CardSource::Files(files)) =
        &boot
    {
        install_files(&mut camera.session, files)?;
    }
    if !card_files.is_empty() {
        boot::media::attach(&mut camera.session, None, &card_files)
            .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    }
    Ok(Opened {
        camera,
        firmware,
        boot,
        policy,
        engine,
        stage: Stage::BootRom,
        card_files,
        release,
        resolved,
    })
}

/// The card files a source contributes and the release tag when it is published.
fn source_card_files(
    library: &Library,
    firmware: &str,
    source: &CardSource,
) -> Result<(Vec<(String, PathBuf)>, Option<String>), OpenError> {
    match source {
        CardSource::Release => {
            let directory = library.release_card(firmware)?;
            let tag = directory
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .map(str::to_owned);
            Ok((directory_files(&directory)?, tag))
        }
        CardSource::Directory(directory) => Ok((directory_files(directory)?, None)),
        CardSource::Files(_) => Ok((Vec::new(), None)),
    }
}

/// Release the boot holds and connect the controls the running firmware loads.
fn finish_boot(
    session: &mut Session,
    resolved: &ResolvedConfiguration,
    holds: &[fujimod_catalog_controls::BootHold],
    ticks_per_second: u64,
) -> Result<(), OpenError> {
    if !holds.is_empty() {
        for hold in holds {
            let position = session
                .contact_driver
                .as_ref()
                .ok_or_else(|| OpenError::at(Stage::Rec, "boot holds lost the contact driver"))?
                .boot_release_position(&hold.id)
                .map_err(|error| OpenError::at(Stage::Rec, error))?;
            session
                .set_position(&hold.id, &position)
                .map_err(|error| OpenError::at(Stage::Rec, error))?;
        }
        // The boot panel was compiled before the firmware's calibration existed;
        // connect again so presses answer the loaded firmware.
        session.controls = None;
        session.contact_driver = None;
    }
    fujimod_emulator_controls::connect(session, &resolved.reference, ticks_per_second)
        .map(|_| ())
        .map_err(|error| OpenError::at(Stage::Rec, error))
}

/// The body controls document's boot.sd.hold list.
fn sd_holds(
    resolved: &ResolvedConfiguration,
) -> Result<Vec<fujimod_catalog_controls::BootHold>, OpenError> {
    let value = resolved
        .reference
        .physical_controls
        .as_ref()
        .ok_or_else(|| {
            OpenError::at(
                Stage::BootRom,
                format!(
                    "body {} has no physical controls document",
                    resolved.reference.body.id
                ),
            )
        })?;
    let controls: PhysicalControls = serde_json::from_value(value.clone()).map_err(|error| {
        OpenError::at(
            Stage::BootRom,
            format!(
                "body {} physical controls: {error}",
                resolved.reference.body.id
            ),
        )
    })?;
    let combination: &BootCombination = controls.boot.get(&BootMode::Sd).ok_or_else(|| {
        OpenError::at(
            Stage::BootRom,
            format!(
                "body {} declares no boot.sd controls",
                resolved.reference.body.id
            ),
        )
    })?;
    Ok(combination.hold.clone())
}

/// Hold the catalog's boot controls through the session's contact driver.
fn hold_boot_inputs(
    session: &mut Session,
    resolved: &ResolvedConfiguration,
    holds: &[fujimod_catalog_controls::BootHold],
    ticks_per_second: u64,
) -> Result<(), OpenError> {
    if holds.is_empty() {
        return Ok(());
    }
    let located = fujimod_catalog_wiring::locate(&resolved.reference)
        .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    let controls =
        fujimod_emulator_controls::compile(&located, &session.definition, None, ticks_per_second)
            .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    session
        .configure_controls(controls.wiring)
        .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    session
        .configure_contact_driver(controls.confirmation)
        .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    for hold in holds {
        let position = session
            .contact_driver
            .as_ref()
            .ok_or_else(|| OpenError::at(Stage::BootRom, "boot holds need the contact driver"))?
            .boot_position(&hold.id, hold.stage.as_deref())
            .map_err(|error| OpenError::at(Stage::BootRom, error))?;
        session
            .set_position(&hold.id, &position)
            .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    }
    Ok(())
}

fn card_revision(session: &Session, boot: &Boot) -> Option<u64> {
    if matches!(boot, Boot::Stock) {
        return None;
    }
    let name = session.contacts.keys().next()?;
    session
        .card_state(name)
        .ok()
        .and_then(|state| state.revision)
}

fn reached_stage(
    session: &Session,
    resolved: &ResolvedConfiguration,
    boot: &Boot,
    card_before: Option<u64>,
    report: &boot::RecReport,
) -> Stage {
    if report.instructions == 0 {
        return Stage::BootRom;
    }
    let sd = matches!(boot, Boot::SdCard(_) | Boot::AutomaticBoot(_));
    if sd
        && let Some(before) = card_before
        && card_revision(session, boot).is_some_and(|after| after != before)
    {
        return Stage::FujiModApplied;
    }
    let loaded = configuration_loaded(session, resolved);
    if sd {
        return if loaded || report.ui_mode != u32::MAX {
            Stage::FujiModApplied
        } else {
            Stage::SdLoader
        };
    }
    if loaded || report.ui_mode != u32::MAX {
        Stage::FirmwareLoaded
    } else {
        Stage::BootRom
    }
}

/// Whether the firmware's generated configuration is readable from the session.
fn configuration_loaded(session: &Session, resolved: &ResolvedConfiguration) -> bool {
    resolved
        .reference
        .firmware
        .generated_facts
        .as_ref()
        .and_then(|facts| facts.cfgdata_location.as_ref())
        .and_then(|location| {
            serde_json::from_value::<fujimod_catalog_controls::facts::ConfigurationLocation>(
                location.clone(),
            )
            .ok()
        })
        .is_some_and(|location| {
            fujimod_emulator_controls::LiveCalibration::new(session, location).is_ok()
        })
}

fn install_files(session: &mut Session, files: &[(String, Vec<u8>)]) -> Result<(), OpenError> {
    if files.is_empty() {
        return Ok(());
    }
    let name =
        session.contacts.keys().next().cloned().ok_or_else(|| {
            OpenError::at(Stage::BootRom, "the run declares no modeled SD card slot")
        })?;
    session
        .install_card_files(&name, None, files)
        .map_err(|error| OpenError::at(Stage::BootRom, error))
}

/// Every file under a directory, named by its path relative to it.
fn directory_files(directory: &Path) -> Result<Vec<(String, PathBuf)>, OpenError> {
    let mut files = Vec::new();
    let mut pending = vec![directory.to_owned()];
    while let Some(next) = pending.pop() {
        let entries = std::fs::read_dir(&next).map_err(|error| {
            OpenError::at(Stage::BootRom, format!("{}: {error}", next.display()))
        })?;
        for entry in entries {
            let path = entry
                .map_err(|error| OpenError::at(Stage::BootRom, error.to_string()))?
                .path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let name = path
                    .strip_prefix(directory)
                    .map_err(|error| OpenError::at(Stage::BootRom, error.to_string()))?
                    .to_str()
                    .ok_or_else(|| OpenError::at(Stage::BootRom, "SD card file name is not UTF-8"))?
                    .replace('\\', "/");
                files.push((name, path));
            }
        }
    }
    files.sort();
    if files.is_empty() {
        return Err(OpenError::at(
            Stage::BootRom,
            format!("SD card directory {} is empty", directory.display()),
        ));
    }
    Ok(files)
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
