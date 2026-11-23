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

/// Shortest wall time between LCD reads, about 30 images a second.
const FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

/// An opened camera of any processor, run live for its window.
pub struct Live {
    camera: fujimod_emulator_boot::Camera,
    selection: fujimod_catalog::reference::ResolvedSelection,
    read_at: std::time::Instant,
}

impl Live {
    pub fn new(
        camera: fujimod_emulator_boot::Camera,
        selection: fujimod_catalog::reference::ResolvedSelection,
    ) -> Self {
        Self {
            camera,
            selection,
            read_at: std::time::Instant::now() - FRAME_INTERVAL,
        }
    }
}

impl Run for Live {
    fn session(&self) -> &fujimod_emulator_session::Session {
        &self.camera.session
    }
    fn session_mut(&mut self) -> &mut fujimod_emulator_session::Session {
        &mut self.camera.session
    }
    fn step(&mut self) -> Result<(), String> {
        // About one LCD frame of guest time per slice keeps the window responsive.
        let session = &mut self.camera.session;
        let until = session.board.now() + (self.camera.ticks_per_second / 60).max(1);
        session.run_window(u64::MAX, self.camera.quantum, None, Some(until))
    }
    fn frames(&mut self) -> Result<Vec<fujimod_emulator_devices::graphics::scanout::Frame>, String> {
        if self.read_at.elapsed() < FRAME_INTERVAL {
            return Ok(Vec::new());
        }
        self.read_at = std::time::Instant::now();
        // The wiring is located once the firmware has loaded its configuration.
        if self.camera.session.controls.is_none() {
            let _ = fujimod_emulator_controls::connect(
                &mut self.camera.session,
                &self.selection,
                self.camera.ticks_per_second,
            );
        }
        fujimod_emulator_boot::lcd_frames(&mut self.camera.session)
    }
    fn finish(self) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({
            "guest_ticks": self.camera.session.board.now(),
            "instructions": self.camera.session.board.instruction_counts(),
        }))
    }
}
