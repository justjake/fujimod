//! The native session behind the emulator UI.
//!
//! The browser and desktop frontends use the same `RemoteEngine` command and
//! frame protocol. Guest execution stays on the host's session thread; a
//! [`Viewer`] applies a window's input to any [`Run`] between guest slices and
//! sends it the latest LCD image.

pub mod launch;
mod protocol;
mod server;

pub use server::{Endpoint, Run, Socket, Viewer, bind, serve};

use fujimod_emulator_boot::xp5::RunHost;

impl Run for RunHost {
    fn session(&self) -> &fujimod_emulator_session::Session {
        RunHost::session(self)
    }
    fn session_mut(&mut self) -> &mut fujimod_emulator_session::Session {
        RunHost::session_mut(self)
    }
    fn step(&mut self) -> Result<(), String> {
        RunHost::step(self)
    }
    fn frames(&mut self) -> Result<Vec<fujimod_emulator_devices::graphics::scanout::Frame>, String> {
        Ok(self.take_frames())
    }
    fn finish(self) -> Result<serde_json::Value, String> {
        RunHost::finish(self)
    }
}
