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
mod phase;
mod profile;
mod rom_start;
mod run;

pub use run::{Run, RunHost, run_boot};


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
    /// The loader start's card in SD slot 2: a raw image, or files for an in-memory volume.
    pub second_card_image: Option<PathBuf>,
    pub second_card_files: Vec<(String, PathBuf)>,
    pub scene: Option<PathBuf>,
    pub output: Option<PathBuf>,
    pub mode: Mode,
    pub quantum: Option<usize>,
    /// Named camera settings (`setup=done`, `language=en`) the loader start boots with, from
    /// the user profile the firmware's settings layout locates. Empty: the flash as installed.
    pub camera_settings: BTreeMap<String, String>,
}

pub(crate) struct RunFacts {
    pub firmware: firmware::Firmware,
    pub state: manifest::BootState,
    pub inputs: Option<fujimod_emulator_xp5_board::session::Inputs>,
    pub outputs: crate::output::Outputs,
    /// The loader start's card images, written back at the end of a run.
    pub card_write_back: Vec<loader_card::WriteBack>,
}

pub(super) fn open_startup(
    configuration: &ResolvedConfiguration,
    startup: Startup,
) -> Result<crate::Camera, String> {
    let Startup { update, manifest, boot_rom: rom_path, card_image: sd_image, card_files,
        second_card_image, second_card_files, scene, output: run_directory, mode, quantum: quantum_override,
        camera_settings } = startup;
    let start = configuration.configuration.start;
    if start == RunStart::SdCard && sd_image.is_none()
        && !card_files.iter().any(|(name, _)| name.eq_ignore_ascii_case(rom_start::CARD_FILE_NAME))
    {
        return Err("XP5 SD card startup requires marble_init.bin or an SD card image".into());
    }
    let firmware_id = configuration.configuration.firmware_id.clone();
    let mut state = manifest::BootState::for_firmware(&firmware_id, manifest.as_deref())?
        .with_catalog_windows(configuration)?;
    let preloads = configuration.configuration.preset.as_ref()
        .and_then(|preset| preset.firmware_profile_id.as_ref())
        .and_then(|id| configuration.reference.firmware.layouts.as_ref()?.profiles.get(id))
        .and_then(|profile| profile.preload.as_deref())
        .unwrap_or_default();
    state.preload.splice(..0, preloads.iter().cloned());
    let update_bytes = std::fs::read(&update).map_err(|e| format!("{}: {e}", update.display()))?;
    let firmware = firmware::Firmware::from_update(&firmware_id, update_bytes, true)?;
    if let Some(directory) = &run_directory {
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    }
    let mut loader = Vec::new();
    let rom = if matches!(start, RunStart::Rom | RunStart::SdCard | RunStart::Setup) {
        if second_card_image.is_some() || !second_card_files.is_empty() {
            return Err("a card in SD slot 2 needs the loader start".into());
        }
        let start = match (rom_path.as_deref(), start) {
            (None, RunStart::Setup) => {
                let handoff = rom_start::SetupHandoff::for_firmware(&firmware_id)
                    .ok_or("this firmware declares no setup-stage start")??;
                rom_start::RomStart::from_setup_handoff(handoff)?
            }
            (Some(path), _) => rom_start::RomStart::load(path, true, card_files, true)?
                .with_setup_windows_of(&firmware_id)?,
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
        // The loader start's cards, inserted before the board is built (loader_card.rs).
        loader.extend(loader_card::LoaderCard::load(loader_card::SLOT, sd_image.as_deref(), &card_files)?);
        loader.extend(loader_card::LoaderCard::load(
            loader_card::SLOT_2, second_card_image.as_deref(), &second_card_files,
        )?);
        for card in &loader {
            card.apply(&mut state)?;
        }
        None
    };
    // A ROM or SD card start drops the hand-off preload: stage one builds that block itself,
    // from the power-controller registers the profile header also goes into.
    let profile = if camera_settings.is_empty() {
        None
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
        &loader,
        profile.as_ref(),
    )?;
    let card_write_back = loader.into_iter().filter_map(|card| card.write_back).collect();
    let open::Opened {
        session,
        inputs,
        outputs,
        ticks_per_second,
        quantum,
        read_limit,
    } = opened;
    Ok(crate::Camera {
        session,
        read_limit,
        ticks_per_second,
        quantum,
        xp5: Some(RunFacts {
            firmware,
            state,
            inputs: Some(inputs),
            outputs,
            card_write_back,
        }),
    })
}
