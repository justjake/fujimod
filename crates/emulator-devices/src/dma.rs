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

/// Device-backed memory that a bus master can read but that is not board RAM, such as a serial
/// flash CPU aperture. Reads are hardware accesses: a source may record them as guest evidence.
pub trait ReadSource: Send + Sync {
    /// Read `out.len()` bytes at `offset` from the window base.
    fn read(&self, offset: u64, out: &mut [u8]) -> Result<(), String>;
}

/// A read-only physical window `base..base + size` served by a [`ReadSource`].
#[derive(Clone)]
pub struct ReadWindow {
    pub base: u64,
    pub size: u64,
    pub source: Arc<dyn ReadSource>,
}

impl ReadWindow {
    fn covers(&self, address: u64, len: usize) -> bool {
        address >= self.base
            && address
                .checked_add(len as u64)
                .is_some_and(|end| end <= self.base.saturating_add(self.size))
    }
}

/// A bus-master read: board RAM first, then a read-only window that covers the whole access.
/// RAM keeps precedence so a start that shadows a window in RAM reads exactly as before.
pub fn bus_read(
    windows: &[ReadWindow],
    cx: &crate::Cx<'_>,
    address: u64,
    out: &mut [u8],
) -> Result<(), String> {
    let ram = cx.ram_read(address, out);
    match (ram, windows.iter().find(|w| w.covers(address, out.len()))) {
        (Err(_), Some(window)) => window.source.read(address - window.base, out),
        (ram, _) => ram,
    }
}

/// A bus-master write: board RAM only. A write into a read-only window refuses by name.
pub fn bus_write(
    windows: &[ReadWindow],
    cx: &mut crate::Cx<'_>,
    address: u64,
    bytes: &[u8],
) -> Result<(), String> {
    match cx.ram_write(address, bytes) {
        Err(e) if windows.iter().any(|w| w.covers(address, bytes.len())) => Err(format!(
            "DMA write to read-only device memory at {address:#x}: {e}"
        )),
        result => result,
    }
}
