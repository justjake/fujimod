//! Shared physical environment, camera rig and Bevy rendering for FujiMod.
mod lighting;
mod navigation;
pub mod rendering;
#[cfg(target_arch = "wasm32")]
mod web;
pub mod world;
pub use world::{InteractionMode, Simulation};
