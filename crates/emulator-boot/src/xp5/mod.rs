//! XP5 bring-up runs on the shared session.
//!
//! Firmware images, the boot manifest, the boot ROM, card files and mods are all named by
//! the caller as paths. CPU execution and device scheduling belong to the shared XP5 session;
//! this crate assembles its inputs, drives the run in slices and records the outcome.

mod card_image;
mod display_output;
pub mod firmware;
mod manifest;
mod mod_loader;
mod open;
mod rom_start;
mod run;

pub use run::{Run, run_boot};


use fujimod_catalog_configuration::RunStart;
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator::board::bus::Mode;
use std::path::PathBuf;

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
    pub mode: Mode,
    pub quantum: Option<usize>,
}

pub struct Started {
    firmware: firmware::Firmware,
    state: manifest::BootState,
    opened: open::Opened,
}

impl Started {
    /// Transfer a loader-start board and its physical controls to a language binding.
    pub fn into_loader_session(
        self,
    ) -> Result<(fujimod_emulator_session::Session, fujimod_emulator_xp5_board::session::Inputs, usize), String> {
        if self.opened.flash_shadow.is_some() {
            return Err("XP5 ROM start needs the flash aperture driver".into());
        }
        Ok((self.opened.session, self.opened.inputs, self.opened.read_limit))
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
        scene, output: run_directory, viewer_port, mode, quantum: quantum_override } = startup;
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
    let mut runtime_files = Vec::new();
    let rom = if matches!(start, RunStart::Rom | RunStart::SdCard) {
        let start = rom_start::RomStart::load(
            rom_path.as_deref().ok_or("--start rom needs --rom FILE")?,
            true,
            card_files,
            true,
        )?;
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
        runtime_files = card_files;
        None
    };
    let mut opened = open::open_from(
        &firmware,
        &state,
        configuration,
        scene.as_deref(),
        run_directory.as_deref(),
        mode,
        quantum_override,
        rom.as_ref(),
    )?;
    if rom.is_none() {
        crate::media::attach(&mut opened.session, sd_image.as_deref(), &runtime_files)?;
    }
    Ok(Started { firmware, state, opened })
}
