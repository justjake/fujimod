//! Checked board execution and observation mechanisms.
pub mod architecture;
pub mod board;

pub const RUSTC_VERSION: &str = env!("FUJIMOD_EMULATOR_RUSTC_VERSION");
pub const TARGET: &str = env!("FUJIMOD_EMULATOR_TARGET");
