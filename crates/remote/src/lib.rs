//! Reference-based camera control. Connections own transport and observation;
//! operations own navigation. Callers supply facts and assert on read results.
#![forbid(unsafe_code)]
#![recursion_limit = "256"]
pub mod reference;
pub use reference::{Control, Menu, OptionReference, Reference, control, menu, option};
pub mod protocol;
pub use protocol::{
    Capture, Clock, ControlSnapshot, Input, InputState, LocalFuture, MemoryRead, MenuSnapshot,
    ReadSet, Readiness, Remote, RemoteReadResult, RemoteReader, Result, State, Value,
};
pub mod backend;
pub mod memory;
pub use fujimod_catalog_remote::Facts;
pub mod resolve;

#[cfg(feature = "board")]
pub mod board;
pub mod connection;
pub mod operations;
#[cfg(feature = "board")]
pub use board::BoardRemote;

#[cfg(feature = "ff80")]
pub mod ff80;
#[cfg(feature = "ff80")]
pub use ff80::Ff80Remote;
pub mod testing;
