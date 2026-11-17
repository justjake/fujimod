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
