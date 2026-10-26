//! Processor-independent function families. Firmware policy and boundaries are inputs.
//! No firmware, disassembly, names or populated indexes are distributed with this crate.
mod decode;
mod candidates;
pub use candidates::FunctionCandidate;
mod inspect;
pub use inspect::*;
mod matching;
mod alignment;
pub use alignment::align_sequences;
mod quality;
pub use quality::NameSupport;
mod model;
pub use model::*;
mod lookup;
pub use lookup::*;
mod store;
pub use store::Store;

#[cfg(test)]
mod tests;

/// Firmware locators built on the shared index query API.
pub mod locators;
