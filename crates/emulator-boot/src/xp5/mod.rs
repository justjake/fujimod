//! XP5 bring-up runs on the shared session.
//!
//! Firmware images, the boot manifest, the boot ROM, card files and mods are all named by
//! the caller as paths. CPU execution and device scheduling belong to the shared XP5 session;
//! this crate assembles its inputs, drives the run in slices and records the outcome.

mod card_image;
mod display_output;
mod loader_card;
pub mod firmware;
mod manifest;
mod open;
mod profile;
mod rom_start;
mod run;

pub use run::{Run, run_boot};


use fujimod_catalog_configuration::RunStart;
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator::board::bus::Mode;
use std::{collections::BTreeMap, path::PathBuf};

/// Explicit files and settings needed to construct an XP5 session.
pub struct Startup {
    pub update: PathBuf,
    pub manifest: Option<PathBuf>,
    pub boot_rom: Option<PathBuf>,
    pub card_image: Option<PathBuf>,
    pub card_files: Vec<(String, PathBuf)>,
    pub scene: Option<PathBuf>,
    pub output: Option<PathBuf>,
    pub viewer_port: Option<u16>,
    /// Physical key requests from the frame viewer (POST /key); None disables them.
    pub viewer_keys: Option<crate::frame_viewer::KeyHandler>,
    pub mode: Mode,
    pub quantum: Option<usize>,
    /// Named camera settings (`setup=done`, `language=en`) the loader start boots with, from
    /// the user profile the firmware's settings layout locates. Empty: the flash as installed.
    pub camera_settings: BTreeMap<String, String>,
}

pub struct Started {
    firmware: firmware::Firmware,
    state: manifest::BootState,
    opened: open::Opened,
    /// The loader start's `--card-image`, written back at the end of a run.
    card_write_back: Option<loader_card::WriteBack>,
}

impl Started {
    /// Transfer the opened board and physical input handle to a language binding.
    pub fn into_session(
        self,
    ) -> (fujimod_emulator_session::Session, fujimod_emulator_xp5_board::session::Inputs, usize) {
        (self.opened.session, self.opened.inputs, self.opened.read_limit)
    }

    pub fn session_mut(&mut self) -> &mut fujimod_emulator_session::Session {
        &mut self.opened.session
    }
}

pub(super) fn open_startup(
    configuration: &ResolvedConfiguration,
    startup: Startup,
) -> Result<Started, String> {
    let Startup { update, manifest, boot_rom: rom_path, card_image: sd_image, card_files,
        scene, output: run_directory, viewer_port, viewer_keys, mode, quantum: quantum_override,
        camera_settings } = startup;
    let start = configuration.configuration.start;
    if start == RunStart::SdCard && sd_image.is_none()
        && !card_files.iter().any(|(name, _)| name.eq_ignore_ascii_case(rom_start::CARD_FILE_NAME))
    {
        return Err("XP5 SD card startup requires marble_init.bin or an SD card image".into());
    }
    let firmware_id = configuration.configuration.firmware_id.clone();
    let mut state = manifest::BootState::for_firmware(&firmware_id, manifest.as_deref())?;
    let preloads = configuration.configuration.preset.as_ref()
        .and_then(|preset| preset.firmware_profile_id.as_ref())
        .and_then(|id| configuration.reference.firmware.layouts.as_ref()?.profiles.get(id))
        .and_then(|profile| profile.preload.as_deref())
        .unwrap_or_default();
    state.preload.splice(..0, preloads.iter().cloned());
    if let Some(port) = viewer_port {
        if run_directory.is_none() {
            return Err("the frame viewer needs a run output directory".into());
        }
        let display = state.device.iter_mut().find(|window| window.base == 0xff99_0000 && window.kind == "display")
            .ok_or("configuration declares no primary display window")?;
        display.options.insert("viewer_port".into(), toml::Value::Integer(port.into()));
    }
    let update_bytes = std::fs::read(&update).map_err(|e| format!("{}: {e}", update.display()))?;
    let firmware = firmware::Firmware::from_update(&firmware_id, update_bytes, true)?;
    if let Some(directory) = &run_directory {
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    }
    let mut loader = None;
    let rom = if matches!(start, RunStart::Rom | RunStart::SdCard | RunStart::Setup) {
        let start = match (rom_path.as_deref(), start) {
            (None, RunStart::Setup) => {
                let handoff = rom_start::SetupHandoff::for_firmware(&firmware_id)
                    .ok_or("this firmware declares no setup-stage start")??;
                rom_start::RomStart::from_setup_handoff(handoff)?
            }
            (Some(path), _) => rom_start::RomStart::load(path, true, card_files, true)?,
            (None, RunStart::SdCard) => {
                let handoff = rom_start::Handoff::for_firmware(&firmware_id)
                    .ok_or("this SD card start requires --boot-rom")??;
                rom_start::RomStart::from_card_handoff(handoff, card_files)?
            }
            (None, _) => return Err("ROM startup requires --boot-rom".into()),
        };
        Some(match &sd_image {
            Some(image) => {
                let start = start.with_card_image(image)?;
                if let Some(card) = &start.card_image {
                    for line in &card.layout {
                        eprintln!("FUJIMOD card image {}: {line}", image.display());
                    }
                    eprintln!("FUJIMOD card image root names: {:?}", card.names);
                }
                start
            }
            None => start,
        })
    } else {
        if rom_path.is_some() {
            return Err("a boot ROM requires a ROM or SD card startup".into());
        }
        // The loader start's card: slot 1, inserted before the board is built (loader_card.rs).
        loader = loader_card::LoaderCard::load(sd_image.as_deref(), &card_files)?;
        if let Some(card) = &loader {
            card.apply(&mut state)?;
        }
        None
    };
    let profile = if camera_settings.is_empty() {
        None
    } else if rom.is_some() {
        return Err("camera settings apply to the loader start only".into());
    } else {
        Some(profile::place(&mut state, &firmware, configuration, &camera_settings)?)
    };
    let opened = open::open_from(
        &firmware,
        &state,
        configuration,
        scene.as_deref(),
        run_directory.as_deref(),
        mode,
        quantum_override,
        rom.as_ref(),
        loader.as_ref(),
        viewer_keys,
        profile.as_ref(),
    )?;
    let card_write_back = loader.and_then(|card| card.write_back);
    Ok(Started { firmware, state, opened, card_write_back })
}
