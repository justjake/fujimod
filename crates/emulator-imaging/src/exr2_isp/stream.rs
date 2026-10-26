//! A physical packed422 stream between warp and JPEG. A producer START
//! publishes one raster; a consumer START claims it. Completion releases the
//! producer through this shared wire, never a firmware buffer or signature.
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    #[default]
    Idle,
    Ready,
    Claimed,
    Finished,
}
#[derive(Default, Serialize, Deserialize)]
pub struct Stream {
    pub state: State,
    pub width: usize,
    pub height: usize,
    pub pixels: fujimod_emulator_devices::work::Buffer,
}
pub type StreamHandle = Arc<Mutex<Stream>>;
impl Stream {
    pub fn claim(
        &mut self,
        width: usize,
        height: usize,
    ) -> Result<fujimod_emulator_devices::work::FrozenBytes, String> {
        if self.state != State::Ready || self.width != width || self.height != height {
            return Err("JPEG has no matching started warp stream".into());
        }
        self.state = State::Claimed;
        Ok(self.pixels.freeze())
    }
    pub fn finish(&mut self) -> Result<(), String> {
        if self.state != State::Claimed {
            return Err("JPEG stream completion without a claim".into());
        }
        self.state = State::Finished;
        // The consumer has encoded the complete raster. Only the completion
        // handshake remains; keep capacity without saving another full image.
        self.pixels.clear();
        Ok(())
    }
}
