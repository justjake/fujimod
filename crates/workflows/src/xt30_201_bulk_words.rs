//! X-T30 2.01 stock capabilities source adapted to bounded DRAM word windows.
//!
//! Hardware qualification is pending. Before calling, the caller must verify
//! camera/firmware identity, existing read gate, stock handler bytes, ordinary
//! DRAM backing and CPU/DMA address equivalence for the admitted aperture, cache
//! visibility and exclusive device ownership. Host session serialization does
//! not establish firmware-task quiescence. This is not a controller/MMIO reader:
//! word alignment and host decoding do not imply single 32-bit CPU or bus reads.
use fujimod_usb::ff80::{
    Session,
    commands::memory::{Address, DebugMemoryLimits, MemoryRange, ReadMemory},
};
use std::num::NonZeroU32;

pub const SOURCE: u32 = 0x0223_7190;
pub const SOURCE_STATE: u32 = 0x02bd_8340;
pub const CAPACITY: u32 = 48;

#[derive(Debug, thiserror::Error)]
pub enum WordReadError {
    #[error("{0}")]
    Window(&'static str),
    #[error("{message}")]
    Observation {
        message: &'static str,
        /// Complete unexpected reply, retained before refusing E1.
        raw: Vec<u8>,
    },
    #[error(transparent)]
    Usb(#[from] fujimod_usb::Error),
}

/// One to twelve contiguous words in a caller-qualified ordinary-DRAM aperture.
/// The numeric low-address restriction is not installed-capacity evidence.
#[derive(Clone, Copy, Debug)]
pub struct WordWindow(MemoryRange);

impl WordWindow {
    pub fn new(range: MemoryRange, aperture: MemoryRange) -> Result<Self, WordReadError> {
        let start = range.address().0;
        let size = range.length().get();
        let low = aperture.address().0;
        let high = u64::from(low) + u64::from(aperture.length().get());
        if high > 0x2000_0000 || start < low || u64::from(start) + u64::from(size) > high {
            return Err(WordReadError::Window(
                "word window must fit its caller-qualified low DRAM aperture",
            ));
        }
        if start % 4 != 0 || size % 4 != 0 || size > CAPACITY {
            return Err(WordReadError::Window(
                "word window needs four-byte alignment and 4..48 bytes in whole words",
            ));
        }
        Ok(Self(range))
    }

    pub fn range(self) -> MemoryRange {
        self.0
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WordRead {
    pub address: u32,
    /// Exact bulk reply; no padding, masking, rounding or byte dropping.
    pub raw: Vec<u8>,
    pub words: Vec<u32>,
}

/// Prepare the existing stock 48-byte source, pin its published pair, then
/// issue one E1. No payload installation, configuration write, arbitrary RAM
/// write, diagnostic tag query, retry or hidden cleanup occurs. The stock
/// capabilities command changes only its reviewed protocol publication state.
/// Each call prepares a fresh source and holds the same mutable session for all
/// exchanges. The caller owns session close; unknown completion forbids cleanup.
pub async fn read(
    session: &mut Session,
    window: WordWindow,
    limits: DebugMemoryLimits,
) -> Result<WordRead, WordReadError> {
    // Refuse an inadequate CPU-reader limit before publishing any source.
    let pair = ReadMemory::new(
        MemoryRange::new(Address(SOURCE_STATE), NonZeroU32::new(8).unwrap())?,
        limits,
    )?;
    let reply = session.prepare_capabilities().await?;
    let mut expected = [0; 32];
    expected[..4].copy_from_slice(&0x580000u32.to_le_bytes());
    expected[20..24].copy_from_slice(&CAPACITY.to_le_bytes());
    if reply.byte_length != CAPACITY || reply.raw != expected {
        session.invalidate_completion();
        return Err(WordReadError::Observation {
            message: "X-T30 capabilities must report 48 bytes and zero reserved fields",
            raw: reply.raw.to_vec(),
        });
    }
    let state = session.read_memory(pair).await?;
    let expected: Vec<u8> = [SOURCE, CAPACITY]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    if state != expected {
        session.invalidate_completion();
        return Err(WordReadError::Observation {
            message: "published capabilities source/capacity differ from the stock pair",
            raw: state,
        });
    }
    let range = window.range();
    let offset = range.address().0.wrapping_sub(SOURCE);
    let raw = session.read_data(offset, range.length()).await?;
    let words = raw
        .chunks_exact(4)
        .map(|word| u32::from_le_bytes(word.try_into().expect("four bytes")))
        .collect();
    Ok(WordRead {
        address: range.address().0,
        raw,
        words,
    })
}

#[cfg(test)]
mod tests;
