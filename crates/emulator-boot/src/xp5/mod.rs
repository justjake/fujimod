//! XP5 bring-up runs on the shared session.
//!
//! Firmware images, the boot manifest, the boot ROM, card files and mods are all named by
//! the caller as paths. CPU execution and device scheduling belong to the shared XP5 session;
//! this crate assembles its inputs, drives the run in slices and records the outcome.

mod card_image;
mod display_output;
mod emulator_patch;
pub mod firmware;
mod frontend;
mod manifest;
mod mod_loader;
mod open;
mod png_writer;
mod rom_start;
mod run;

pub use run::{Run, parse_quantum, parse_seconds, run_boot};


use fujimod_catalog_configuration::RunStart;
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator::board::bus::Mode;
use std::path::PathBuf;

/// Explicit files and settings needed to construct an XP5 session.
pub struct Startup {
    pub update: PathBuf,
    pub boot_rom: Option<PathBuf>,
    pub card_image: Option<PathBuf>,
    pub card_files: Vec<(String, PathBuf)>,
    pub scene: Option<PathBuf>,
    pub output: PathBuf,
    pub mode: Mode,
    pub quantum: Option<usize>,
}

pub struct Started {
    firmware: firmware::Firmware,
    state: manifest::BootState,
    opened: open::Opened,
}

impl Started {
    pub fn session_mut(&mut self) -> &mut fujimod_emulator_session::Session {
        &mut self.opened.session
    }
}

pub(super) fn open_startup(
    configuration: &ResolvedConfiguration,
    startup: Startup,
) -> Result<Started, String> {
    let Startup { update, boot_rom: rom_path, card_image: sd_image, card_files,
        scene, output: run_directory, mode, quantum: quantum_override } = startup;
    let start_rom = matches!(configuration.configuration.start, RunStart::Rom);
    let firmware_id = configuration.configuration.firmware_id.clone();
    let state = manifest::BootState::for_firmware(&firmware_id)?;
    let update_bytes = std::fs::read(&update).map_err(|e| format!("{}: {e}", update.display()))?;
    let firmware = firmware::Firmware::from_update(&firmware_id, update_bytes, true)?;
    std::fs::create_dir_all(&run_directory).map_err(|error| error.to_string())?;
    let rom = if start_rom {
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
    } else if rom_path.is_some()
        || sd_image.is_some()
        || !card_files.is_empty()
    {
        return Err("--rom, --sd-file, --sd-image, --sd-absent and --dials apply to --start rom only".into());
    } else {
        None
    };
    let opened = open::open_from(
        &firmware,
        &state,
        configuration,
        scene.as_deref(),
        &run_directory,
        mode,
        quantum_override,
        rom.as_ref(),
    )?;
    Ok(Started { firmware, state, opened })
}
