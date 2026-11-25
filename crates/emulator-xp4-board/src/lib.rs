//! X-Processor 4 boards on the shared emulator board: the definition, the
//! catalog builder, the XP4 device models and the session they make.
pub mod catalog;
mod definition;
mod device_build;
mod devices;
mod dsp;
mod jpeg;
pub mod memory;
pub mod shared;
mod still;

pub use definition::{Definition, NorImmutablePrefix};

use fujimod_emulator_cpu::memory::Backing;
use std::collections::BTreeMap;
