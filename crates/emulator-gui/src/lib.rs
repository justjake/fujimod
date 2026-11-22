//! The native session behind the emulator UI.
//!
//! The browser and desktop frontends use the same `RemoteEngine` command and
//! frame protocol. Guest execution stays on this process's session thread;
//! WebSocket clients submit host input and receive the latest LCD image.

pub mod launch;
mod protocol;
mod server;

pub use server::{Endpoint, Host, Socket, bind};

/// A prepared XP5 run, stopped at the boundary a desktop frontend drives.
pub use fujimod_emulator_boot::xp5::RunHost;

/// Serve a prepared run once its desktop frontend has joined the endpoint.
pub fn host(run: RunHost, socket: Socket) -> Result<serde_json::Value, String> {
    socket.serve(run).run()
}
