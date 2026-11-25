//! Turn firmware files, media and a start choice into a camera at REC.
//!
//! This is the emulator layer: it knows cameras, not FujiMod. A request names
//! the firmware (by library id, or by its two files), the internal flash, the
//! SD card's files and options, and how the camera starts: from the boot ROM
//! with nothing held, from the boot ROM with a catalog power-on combination
//! held, or at the native loader with a caller-verified RAM image. Opening
//! writes nothing to disk unless the caller asked for the boot cache.
use crate::{
    Library, LibraryError, Stage, StateDir, flash,
    library::FirmwareFiles,
};
use fujimod_catalog_configuration::{RunEngine, RunInputs};
use fujimod_catalog_controls::{BootCombination, BootMode, PhysicalControls};
use fujimod_catalog_remote::{Facts, Selection};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_boot::{self as boot, Camera, CardLayout};
use fujimod_emulator_cpu::ExecutionPolicy;
use fujimod_emulator_devices::parts::{nand::Nand, sd_card::Card};
use fujimod_emulator_session::Session;
use sha2::{Digest, Sha256};
use std::{
    fmt, fs,
    path::{Path, PathBuf},
};

/// Largest instruction budget an open run may execute.
const INSTRUCTION_LIMIT: u64 = 2_000_000_000;

/// Where a request's firmware files come from.
#[derive(Clone, Debug)]
pub enum FirmwareInput {
    /// `<firmware-id>/FPUPDATE.DAT` and `<body-id>/boot-rom.bin` in the library.
    Library,
    /// The two files, which must match the catalog's entry for the firmware.
    Files { update: PathBuf, boot_rom: PathBuf },
}

/// How a camera starts.
#[derive(Clone, Debug)]
pub enum Start {
    /// The boot ROM with nothing held.
    Rom,
    /// The boot ROM with a catalog power-on combination held: a name from the
    /// body's controls document such as `sd`, `firmware_update` or `service`.
    Held { combination: String },
    /// A verified image at address zero, starting at the native loader.
    Loader(LoaderImage),
}

impl Start {
    /// The combination this start holds, when it holds one.
    pub fn combination(&self) -> Option<&str> {
        match self {
            Self::Held { combination } => Some(combination),
            Self::Rom | Self::Loader(_) => None,
        }
    }
}

/// A caller-verified RAM image and its SHA-256.
#[derive(Clone)]
pub struct LoaderImage {
    pub bytes: Vec<u8>,
    /// SHA-256 of `bytes`, case-insensitive.
    pub sha256: String,
}

impl fmt::Debug for LoaderImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoaderImage")
            .field("bytes", &self.bytes.len())
            .field("sha256", &self.sha256)
            .finish()
    }
}

impl LoaderImage {
    /// A RAM image and its SHA-256, computed here.
    pub fn new(bytes: Vec<u8>) -> Self {
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        Self { bytes, sha256 }
    }
}

/// The camera's internal flash at power-on.
#[derive(Clone)]
pub enum Flash {
    /// The catalog's reference internal flash.
    Stock,
    /// A saved image of the catalog's stage-1 blocks.
    Image(Vec<u8>),
}

/// The SD card's contents and options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardInputs {
    /// The card's name and bytes pairs, or none.
    pub files: Vec<(String, Vec<u8>)>,
    /// Whether the card is inserted at power-on.
    pub present: bool,
    /// Whether the card's write-protect contact is asserted.
    pub protected: bool,
    /// Guest ticks the host charges for each SD data transfer; the run's
    /// catalog value when absent.
    pub transfer_ticks: Option<u64>,
    /// FAT geometry for a stock card, in place of the run's catalog layout.
    pub layout: Option<CardLayout>,
}

impl Default for CardInputs {
    fn default() -> Self {
        Self {
            files: Vec::new(),
            present: true,
            protected: false,
            transfer_ticks: None,
            layout: None,
        }
    }
}

/// One camera to open.
#[derive(Clone)]
pub struct Request {
    /// Catalog firmware identity, for example x70-110.
    pub firmware: String,
    /// Where the firmware's update and boot ROM come from.
    pub files: FirmwareInput,
    /// How the camera starts.
    pub start: Start,
    /// The internal flash it starts with.
    pub flash: Flash,
    /// Its SD card.
    pub card: CardInputs,
    pub policy: ExecutionPolicy,
    /// Scheduler engine override; the run configuration's engine otherwise.
    pub engine: Option<RunEngine>,
    /// Whether the open may restore a saved boot and keep a new one.
    pub cache: BootCache,
    /// Run the camera to REC before returning. When false, the open applies
    /// the boot holds and returns before the first instruction; the caller
    /// runs the session and calls [`Opened::release_holds`]. The boot cache is
    /// not used either way.
    pub run_to_rec: bool,
    /// Internal flash and SD card kept from an earlier open, for a power-on.
    pub kept: KeptMedia,
}

/// Media a request keeps from a camera's last use. Each part replaces what the
/// open would otherwise build.
#[derive(Clone, Default)]
pub struct KeptMedia {
    /// The medium the internal-flash controller last held.
    pub nand: Option<Nand>,
    /// The SD card's medium, including every file the camera wrote.
    pub card: Option<Card>,
}

/// A power-on that keeps a camera's internal flash and SD card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PowerOn {
    /// Hold this catalog power-on combination from power-on.
    pub hold: Option<String>,
    /// Put the open's card files on a fresh card instead of the one the camera
    /// had.
    pub fresh_card: bool,
    /// Run to REC before returning; otherwise stop before the first
    /// instruction with the boot holds applied.
    pub run_to_rec: bool,
}

impl Default for PowerOn {
    fn default() -> Self {
        Self {
            hold: None,
            fresh_card: false,
            run_to_rec: true,
        }
    }
}

/// Whether an open uses the boot cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootCache {
    /// Restore a saved boot with the same key; save this boot when none matches.
    Reuse,
    /// Boot the camera and leave the boot cache alone.
    Bypass,
}

/// Opening stopped before REC.
#[derive(Debug)]
pub struct OpenError {
    /// The furthest boot stage confirmed reached.
    pub stage: Stage,
    pub detail: String,
}

impl OpenError {
    pub fn at(stage: Stage, detail: impl Into<String>) -> Self {
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

/// A camera built from a request, at REC unless the request stopped before
/// the first instruction, with the facts it was opened with.
pub struct Opened {
    pub camera: Camera,
    pub firmware: String,
    pub start: Start,
    pub policy: ExecutionPolicy,
    pub engine: RunEngine,
    /// The boot stage the camera reached: `Stage::Rec` after a boot, or
    /// `Stage::BootRom` from `open_unbooted`.
    pub stage: Stage,
    /// Every file on the card.
    pub card_files: Vec<(String, Vec<u8>)>,
    /// The catalog resolution the camera was built from.
    pub resolved: ResolvedConfiguration,
    /// Whether the camera was restored from the boot cache instead of booted.
    pub cached: bool,
    /// The controls the start holds from power-on.
    pub holds: BootHolds,
    /// Whether the holds are applied and not yet released.
    holds_applied: bool,
    /// The request that opened the camera, for a later power-on.
    request: Request,
}

impl Opened {
    pub fn session(&self) -> &Session {
        &self.camera.session
    }

    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.camera.session
    }

    /// Hold the boot controls without running the guest.
    pub fn apply_holds(&mut self) -> Result<(), OpenError> {
        if self.holds_applied {
            return Ok(());
        }
        hold_boot_inputs(
            &mut self.camera.session,
            &self.holds.resolved,
            &self.holds.holds,
            self.holds.ticks_per_second,
        )?;
        self.holds_applied = true;
        Ok(())
    }

    /// Release the boot holds and connect the controls the running firmware
    /// loads. An open that ran to REC has already done this.
    pub fn release_holds(&mut self) -> Result<(), OpenError> {
        if !self.holds_applied {
            return Ok(());
        }
        self.holds.release(&mut self.camera.session)?;
        self.holds_applied = false;
        Ok(())
    }

    /// Whether the boot holds are applied and not yet released.
    pub fn holds_applied(&self) -> bool {
        self.holds_applied
    }

    /// Power the camera off and on again, keeping its internal flash and the
    /// SD card it last had. The new session starts from those media instead of
    /// the catalog's reference flash and the request's card files.
    pub fn power_on(
        &self,
        library: &Library,
        state: &StateDir,
        options: PowerOn,
    ) -> Result<Opened, OpenError> {
        let nand = self.camera.session.nand_medium("nand").map_err(|error| {
            OpenError::at(
                Stage::BootRom,
                format!("the camera keeps no internal flash: {error}"),
            )
        })?;
        let card = if options.fresh_card {
            None
        } else {
            Some(self.card_medium()?)
        };
        let start = match &options.hold {
            Some(combination) => Start::Held {
                combination: combination.clone(),
            },
            None => Start::Rom,
        };
        let mut request = self.request.clone();
        request.start = start;
        request.run_to_rec = options.run_to_rec;
        request.cache = BootCache::Bypass;
        request.kept = KeptMedia {
            nand: Some(nand),
            card,
        };
        open(library, state, request)
    }

    /// The SD card medium the camera currently holds.
    fn card_medium(&self) -> Result<Card, OpenError> {
        let name = self
            .camera
            .session
            .contacts
            .keys()
            .next()
            .cloned()
            .ok_or_else(|| {
                OpenError::at(Stage::BootRom, "the run declares no modeled SD card slot")
            })?;
        self.camera
            .session
            .card_medium(&name)
            .map_err(|error| OpenError::at(Stage::BootRom, error))
    }
}

/// The controls a start holds from power-on, and what releasing them needs.
#[derive(Clone)]
pub struct BootHolds {
    resolved: ResolvedConfiguration,
    holds: Vec<fujimod_catalog_controls::BootHold>,
    ticks_per_second: u64,
}

impl BootHolds {
    /// The catalog ids of the controls the start holds.
    pub fn controls(&self) -> impl Iterator<Item = &str> {
        self.holds.iter().map(|hold| hold.id.as_str())
    }

    /// Release each hold and connect the controls the running firmware loads.
    pub fn release(&self, session: &mut Session) -> Result<(), OpenError> {
        finish_boot(session, &self.resolved, &self.holds, self.ticks_per_second)
    }
}

/// Open a camera and run it to REC or its first-start setup. A request with
/// `run_to_rec` false builds it and applies its boot holds instead.
pub fn open(library: &Library, state: &StateDir, request: Request) -> Result<Opened, OpenError> {
    let resolved = resolve(library, &request)?;
    if !request.card.present && matches!(request.start, Start::Held { .. }) {
        return Err(OpenError::at(
            Stage::BootRom,
            "a held power-on needs the card present",
        ));
    }
    if matches!(request.start, Start::Loader(_)) && !matches!(request.flash, Flash::Stock) {
        return Err(OpenError::at(
            Stage::BootRom,
            "a RAM image starts at the loader from the catalog's flash",
        ));
    }
    if request.card.layout.is_some() && matches!(request.start, Start::Held { .. }) {
        return Err(OpenError::at(
            Stage::BootRom,
            "a card layout applies only to a start with nothing held",
        ));
    }
    if request.card.layout.is_some() && !request.card.present {
        return Err(OpenError::at(
            Stage::BootRom,
            "a card layout needs the card present",
        ));
    }
    if !request.run_to_rec {
        return open_held(library, request, resolved);
    }
    let key = match request.cache {
        BootCache::Reuse => Some(crate::cache::Key::new(state, &request, &resolved)?),
        BootCache::Bypass => None,
    };
    if let Some(key) = &key
        && let Some(opened) = crate::cache::restore(library, &request, &resolved, key)?
    {
        return Ok(opened);
    }
    let opened = open_booted(library, request, resolved)?;
    if let Some(key) = &key {
        crate::cache::save(&opened, key);
    }
    Ok(opened)
}

/// Build a camera, hold its boot controls and stop before the first instruction.
fn open_held(
    library: &Library,
    request: Request,
    resolved: ResolvedConfiguration,
) -> Result<Opened, OpenError> {
    let mut opened = construct(library, request, resolved)?;
    opened.apply_holds()?;
    Ok(opened)
}

/// The catalog resolution a request runs: the firmware's registered
/// configuration with the request's engine override applied.
fn resolve(library: &Library, request: &Request) -> Result<ResolvedConfiguration, OpenError> {
    fujimod_emulator_exr2_board::register_board_builder();
    fujimod_emulator_models::register();
    let mut resolved = library.resolved(&request.firmware)?;
    if let Some(engine) = request.engine {
        resolved
            .override_engine(engine)
            .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    }
    Ok(resolved)
}

fn open_booted(
    library: &Library,
    request: Request,
    resolved: ResolvedConfiguration,
) -> Result<Opened, OpenError> {
    let mut opened = construct(library, request, resolved)?;
    boot_camera(&mut opened)?;
    Ok(opened)
}

/// The controls a start holds from power-on, from the catalog's boot entry.
fn boot_holds(
    start: &Start,
    resolved: &ResolvedConfiguration,
) -> Result<Vec<fujimod_catalog_controls::BootHold>, OpenError> {
    match start {
        Start::Rom | Start::Loader(_) => Ok(Vec::new()),
        Start::Held { combination } => combination_holds(combination, resolved),
    }
}

/// Run a constructed camera to REC, holding and releasing its controls around it.
fn boot_camera(opened: &mut Opened) -> Result<(), OpenError> {
    opened.apply_holds()?;
    let card_before = card_revision(&opened.camera.session, &opened.start);
    let report = boot::to_rec(&mut opened.camera.session, INSTRUCTION_LIMIT)
        .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    if report.status == "rec" && !report.pending && report.scanout_instance.is_some() {
        opened.release_holds()?;
        opened.stage = Stage::Rec;
        return Ok(());
    }
    let stage = reached_stage(
        &opened.camera.session,
        &opened.resolved,
        &opened.start,
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

/// Build a camera's board and media without running its guest. A checkpoint
/// restores into the session this returns, built from the same catalog facts
/// the boot would use.
pub fn open_unbooted(library: &Library, request: Request) -> Result<Opened, OpenError> {
    let resolved = resolve(library, &request)?;
    construct(library, request, resolved)
}

pub(crate) fn construct(
    library: &Library,
    request: Request,
    resolved: ResolvedConfiguration,
) -> Result<Opened, OpenError> {
    let opened_request = request.clone();
    let Request {
        firmware,
        files,
        start,
        flash,
        card,
        policy,
        engine: _,
        cache: _,
        run_to_rec: _,
        kept,
    } = request;
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
    let files = firmware_files(library, &firmware, &files)?;
    let camera_directory = camera_directory(library, &resolved)?;
    let mut camera = boot::open_request(
        resolved.clone(),
        boot::Request {
            update: files.update,
            boot_rom: Some(files.boot_rom),
            nand: None,
            settings: None,
            camera: camera_directory,
            application_plan: None,
            card_files: Vec::new(),
            discovery: false,
            sd_card_transfer_ticks: card.transfer_ticks,
            card_layout: card.layout.clone(),
        },
    )
    .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    camera
        .session
        .board
        .set_execution_policy(policy)
        .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    install_files(&mut camera.session, &card.files)?;
    apply_kept_media(&mut camera.session, kept)?;
    if let Flash::Image(bytes) = &flash {
        flash::write(&mut camera.session, library.nand_boot(&firmware)?, bytes)?;
    }
    apply_card_contacts(&mut camera.session, &card)?;
    if let Start::Loader(image) = start.clone() {
        place_loader_image(&mut camera.session, image)?;
    }
    let holds = BootHolds {
        resolved: resolved.clone(),
        holds: boot_holds(&start, &resolved)?,
        ticks_per_second: camera.ticks_per_second,
    };
    Ok(Opened {
        camera,
        firmware,
        start,
        policy,
        engine,
        stage: Stage::BootRom,
        card_files: card.files,
        resolved,
        cached: false,
        holds,
        holds_applied: false,
        request: opened_request,
    })
}

/// The catalog-named files a request's firmware resolves to, hashed once.
fn firmware_files(
    library: &Library,
    firmware: &str,
    input: &FirmwareInput,
) -> Result<FirmwareFiles, OpenError> {
    match input {
        FirmwareInput::Library => Ok(library.files(firmware)?),
        FirmwareInput::Files { update, boot_rom } => {
            let catalog = library.firmware(firmware)?;
            let update_sha256 = confirm(update, firmware, "stock update", catalog)?;
            let boot_rom_sha256 = confirm(boot_rom, firmware, "boot ROM", catalog)?;
            Ok(FirmwareFiles {
                update: update.clone(),
                update_sha256,
                boot_rom: boot_rom.clone(),
                boot_rom_sha256,
            })
        }
    }
}

/// Read one explicit file and confirm it is the catalog's entry.
fn confirm(
    path: &Path,
    firmware: &str,
    kind: &'static str,
    catalog: &fujimod_catalog::Firmware,
) -> Result<String, OpenError> {
    let expected = match kind {
        "stock update" => catalog.update.as_ref().map(|update| update.sha256.as_str()),
        _ => catalog
            .boot_rom
            .as_ref()
            .map(|rom| rom.content.sha256.as_str()),
    }
    .ok_or_else(|| {
        OpenError::at(
            Stage::BootRom,
            format!("firmware {firmware} has no {kind} identity"),
        )
    })?;
    let bytes = fs::read(path)
        .map_err(|error| OpenError::at(Stage::BootRom, format!("{}: {error}", path.display())))?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    if sha256 != expected {
        return Err(OpenError::at(
            Stage::BootRom,
            format!(
                "{}: sha256 {sha256} is not the catalog's {kind} for {firmware}",
                path.display()
            ),
        ));
    }
    Ok(sha256)
}

/// Restore the media a power-on keeps: internal flash and the SD card medium.
fn apply_kept_media(session: &mut Session, kept: KeptMedia) -> Result<(), OpenError> {
    if let Some(nand) = kept.nand {
        session.retain_nand("nand", nand).map_err(|error| {
            OpenError::at(Stage::BootRom, format!("retained internal flash: {error}"))
        })?;
    }
    if let Some(card) = kept.card {
        let name = session.contacts.keys().next().cloned().ok_or_else(|| {
            OpenError::at(Stage::BootRom, "the run declares no modeled SD card slot")
        })?;
        session
            .insert_card(&name, card, true, false)
            .map(|_| ())
            .map_err(|error| OpenError::at(Stage::BootRom, format!("retained SD card: {error}")))?;
    }
    Ok(())
}

/// Place a caller-verified image at address zero and start the first core at
/// the native loader, before any instruction runs.
fn place_loader_image(session: &mut Session, image: LoaderImage) -> Result<(), OpenError> {
    let digest = format!("{:x}", Sha256::digest(&image.bytes));
    if digest != image.sha256.to_ascii_lowercase() {
        return Err(OpenError::at(
            Stage::BootRom,
            format!("RAM image sha256 is {digest}, not {}", image.sha256),
        ));
    }
    session
        .board
        .write_physical(0, &image.bytes)
        .map_err(|error| OpenError::at(Stage::BootRom, format!("RAM image: {error}")))?;
    session
        .board
        .write_register(0, fujimod_emulator_cpu::arm::CORE[15], 0)
        .map_err(|error| OpenError::at(Stage::BootRom, format!("loader entry: {error}")))?;
    Ok(())
}

/// Put the caller's card into the modeled slot: its presence and write
/// protection, after every source has installed its files.
fn apply_card_contacts(session: &mut Session, card: &CardInputs) -> Result<(), OpenError> {
    if card.present && !card.protected {
        return Ok(());
    }
    let name =
        session.contacts.keys().next().cloned().ok_or_else(|| {
            OpenError::at(Stage::BootRom, "the run declares no modeled SD card slot")
        })?;
    let medium = session
        .card_medium(&name)
        .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    session
        .insert_card(&name, medium, card.present, card.protected)
        .map(|_| ())
        .map_err(|error| OpenError::at(Stage::BootRom, error))
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
            let driver = session
                .contact_driver
                .as_ref()
                .ok_or_else(|| OpenError::at(Stage::Rec, "boot holds lost the contact driver"))?;
            let control = driver
                .hold_control(&hold.id)
                .map_err(|error| OpenError::at(Stage::Rec, error))?;
            let position = driver
                .boot_release_position(&hold.id)
                .map_err(|error| OpenError::at(Stage::Rec, error))?;
            session
                .set_position(&control, &position)
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

/// The boot combination a name selects from the body's controls document.
fn boot_mode(combination: &str) -> Result<BootMode, OpenError> {
    serde_json::from_value(serde_json::Value::String(combination.to_owned())).map_err(|_| {
        OpenError::at(
            Stage::BootRom,
            format!(
                "unknown power-on combination {combination:?}; use sd, firmware_update or service"
            ),
        )
    })
}

/// The body controls document's hold list for a named combination.
fn combination_holds(
    combination: &str,
    resolved: &ResolvedConfiguration,
) -> Result<Vec<fujimod_catalog_controls::BootHold>, OpenError> {
    let mode = boot_mode(combination)?;
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
    let combination: &BootCombination = controls.boot.get(&mode).ok_or_else(|| {
        OpenError::at(
            Stage::BootRom,
            format!(
                "body {} declares no {} power-on controls",
                resolved.reference.body.id, combination
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
        let driver = session
            .contact_driver
            .as_ref()
            .ok_or_else(|| OpenError::at(Stage::BootRom, "boot holds need the contact driver"))?;
        let control = driver
            .hold_control(&hold.id)
            .map_err(|error| OpenError::at(Stage::BootRom, error))?;
        let position = driver
            .boot_position(&hold.id, hold.stage.as_deref())
            .map_err(|error| OpenError::at(Stage::BootRom, error))?;
        session
            .set_position(&control, &position)
            .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    }
    Ok(())
}

fn card_revision(session: &Session, start: &Start) -> Option<u64> {
    if !matches!(start, Start::Held { .. }) {
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
    start: &Start,
    card_before: Option<u64>,
    report: &boot::RecReport,
) -> Stage {
    if report.instructions == 0 {
        return Stage::BootRom;
    }
    let held = matches!(start, Start::Held { .. });
    if held
        && let Some(before) = card_before
        && card_revision(session, start).is_some_and(|after| after != before)
    {
        return Stage::FujiModApplied;
    }
    let loaded = configuration_loaded(session, resolved);
    if held {
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

/// Read every file under a directory, named by its path relative to it.
pub fn directory_files(directory: &Path) -> Result<Vec<(String, Vec<u8>)>, OpenError> {
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
                continue;
            }
            let name = path
                .strip_prefix(directory)
                .map_err(|error| OpenError::at(Stage::BootRom, error.to_string()))?
                .to_str()
                .ok_or_else(|| OpenError::at(Stage::BootRom, "SD card file name is not UTF-8"))?
                .replace('\\', "/");
            let bytes = fs::read(&path).map_err(|error| {
                OpenError::at(Stage::BootRom, format!("{}: {error}", path.display()))
            })?;
            files.push((name, bytes));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
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
