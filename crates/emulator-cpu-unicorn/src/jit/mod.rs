//! TCG-to-Wasm compilation owned by the Unicorn backend.
//!
//! Decoding and emission are independent of host module/table handles. The
//! host loader will receive bytes and export descriptions from this module.
mod analysis;
mod binary;
mod decode;
mod emit;
#[cfg(target_os = "emscripten")]
pub mod emscripten;
pub mod ir;
mod layout;
mod tier;

pub use analysis::{BasicBlock, PreparedBlock, Registers};
pub use binary::Binary;
pub use decode::Compiler;
pub use emit::{Module, PreparedFunction};
pub use layout::{Environment, Helper, HelperKind, HelperLayout, MemoryLayout};
pub use tier::{Host, JitOptions, Metadata, Statistics, Stats, Tier};
