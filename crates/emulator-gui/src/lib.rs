//! The native session behind the emulator UI.
//!
//! The browser and desktop frontends use the same `RemoteEngine` command and
//! frame protocol. Guest execution stays on this process's session thread;
//! WebSocket clients submit host input and receive the latest LCD image.

mod protocol;
mod server;
mod controls;

pub use protocol::GuiOptions;
pub use server::{Endpoint, Host, Socket, bind, open, serve};

/// An opened XP5 camera and its physical key handle.
pub struct Xp5Session {
    pub configuration: fujimod_catalog_resolution::ResolvedConfiguration,
    pub session: fujimod_emulator_session::Session,
    pub inputs: fujimod_emulator_xp5_board::session::Inputs,
    pub ticks_per_second: u64,
    pub quantum: usize,
}
