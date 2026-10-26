//! Hardware request lines and peripheral data ports shared with DMA engines.
//! Port operations are hardware accesses, never host inspection. The graph must
//! declare dependencies in both directions when FIFO changes wake a DMA engine
//! and DMA accesses change a peripheral's next deadline.
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Receive,
    Transmit,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    since: Option<u64>,
}
impl Request {
    pub fn since(&self) -> Option<u64> {
        self.since
    }
    pub fn update(&mut self, now: u64, high: bool) {
        if !high {
            self.since = None;
        } else if self.since.is_none() {
            self.since = Some(now);
        }
    }
}
pub trait Port: Send {
    /// First assertion tick of a still-asserted hardware request.
    fn request(&self, direction: Direction, burst: bool) -> Option<u64>;
    fn receive(&mut self, now: u64, bytes: u8) -> Result<u64, String>;
    fn transmit(&mut self, now: u64, bytes: u8, value: u64) -> Result<(), String>;
}
pub type SharedPort = Arc<Mutex<dyn Port>>;
