//! Paused host instrumentation on the shared runtime, outside Session state.
use super::*;
impl Board {
    pub(super) fn clear_observation(&mut self) {
        let session = match self.backend.as_ref() {
            Some(Backend::Graph(session)) => Some(session),
            Some(Backend::Xp5(opened)) => Some(&opened.session),
            _ => None,
        };
        if let Some(session) = session {
            if let Some(mut watch) = self.function_watch.take() {
                let _ = session.board.function_watch_end(&mut watch);
            }
            if let Some(mut watch) = self.memory_watch.take() {
                let _ = session.board.memory_watch_end(&mut watch);
            }
            for (core, mut points) in std::mem::take(&mut self.instruction_points) {
                let _ = session.board.instruction_points_end(core, &mut points);
            }
        }
    }
}
