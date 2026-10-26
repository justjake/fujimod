//! Camera SoC devices implement the Rust Device interface for a runtime.
//!
//! Plain Rust: no Python, Unicorn or Hypervisor.framework dependency. A
//! runtime owns the CPU, clock, and dispatch; models see it only through
//! [`Host`] (inside a [`Cx`]). Device effects flow through Host in order, and
//! unsupported operations return explicit refusals.

pub mod bus;
pub mod capability;
pub mod checkpoint;
pub mod compression;
pub mod dma;
pub mod exr2;
pub mod fingerprint;
pub mod gic;
pub mod graph;
pub mod graphics;
pub mod keyed_io;
pub mod exr;
pub mod lz_osd;
pub mod map;
pub mod parts;
pub mod math;
pub mod record;
pub mod signals;
pub mod storage_reads;
#[doc(hidden)]
pub mod test_host;
pub mod work;
pub mod xp5;

pub use bus::{Cx, Device, Host};
pub use map::WordMap;
pub use record::{Action, Record, Unsupported, Value};
