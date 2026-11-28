//! Open an emulated camera with FujiMod: the FujiMod layer.
//!
//! A preset turns into emulator inputs:
//!
//! | Preset | Internal flash | SD card | Combination |
//! |---|---|---|---|
//! | Stock | Stock | None, or files the caller adds | None |
//! | SD card boot | Stock | The FujiMod SD card | `sd` |
//! | Autostart | Stock with the internal-flash stage 1 installed | The FujiMod SD card | None |
//!
//! The FujiMod SD card is a folder or files the caller gives, or the published
//! release SD card for the firmware, downloaded once into the state directory.
//! Autostart flash is made once per firmware and SD card the way an owner makes
//! it: an SD card boot, then FUJIMOD SET-UP > BOOT SETUP > AUTOMATIC, and the
//! camera powers off. After an SD card boot or autostart reaches REC, this
//! layer confirms FujiMod is in RAM and that the release was confirmed.
//!
//! Anything that wants a camera without FujiMod calls the emulator layer
//! directly.
mod autostart;
mod release;

use fujimod_emulator_camera as emulator;
use fujimod_emulator_camera::BootPreset;
use fujimod_emulator_camera::{Opened as EmulatorOpened, Request as EmulatorRequest, Start};
use fujimod_emulator_session::Session;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

pub use autostart::stock_menus;
pub use emulator::{
    Action, ActionInput, Actions, BootCache, CameraDescription, Card, CardContents, CardFile,
    CardLayout, Cards, ExecutionPolicy, ExecutionPolicyName, FirmwareChoice, FirmwareInput, Flash,
    Library, LibraryError, LibraryFirmware, LoaderImage, OpenError, PowerOn as EmulatorPowerOn,
    Request as EmulatorRequestBase, RunEngine, RunFaceResultProfile, Stage, StateDir,
    directory_files, plan, read_flash,
};
pub use release::{RELEASES_URL, card as release_card};

/// Guest instructions a FujiMod release may take to confirm after REC.
const CONFIRM_INSTRUCTION_LIMIT: u64 = 1_000_000_000;

/// Instructions per observation while waiting for a release confirmation.
const CONFIRM_WINDOW: u64 = 50_000_000;

/// How a camera starts with FujiMod.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// The firmware installed in internal flash.
    Stock,
    /// FujiMod loaded from the SD card by the boot ROM.
    SdCard,
    /// FujiMod from the SD card, started by the autostart installer.
    Autostart,
}

impl Preset {
    /// The preset's name in callers: `stock`, `sd-card` or `autostart`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Stock => "stock",
            Self::SdCard => "sd-card",
            Self::Autostart => "autostart",
        }
    }

    /// Parse a caller's preset name.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "stock" => Some(Self::Stock),
            "sd-card" => Some(Self::SdCard),
            "autostart" => Some(Self::Autostart),
            _ => None,
        }
    }

    /// A request document's boot.
    pub fn from_description(boot: Option<BootPreset>) -> Self {
        match boot {
            None | Some(BootPreset::Stock) => Self::Stock,
            Some(BootPreset::SdCard) => Self::SdCard,
            Some(BootPreset::Autostart) => Self::Autostart,
        }
    }
}

/// One camera to open with FujiMod.
#[derive(Clone)]
pub struct Request {
    /// The emulator request: the body, its firmware, cards, settings and the
    /// session's other choices. A `stock` preset keeps its start and flash;
    /// the other presets choose their own.
    pub camera: EmulatorRequest,
    /// How the camera starts with FujiMod. The card presets boot FujiMod from
    /// the first SD card's files, or from the current release card when it
    /// names none.
    pub preset: Preset,
}

impl Request {
    /// The request a camera document describes, with host paths relative to
    /// `base`. The document's `snapshot` is the caller's to restore.
    pub fn from_description(
        document: &CameraDescription,
        base: &std::path::Path,
    ) -> Result<Self, Error> {
        let camera = emulator::description::request(document, base).map_err(Error::Missing)?;
        Ok(Self {
            camera,
            preset: Preset::from_description(document.boot),
        })
    }
}

/// A power-on that keeps the camera's internal flash and SD card.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerOn {
    /// Hold the body's SD power-on combination.
    pub hold_controls: bool,
    /// Put the SD card source's files on a fresh card.
    pub fresh_card: bool,
    /// Run to REC before returning.
    pub run_to_rec: bool,
}

/// Why a camera did not open with FujiMod.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Open(#[from] OpenError),
    #[error("{0}")]
    Library(#[from] LibraryError),
    #[error("release: {0}")]
    Release(String),
    #[error("{0}")]
    Missing(String),
}

impl From<Error> for OpenError {
    fn from(error: Error) -> Self {
        match error {
            Error::Open(open) => open,
            Error::Library(library) => Self::at(Stage::BootRom, library.to_string()),
            other => Self::at(Stage::BootRom, other.to_string()),
        }
    }
}

/// A camera opened with FujiMod, at REC unless the request stopped earlier.
pub struct Opened {
    pub camera: EmulatorOpened,
    /// Whether the request named an SD card source to hold.
    has_card_source: bool,
    /// The preset that opened it.
    pub preset: Preset,
    /// The release tag when the boot used the published release card.
    pub release: Option<String>,
}

impl std::ops::Deref for Opened {
    type Target = EmulatorOpened;

    fn deref(&self) -> &Self::Target {
        &self.camera
    }
}

impl Opened {
    /// The boot camera, for a host that serves the session.
    pub fn into_camera(self) -> emulator::Camera {
        self.camera.camera
    }

    /// The running session.
    pub fn session(&self) -> &Session {
        self.camera.session()
    }

    /// The running session, for a caller driving the guest.
    pub fn session_mut(&mut self) -> &mut Session {
        self.camera.session_mut()
    }

    /// Power the camera off and on again, keeping its internal flash and SD
    /// card, and check again that FujiMod is resident.
    pub fn power_on(
        &self,
        library: &Library,
        state: &StateDir,
        options: PowerOn,
    ) -> Result<Opened, Error> {
        if options.hold_controls && !self.has_card_source {
            return Err(Error::Missing(
                "this open named no SD card source to hold the card's boot controls".into(),
            ));
        }
        let camera = self.camera.power_on(
            library,
            state,
            EmulatorPowerOn {
                hold: options.hold_controls.then(|| "sd".to_owned()),
                fresh_card: options.fresh_card,
                run_to_rec: options.run_to_rec,
            },
        )?;
        let mut opened = Opened {
            camera,
            preset: self.preset,
            release: self.release.clone(),
            has_card_source: self.has_card_source,
        };
        opened.check(library)?;
        Ok(opened)
    }

    /// The FujiMod checks after a boot reaches REC: the card's loader is in
    /// RAM, and the release it applied is confirmed.
    fn check(&mut self, library: &Library) -> Result<(), Error> {
        if self.camera.stage != Stage::Rec {
            return Ok(());
        }
        if matches!(self.preset, Preset::Autostart) {
            let facts = library.nand_boot(&self.camera.firmware)?;
            fujimod_resident(&self.camera, facts.load_range.address)?;
        }
        if matches!(self.preset, Preset::SdCard | Preset::Autostart) {
            confirm_release(&mut self.camera)?;
        }
        Ok(())
    }
}

/// Open a camera with FujiMod.
pub fn open(library: &Library, state: &StateDir, request: Request) -> Result<Opened, Error> {
    let (emulator_request, release) = emulator_request(state, library, &request)?;
    let mut opened = Opened {
        camera: emulator::open(library, state, emulator_request)?,
        preset: request.preset,
        release,
        has_card_source: !matches!(card_source(&request), CardSource::None),
    };
    opened.check(library)?;
    Ok(opened)
}

/// Build a camera with FujiMod without running its guest.
pub fn open_unbooted(
    library: &Library,
    state: &StateDir,
    request: Request,
) -> Result<Opened, Error> {
    let (emulator_request, release) = emulator_request(state, library, &request)?;
    Ok(Opened {
        camera: emulator::open_unbooted(library, emulator_request)?,
        preset: request.preset,
        release,
        has_card_source: !matches!(card_source(&request), CardSource::None),
    })
}

/// The emulator inputs a preset names, and the release tag when it is the
/// published release card.
fn emulator_request(
    state: &StateDir,
    library: &Library,
    request: &Request,
) -> Result<(EmulatorRequest, Option<String>), Error> {
    let camera = &request.camera;
    if request.preset != Preset::Stock {
        if !matches!(camera.start, Start::Rom) {
            return Err(Error::Missing(format!(
                "a power-on hold and a loader image open the stock boot; boot {} starts its own way",
                request.preset.name()
            )));
        }
        if !matches!(camera.flash, Flash::Stock) {
            return Err(Error::Missing(format!(
                "a flash image opens the stock boot, not boot {}",
                request.preset.name()
            )));
        }
    }
    let firmware = emulator::plan(library, camera)?.firmware;
    let mut first = camera.cards.first();
    if request.preset != Preset::Stock && matches!(first.contents, CardContents::Image(_)) {
        return Err(Error::Missing(
            "a FujiMod card boot takes the card's files, not a card image".into(),
        ));
    }
    let mut release = None;
    let mut files = match card_source(request) {
        CardSource::Given => first.read_files()?,
        CardSource::Release => {
            let directory = release::card(state, library, &firmware)?;
            release = release_tag(state, &firmware);
            directory_files(&directory)?
        }
        CardSource::None => Vec::new(),
    };
    files.sort_by(|left, right| left.0.cmp(&right.0));
    if !matches!(first.contents, CardContents::Image(_)) {
        first.contents = Card::with_files(files.clone()).contents;
    }
    let start = match request.preset {
        Preset::Stock => camera.start.clone(),
        Preset::Autostart => Start::Rom,
        Preset::SdCard => Start::Held {
            combination: "sd".into(),
        },
    };
    let flash = match request.preset {
        Preset::Stock => camera.flash.clone(),
        Preset::SdCard => Flash::Stock,
        Preset::Autostart => autostart::flash(state, library, request, &firmware, &files)?,
    };
    let mut built = camera.clone();
    built.firmware = FirmwareChoice::Named(firmware);
    built.start = start;
    built.flash = flash;
    built.cards.sd_card = Some(first);
    Ok((built, release))
}

/// Where the first SD card's files come from.
enum CardSource {
    /// The files or image the request names.
    Given,
    /// The current release's SD card, downloaded once into the state directory.
    Release,
    /// A stock boot with no files: a blank card.
    None,
}

/// The card source a preset uses.
fn card_source(request: &Request) -> CardSource {
    let given = request
        .camera
        .cards
        .sd_card
        .as_ref()
        .is_some_and(|card| match &card.contents {
            CardContents::Files(files) => !files.is_empty(),
            CardContents::Image(_) => true,
        });
    match (request.preset, given) {
        (_, true) => CardSource::Given,
        (Preset::Stock, false) => CardSource::None,
        (Preset::SdCard | Preset::Autostart, false) => CardSource::Release,
    }
}

/// The release tag a downloaded card belongs to.
fn release_tag(state: &StateDir, firmware: &str) -> Option<String> {
    let directory = state.sd_cards();
    let tag = fs::read_dir(directory)
        .ok()?
        .flatten()
        .filter(|entry| entry.path().join(firmware).is_dir())
        .max_by_key(|entry| entry.metadata().ok()?.modified().ok())?
        .file_name()
        .to_str()?
        .to_owned();
    Some(tag)
}

/// Wait for FujiMod's runtime to confirm the release this boot applied. A
/// card without a FujiMod release, or one already confirmed, needs nothing.
fn confirm_release(opened: &mut EmulatorOpened) -> Result<(), Error> {
    let Some(state) = release_state(opened.session(), &opened.firmware) else {
        return Ok(());
    };
    let last = state.get("last_boot");
    let result = last
        .and_then(|last| last.get("result"))
        .and_then(Value::as_str);
    if result == Some("confirmed") {
        return Ok(());
    }
    if result != Some("running") {
        return Err(OpenError::at(
            Stage::Rec,
            format!(
                "release confirmation: {}",
                result.unwrap_or("STATE.JSN names no boot")
            ),
        )
        .into());
    }
    let quantum = opened.camera.quantum;
    let mut spent = 0u64;
    while spent < CONFIRM_INSTRUCTION_LIMIT {
        let state = release_state(opened.session(), &opened.firmware);
        let last = state.as_ref().and_then(|state| state.get("last_boot"));
        if let Some(result) = last
            .and_then(|last| last.get("result"))
            .and_then(Value::as_str)
        {
            if result == "confirmed" {
                return Ok(());
            }
            if result != "running" {
                return Err(OpenError::at(
                    Stage::Rec,
                    format!("release confirmation: the release ended as {result}"),
                )
                .into());
            }
        }
        let window = CONFIRM_WINDOW.min(CONFIRM_INSTRUCTION_LIMIT - spent);
        opened
            .session_mut()
            .run_with_limits(window, quantum, None, None)
            .map_err(|error| OpenError::at(Stage::Rec, error))?;
        spent += window;
    }
    Err(OpenError::at(
        Stage::Rec,
        format!("release confirmation: the release did not confirm within {CONFIRM_INSTRUCTION_LIMIT} instructions"),
    )
    .into())
}

/// The release's persisted state, when the card has one.
fn release_state(session: &Session, firmware: &str) -> Option<Value> {
    let name = session.contacts.keys().next()?.clone();
    let folder = release_folder(firmware);
    let path = format!("FUJIMOD/SYSTEM/{folder}/STATE.JSN");
    let bytes = session.card_read(&name, &path, None, 1 << 20).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The release's 8.3 directory under FUJIMOD/SYSTEM: the upper-case identity,
/// or its hyphens removed when it is longer than eight characters.
fn release_folder(firmware: &str) -> String {
    let identity = firmware.to_ascii_uppercase();
    if identity.len() <= 8 {
        identity
    } else {
        identity.replace('-', "")
    }
}

/// The FujiMod stage-2 loader carries its own UI strings; finding one proves
/// FujiMod is resident rather than the stock firmware.
fn fujimod_resident(opened: &EmulatorOpened, start: u64) -> Result<(), Error> {
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
        "FujiMod is not resident after the autostart boot",
    )
    .into())
}

/// A kept autostart flash's file name: a hash of the firmware and the card.
pub(crate) fn flash_key(firmware: &str, files: &[(String, Vec<u8>)]) -> String {
    let mut hash = Sha256::new();
    hash.update(firmware.as_bytes());
    for (name, bytes) in files {
        hash.update([0]);
        hash.update(name.as_bytes());
        hash.update([0]);
        hash.update(bytes);
    }
    format!("{:x}", hash.finalize())
}

/// The kept flash file a firmware and card name.
pub(crate) fn flash_path(state: &StateDir, firmware: &str, files: &[(String, Vec<u8>)]) -> PathBuf {
    state
        .autostart()
        .join(firmware)
        .join(format!("{}.flash", flash_key(firmware, files)))
}
