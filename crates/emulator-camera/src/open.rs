//! Turn a body, a firmware choice, media and a start choice into a camera at
//! REC.
//!
//! This is the emulator layer: it knows cameras, not FujiMod. A request names
//! the body and optionally its firmware, the firmware's files (the library's,
//! or two named files), the internal flash, the storage cards, the session's
//! settings and how the camera starts: from the boot ROM with nothing held,
//! from the boot ROM with a catalog power-on combination held, or at the
//! native loader with a caller-verified RAM image. The body and firmware
//! resolve to the firmware's canonical run configuration, and the
//! configuration's processor decides the startup path. Opening writes nothing
//! to disk unless the caller asked for the boot cache.
use crate::{
    FirmwareChoice, Library, LibraryError, Stage, StateDir, flash,
    library::{FirmwareFiles, processor_name},
};
use fujimod_catalog::reference::StorageSlotId;
use fujimod_catalog_configuration::{RunEngine, RunInputs};
use fujimod_catalog_controls::{BootCombination, BootMode, PhysicalControls};
use fujimod_catalog_remote::{Facts, Selection};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_catalog_run_inputs::RunFaceResultProfile;
use fujimod_emulator_boot::{self as boot, BootRomUse, Camera, CardLayout, StartupPath};
use fujimod_emulator_cpu::ExecutionPolicy;
use fujimod_emulator_devices::parts::{nand::Nand, sd_card::Card as CardMedium};
use fujimod_emulator_session::Session;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
};

/// Largest instruction budget an open run may execute.
const INSTRUCTION_LIMIT: u64 = 2_000_000_000;
/// The instructions of all cores an X-Processor 5 open runs to REC: its four
/// cores reach REC at about 1.5 billion each.
const XP5_INSTRUCTION_LIMIT: u64 = 12_000_000_000;

/// Where a request's firmware files come from.
#[derive(Clone, Debug)]
pub enum FirmwareInput {
    /// The library's records for the firmware.
    Library,
    /// Named files, which must match the catalog's entry for the firmware.
    /// The boot ROM is needed only by a start that runs it.
    Files {
        update: PathBuf,
        boot_rom: Option<PathBuf>,
    },
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

/// One file on a card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardFile {
    /// A host file, read when the camera is built.
    Source(PathBuf),
    /// Bytes the host already holds.
    Bytes(Vec<u8>),
}

/// What a card holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardContents {
    /// A freshly formatted card holding these files, by their full path on
    /// the card. Empty is a blank card.
    Files(BTreeMap<String, CardFile>),
    /// A raw card image, read without being written.
    Image(PathBuf),
}

/// A card in one of the body's storage slots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    /// Whether the card is in the slot at power-on.
    pub present: bool,
    /// Whether the card's write-protect switch is set.
    pub write_protected: bool,
    pub contents: CardContents,
    /// FAT geometry for a freshly formatted card, in place of the run's.
    pub layout: Option<CardLayout>,
    /// Guest ticks the host charges for each SD data transfer; the run's
    /// catalog value when absent.
    pub transfer_ticks: Option<u64>,
    /// The modeled SD host's initialization fault, as the device model names
    /// it; none otherwise.
    pub initialization_fault: Option<serde_json::Value>,
}

impl Default for Card {
    fn default() -> Self {
        Self {
            present: true,
            write_protected: false,
            contents: CardContents::Files(BTreeMap::new()),
            layout: None,
            transfer_ticks: None,
            initialization_fault: None,
        }
    }
}

impl Card {
    /// A present card holding these files.
    pub fn with_files(files: impl IntoIterator<Item = (String, Vec<u8>)>) -> Self {
        Self {
            contents: CardContents::Files(
                files
                    .into_iter()
                    .map(|(name, bytes)| (name, CardFile::Bytes(bytes)))
                    .collect(),
            ),
            ..Self::default()
        }
    }

    /// Whether the card holds files a host named.
    fn has_contents(&self) -> bool {
        match &self.contents {
            CardContents::Files(files) => !files.is_empty(),
            CardContents::Image(_) => true,
        }
    }

    /// The card's files read into memory; an image card has none.
    pub fn read_files(&self) -> Result<Vec<(String, Vec<u8>)>, OpenError> {
        let CardContents::Files(files) = &self.contents else {
            return Ok(Vec::new());
        };
        files
            .iter()
            .map(|(name, file)| match file {
                CardFile::Bytes(bytes) => Ok((name.clone(), bytes.clone())),
                CardFile::Source(path) => fs::read(path)
                    .map(|bytes| (name.clone(), bytes))
                    .map_err(|error| {
                        OpenError::at(Stage::BootRom, format!("{}: {error}", path.display()))
                    }),
            })
            .collect()
    }
}

/// The cards in the body's storage slots. An omitted SD card is one blank
/// card in the first slot; the other slots stay empty unless named.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cards {
    pub sd_card: Option<Card>,
    pub sd_card_2: Option<Card>,
    pub cfexpress_card: Option<Card>,
}

impl Cards {
    /// The card in the first SD slot, as the open builds it.
    pub fn first(&self) -> Card {
        self.sd_card.clone().unwrap_or_default()
    }

    /// The named slots, with the first SD slot always present.
    fn named(&self) -> Vec<(StorageSlotId, Card)> {
        let mut slots = vec![(StorageSlotId::SdCard, self.first())];
        if let Some(card) = &self.sd_card_2 {
            slots.push((StorageSlotId::SdCard2, card.clone()));
        }
        if let Some(card) = &self.cfexpress_card {
            slots.push((StorageSlotId::CfexpressCard, card.clone()));
        }
        slots
    }
}

/// One camera to open.
#[derive(Clone)]
pub struct Request {
    /// Catalog body identity, for example xh2s.
    pub body: String,
    /// The body's firmware.
    pub firmware: FirmwareChoice,
    /// Where the firmware's update and boot ROM come from.
    pub files: FirmwareInput,
    /// The application startup plan an X-Processor 4 or X-Processor Pro
    /// firmware starts from.
    pub application_plan: Option<PathBuf>,
    /// Catalog overlays of the firmware's run configuration.
    pub overlays: Vec<String>,
    /// How the camera starts.
    pub start: Start,
    /// The internal flash it starts with.
    pub flash: Flash,
    /// Its storage cards.
    pub cards: Cards,
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
    /// A saved internal-flash image, restored before the first instruction.
    pub nand_image: Option<Vec<u8>>,
    /// External face detector results, in place of the run's passive face
    /// bank: a declared result the guest's own detector start consumes.
    pub face_result_profile: Option<RunFaceResultProfile>,
    /// Named camera settings, such as `language: en`, written through the
    /// firmware's settings record before the first instruction.
    pub settings: BTreeMap<String, String>,
    /// The camera clock at power-on, in seconds since 2000-01-01 00:00.
    pub clock_seconds_2000: Option<u32>,
    /// A scene document the sensor sees, in place of the run's.
    pub scene: Option<PathBuf>,
    /// Internal flash and SD card kept from an earlier open, for a power-on.
    pub kept: KeptMedia,
}

impl Request {
    /// A stock open of a body's firmware from the library, with every other
    /// choice at its default.
    pub fn new(body: impl Into<String>, firmware: FirmwareChoice) -> Self {
        Self {
            body: body.into(),
            firmware,
            files: FirmwareInput::Library,
            application_plan: None,
            overlays: Vec::new(),
            start: Start::Rom,
            flash: Flash::Stock,
            cards: Cards::default(),
            policy: ExecutionPolicy::Exact,
            engine: None,
            cache: BootCache::Bypass,
            run_to_rec: true,
            nand_image: None,
            face_result_profile: None,
            settings: BTreeMap::new(),
            clock_seconds_2000: None,
            scene: None,
            kept: KeptMedia::default(),
        }
    }
}

/// Media a request keeps from a camera's last use. Each part replaces what the
/// open would otherwise build.
#[derive(Clone, Default)]
pub struct KeptMedia {
    /// The medium the internal-flash controller last held.
    pub nand: Option<Nand>,
    /// The SD card's medium, including every file the camera wrote.
    pub card: Option<CardMedium>,
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

/// What a request resolves to before any firmware byte is read: the firmware,
/// its run configuration and the startup path its processor takes.
#[derive(Clone)]
pub struct Plan {
    /// The firmware identity the body and firmware choice select.
    pub firmware: String,
    /// The firmware's canonical run configuration with the request's choices
    /// applied.
    pub resolved: ResolvedConfiguration,
    /// The startup path of the configuration's processor.
    pub path: StartupPath,
    /// Whether the start reads the boot ROM.
    pub boot_rom: BootRomUse,
    /// Why the open cannot observe REC, when the firmware declares no REC
    /// readiness facts. Such a camera is built and returned before its first
    /// instruction.
    pub rec_unobserved: Option<String>,
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
    /// Every file the request put on the first SD card.
    pub card_files: Vec<(String, Vec<u8>)>,
    /// The catalog resolution the camera was built from.
    pub resolved: ResolvedConfiguration,
    /// Whether the camera was restored from the boot cache instead of booted.
    pub cached: bool,
    /// The controls the start holds from power-on.
    pub holds: BootHolds,
    /// Why the camera was returned before its first instruction although the
    /// request asked for REC: the firmware declares no REC readiness facts.
    pub rec_unobserved: Option<String>,
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

    /// Where the firmware files came from, so a power-on reads the same ones.
    pub fn firmware_input(&self) -> &FirmwareInput {
        &self.request.files
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
    fn card_medium(&self) -> Result<CardMedium, OpenError> {
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

/// Resolve a request to the point just before any firmware byte is read: the
/// firmware the body and choice select, its canonical run configuration with
/// the request's choices applied, and the processor's startup path. Every
/// refusal names the gap that stops the request.
pub fn plan(library: &Library, request: &Request) -> Result<Plan, OpenError> {
    fujimod_emulator_exr2_board::register_board_builder();
    fujimod_emulator_models::register();
    let firmware = library.select(&request.body, &request.firmware)?;
    let mut resolved = library.resolved(&firmware, &request.overlays)?;
    let refuse = |detail: String| OpenError::at(Stage::BootRom, detail);
    let path = boot::startup_path(&resolved).map_err(refuse)?;
    let processor = processor_name(&resolved.reference.soc.id);
    if path == StartupPath::Application && request.application_plan.is_none() {
        return Err(refuse(format!(
            "firmware {firmware} starts from an application startup plan, which the catalog \
             does not hold yet; name one in application_plan"
        )));
    }
    let boot_rom = boot::boot_rom_use(&resolved).map_err(refuse)?;
    if boot_rom == BootRomUse::Required && resolved.reference.firmware.boot_rom.is_none() {
        return Err(refuse(format!(
            "firmware {firmware} starts from the boot ROM, and the catalog has no boot ROM identity for it"
        )));
    }
    if let Some(engine) = request.engine {
        resolved.override_engine(engine).map_err(refuse)?;
    }
    if let Some(profile) = request.face_result_profile.clone() {
        let preset = resolved.configuration.preset.as_mut().ok_or_else(|| {
            refuse("a face result profile needs a run configuration with a preset".into())
        })?;
        preset.factory_options.face_result_profile = Some(profile);
    }
    if path == StartupPath::Exr2 && !request.settings.is_empty() {
        let settings = match resolved.inputs.as_mut() {
            Some(RunInputs::Exr2(inputs)) => inputs.settings.as_mut(),
            _ => resolved
                .configuration
                .preset
                .as_mut()
                .and_then(|preset| preset.settings.as_mut()),
        }
        .ok_or_else(|| {
            refuse(format!(
                "firmware {firmware} has no power-on settings record for camera settings"
            ))
        })?;
        settings.requested.extend(request.settings.clone());
    }
    if request.clock_seconds_2000.is_some() && path != StartupPath::Exr2 {
        return Err(refuse(format!(
            "the {processor} camera clock is not modeled yet"
        )));
    }
    if request.scene.is_some() && path == StartupPath::Application {
        return Err(refuse(format!(
            "the {processor} application start takes no scene document yet"
        )));
    }
    check_cards(library, &request.body, &request.cards, path, &processor)?;
    if matches!(request.start, Start::Loader(_)) && path != StartupPath::Exr2 {
        return Err(refuse(format!(
            "a native loader start with a RAM image is modeled for EXR II only, not {processor}"
        )));
    }
    if (!matches!(request.flash, Flash::Stock) || request.nand_image.is_some())
        && path != StartupPath::Exr2
    {
        return Err(refuse(format!(
            "an internal-flash image is modeled for EXR II only, not {processor}"
        )));
    }
    let observed = match path {
        StartupPath::Exr2 => resolved
            .reference
            .firmware
            .generated_facts
            .as_ref()
            .is_some_and(|facts| facts.observation.is_some()),
        StartupPath::Xp5 => resolved.reference.rec_readiness.is_some(),
        StartupPath::Application => false,
    };
    let rec_unobserved = (!observed).then(|| {
        format!(
            "firmware {firmware} declares no REC readiness facts, so the open cannot observe \
             REC; the camera is built and returned before its first instruction"
        )
    });
    Ok(Plan {
        firmware,
        resolved,
        path,
        boot_rom,
        rec_unobserved,
    })
}

/// Refuse a card in a slot the body lacks, a slot the emulator does not model
/// yet, or an option the processor's card does not take.
fn check_cards(
    library: &Library,
    body_id: &str,
    cards: &Cards,
    path: StartupPath,
    processor: &str,
) -> Result<(), OpenError> {
    let refuse = |detail: String| OpenError::at(Stage::BootRom, detail);
    let body = library.body(body_id)?;
    let slots: Vec<StorageSlotId> = body.storage_slots.iter().map(|slot| slot.id).collect();
    let names = || {
        if slots.is_empty() {
            "none".to_owned()
        } else {
            slots
                .iter()
                .map(|slot| slot.name())
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    for (slot, card) in [
        (StorageSlotId::SdCard, cards.sd_card.as_ref()),
        (StorageSlotId::SdCard2, cards.sd_card_2.as_ref()),
        (StorageSlotId::CfexpressCard, cards.cfexpress_card.as_ref()),
    ] {
        let Some(card) = card else { continue };
        if !slots.contains(&slot) {
            return Err(refuse(format!(
                "body {body_id} has no {} slot; its slots: {}",
                slot.name(),
                names()
            )));
        }
        if slot == StorageSlotId::CfexpressCard {
            return Err(refuse("CFexpress is not modeled yet".into()));
        }
        if (card.layout.is_some()
            || card.transfer_ticks.is_some()
            || card.initialization_fault.is_some())
            && path != StartupPath::Exr2
        {
            return Err(refuse(format!(
                "card layout, transfer ticks and initialization faults are modeled for EXR II only, not {processor}"
            )));
        }
        if card.layout.is_some() && !card.present {
            return Err(refuse("a card layout needs the card present".into()));
        }
        if card.layout.is_some() && matches!(card.contents, CardContents::Image(_)) {
            return Err(refuse("a card image has its own layout".into()));
        }
        if let Some(layout) = &card.layout {
            layout.validate().map_err(refuse)?;
        }
        if card.transfer_ticks == Some(0) {
            return Err(refuse("card transfer ticks must be positive".into()));
        }
        if !card.present && card.has_contents() {
            return Err(refuse(format!(
                "the {} is absent, so it holds no files",
                slot.name()
            )));
        }
    }
    Ok(())
}

/// Open a camera and run it to REC or its first-start setup. A request with
/// `run_to_rec` false builds it and applies its boot holds instead, as does a
/// firmware that declares no REC readiness facts.
pub fn open(library: &Library, state: &StateDir, request: Request) -> Result<Opened, OpenError> {
    let plan = plan(library, &request)?;
    let first = request.cards.first();
    if !first.present && matches!(request.start, Start::Held { .. }) {
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
    if !request.run_to_rec || plan.rec_unobserved.is_some() {
        return open_held(library, request, plan);
    }
    let key = match request.cache {
        BootCache::Reuse => Some(crate::cache::Key::new(state, &request, &plan)?),
        BootCache::Bypass => None,
    };
    if let Some(key) = &key
        && let Some(opened) = crate::cache::restore(library, &request, &plan, key)?
    {
        return Ok(opened);
    }
    let opened = open_booted(library, request, plan)?;
    if let Some(key) = &key {
        crate::cache::save(&opened, key);
    }
    Ok(opened)
}

/// Build a camera, hold its boot controls and stop before the first instruction.
fn open_held(library: &Library, request: Request, plan: Plan) -> Result<Opened, OpenError> {
    let mut opened = construct(library, request, plan)?;
    opened.apply_holds()?;
    Ok(opened)
}

fn open_booted(library: &Library, request: Request, plan: Plan) -> Result<Opened, OpenError> {
    let mut opened = construct(library, request, plan)?;
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
    let report = match &opened.resolved.reference.rec_readiness {
        Some(readiness) => boot::xp5_rec::to_rec(
            &mut opened.camera.session,
            readiness,
            opened.camera.quantum,
            opened.camera.ticks_per_second,
            XP5_INSTRUCTION_LIMIT,
        ),
        None => boot::to_rec(&mut opened.camera.session, INSTRUCTION_LIMIT),
    }
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
    let plan = plan(library, &request)?;
    construct(library, request, plan)
}

/// A request card as the startup path inserts it.
fn card_insert(card: &Card) -> boot::media::CardInsert {
    let (image, files) = match &card.contents {
        CardContents::Image(path) => (Some(path.clone()), Vec::new()),
        CardContents::Files(files) => (
            None,
            files
                .iter()
                .map(|(name, file)| {
                    (
                        name.clone(),
                        match file {
                            CardFile::Source(path) => boot::media::CardFile::Path(path.clone()),
                            CardFile::Bytes(bytes) => boot::media::CardFile::Bytes(bytes.clone()),
                        },
                    )
                })
                .collect(),
        ),
    };
    boot::media::CardInsert {
        present: card.present,
        write_protected: card.write_protected,
        image,
        files,
    }
}

pub(crate) fn construct(
    library: &Library,
    request: Request,
    plan: Plan,
) -> Result<Opened, OpenError> {
    let opened_request = request.clone();
    let Plan {
        firmware,
        resolved,
        path,
        boot_rom,
        rec_unobserved,
    } = plan;
    let refuse = |error: String| OpenError::at(Stage::BootRom, error);
    let engine = match resolved.configuration.execution {
        Some(execution) => execution.engine,
        None if path == StartupPath::Exr2 => {
            return Err(refuse(format!(
                "{} declares no execution policy",
                resolved.configuration.id
            )));
        }
        None => RunEngine::Unicorn,
    };
    let read_rom = match boot_rom {
        BootRomUse::Required => true,
        BootRomUse::Optional => resolved.reference.firmware.boot_rom.is_some(),
        BootRomUse::Unused => false,
    };
    let files = firmware_files(library, &firmware, &request.files, read_rom)?;
    let camera_directory = settings_camera(library, &resolved)?;
    let first = request.cards.first();
    // EXR II formats its card after the board opens, so the open puts the
    // card in itself, after any kept media; the other paths insert theirs
    // before the board is built.
    let mut cards = BTreeMap::new();
    if path != StartupPath::Exr2 {
        for (slot, card) in request.cards.named() {
            let number = match slot {
                StorageSlotId::SdCard => 1,
                StorageSlotId::SdCard2 => 2,
                StorageSlotId::CfexpressCard => continue,
            };
            // An omitted card on these paths keeps the slot as the startup
            // declares it.
            if slot == StorageSlotId::SdCard && request.cards.sd_card.is_none() {
                continue;
            }
            cards.insert(number, card_insert(&card));
        }
    }
    let mut camera = boot::open_request(
        resolved.clone(),
        boot::Request {
            update: boot::FirmwareFile::Bytes(files.update),
            boot_rom: files.boot_rom.map(boot::FirmwareFile::Bytes),
            nand: None,
            settings: None,
            camera: camera_directory,
            application_plan: request.application_plan.clone(),
            cards,
            discovery: false,
            sd_card_transfer_ticks: first.transfer_ticks,
            card_layout: first.layout.clone(),
            card_initialization_fault: first.initialization_fault.clone(),
            camera_settings: if path == StartupPath::Exr2 {
                BTreeMap::new()
            } else if request.settings.is_empty() {
                // The run's power-on settings are the session default.
                resolved
                    .configuration
                    .preset
                    .as_ref()
                    .and_then(|preset| preset.settings.as_ref())
                    .map(|settings| settings.requested.clone())
                    .unwrap_or_default()
            } else {
                request.settings.clone()
            },
            clock_seconds_2000: request.clock_seconds_2000,
            scene: request.scene.clone(),
        },
    )
    .map_err(refuse)?;
    camera
        .session
        .board
        .set_execution_policy(request.policy)
        .map_err(refuse)?;
    let mut card_files = Vec::new();
    if path == StartupPath::Exr2 {
        card_files = first.read_files()?;
        let image = match &first.contents {
            CardContents::Image(path) => Some(boot::media::read_image(path).map_err(refuse)?),
            CardContents::Files(_) => None,
        };
        install_files(&mut camera.session, image, &card_files)?;
    }
    if let Some(bytes) = request.nand_image.as_ref() {
        let medium = camera.session.nand_medium("nand").map_err(refuse)?;
        let restored = medium.restore_committed(bytes).map_err(|error| {
            OpenError::at(Stage::BootRom, format!("saved internal flash: {error}"))
        })?;
        camera
            .session
            .retain_nand("nand", restored)
            .map_err(|error| {
                OpenError::at(Stage::BootRom, format!("saved internal flash: {error}"))
            })?;
    }
    // A power-on's kept medium replaces the flash this request would build.
    apply_kept_media(&mut camera.session, request.kept.clone())?;
    if let Flash::Image(bytes) = &request.flash {
        flash::write(&mut camera.session, library.nand_boot(&firmware)?, bytes)?;
    }
    if path == StartupPath::Exr2 {
        apply_card_contacts(&mut camera.session, &first)?;
    }
    if let Start::Loader(image) = request.start.clone() {
        place_loader_image(&mut camera.session, image)?;
    }
    let holds = BootHolds {
        resolved: resolved.clone(),
        holds: boot_holds(&request.start, &resolved)?,
        ticks_per_second: camera.ticks_per_second,
    };
    Ok(Opened {
        camera,
        firmware,
        start: request.start,
        policy: request.policy,
        engine,
        stage: Stage::BootRom,
        card_files,
        resolved,
        cached: false,
        holds,
        rec_unobserved,
        holds_applied: false,
        request: opened_request,
    })
}

/// The catalog-named files a request's firmware resolves to, hashed once.
fn firmware_files(
    library: &Library,
    firmware: &str,
    input: &FirmwareInput,
    boot_rom: bool,
) -> Result<FirmwareFiles, OpenError> {
    match input {
        FirmwareInput::Library => Ok(library.files(firmware, boot_rom)?),
        FirmwareInput::Files {
            update,
            boot_rom: named,
        } => {
            let rom = match (boot_rom, named) {
                (true, Some(path)) => Some(path.as_path()),
                (true, None) => {
                    return Err(OpenError::at(
                        Stage::BootRom,
                        format!("firmware {firmware} starts from the boot ROM; name its file"),
                    ));
                }
                (false, _) => None,
            };
            Ok(library.files_from(firmware, update, rom)?)
        }
    }
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
fn apply_card_contacts(session: &mut Session, card: &Card) -> Result<(), OpenError> {
    if card.present && !card.write_protected {
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
        .insert_card(&name, medium, card.present, card.write_protected)
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

fn install_files(
    session: &mut Session,
    image: Option<CardMedium>,
    files: &[(String, Vec<u8>)],
) -> Result<(), OpenError> {
    if image.is_none() && files.is_empty() {
        return Ok(());
    }
    let name =
        session.contacts.keys().next().cloned().ok_or_else(|| {
            OpenError::at(Stage::BootRom, "the run declares no modeled SD card slot")
        })?;
    session
        .install_card_files(&name, image, files)
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

/// The directory of the body camera whose NAND sections a run's settings name, when
/// they name one.
pub fn settings_camera(
    library: &Library,
    resolved: &ResolvedConfiguration,
) -> Result<Option<PathBuf>, OpenError> {
    let camera = match resolved.inputs.as_ref() {
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
