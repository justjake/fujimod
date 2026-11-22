//! Data-cache coherence over declared device-visible physical ranges.
//!
//! Execution keeps one copy of memory: the processor's view, with every store
//! applied. A bus master that reads the same memory sees only what the write
//! buffer and caches have pushed out. This model records, for lines inside
//! the declared ranges, what the other copy holds so that a device read
//! returns the bytes memory held before a store was cleaned, and a processor
//! load returns the bytes a line was filled with before a device wrote it.
//!
//! - A processor store marks the stored bytes dirty and saves the bytes they
//!   replaced: the memory image.
//! - A device read overlays the memory image over processor-view bytes.
//! - A device write to a line the processor has cached saves the
//!   processor's bytes as stale, so later loads still see them until the line
//!   is invalidated.
//! - Clean and invalidate drop the saved copies. Each returns the patches to
//!   apply to the processor-view memory when its bytes must change (a clean
//!   line whose stale bytes outlived a device write, an invalidated dirty
//!   line reverting to the memory image).
//!
//! Lines are never evicted on their own, which is stricter than hardware and
//! deterministic. An empty range list costs one branch per call.
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// Bytes to write into processor-view memory after a maintenance operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Patch {
    pub address: u64,
    pub bytes: Vec<u8>,
}

/// Bytes a processor load must return in place of memory's contents.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct StaleOverlay {
    pieces: Vec<(u64, Vec<u8>)>,
}
impl StaleOverlay {
    /// Replace the stale bytes in `buffer`, which holds the bytes at
    /// `address`.
    pub fn apply(&self, address: u64, buffer: &mut [u8]) {
        for (start, bytes) in &self.pieces {
            let offset = (start - address) as usize;
            buffer[offset..offset + bytes.len()].copy_from_slice(bytes);
        }
    }
}

#[derive(Default)]
struct Line {
    cached: bool,
    /// Bytes the processor stored that no clean has pushed out.
    dirty: u128,
    /// What memory holds for the `dirty` bytes.
    memory: Vec<u8>,
    /// Bytes where the processor's copy differs from memory because a device
    /// wrote memory after the line was cached.
    stale: u128,
    /// The processor's copy of the `stale` bytes.
    processor: Vec<u8>,
}
pub struct CoherentRanges {
    line_bytes: u64,
    /// Line-aligned physical `[begin, end)` ranges.
    ranges: Vec<(u64, u64)>,
    lines: BTreeMap<u64, Line>,
}

impl CoherentRanges {
    /// `line_bytes` is a power of two from 16 to 128. Ranges must be aligned
    /// to it.
    pub fn new(line_bytes: u32, ranges: &[(u64, u64)]) -> Result<Self, String> {
        if !line_bytes.is_power_of_two() || !(16..=128).contains(&line_bytes) {
            return Err("invalid coherent-range line size".into());
        }
        let line = line_bytes as u64;
        if ranges
            .iter()
            .any(|&(begin, end)| begin >= end || begin % line != 0 || end % line != 0)
        {
            return Err("coherent ranges must be line aligned and non-empty".into());
        }
        Ok(Self {
            line_bytes: line,
            ranges: ranges.to_vec(),
            lines: BTreeMap::new(),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    fn covers(&self, line: u64) -> bool {
        self.ranges.iter().any(|&(b, e)| b <= line && line < e)
    }

    /// Each covered line the access touches: line address and the byte span
    /// `[from, to)` inside the line.
    fn touched(&self, address: u64, len: usize) -> Vec<(u64, usize, usize)> {
        let mut out = Vec::new();
        if self.ranges.is_empty() || len == 0 {
            return out;
        }
        let end = address + len as u64;
        let mut line = address & !(self.line_bytes - 1);
        while line < end {
            if self.covers(line) {
                let from = address.max(line) - line;
                let to = end.min(line + self.line_bytes) - line;
                out.push((line, from as usize, to as usize));
            }
            line += self.line_bytes;
        }
        out
    }

    fn ensure(line: &mut Line, bytes: usize) {
        if line.memory.is_empty() {
            line.memory = vec![0; bytes];
            line.processor = vec![0; bytes];
        }
    }

    /// A processor store of `len` bytes at `address`. `before` holds the
    /// bytes memory held there before the store.
    pub fn cpu_store(&mut self, address: u64, len: usize, before: &[u8]) {
        let bytes = self.line_bytes as usize;
        for (line_address, from, to) in self.touched(address, len) {
            let line = self.lines.entry(line_address).or_default();
            Self::ensure(line, bytes);
            line.cached = true;
            let source = (line_address + from as u64 - address) as usize;
            for i in from..to {
                let bit = 1u128 << i;
                if line.dirty & bit == 0 {
                    line.dirty |= bit;
                    line.memory[i] = before[source + i - from];
                }
                // The store replaces the processor's copy of this byte.
                if line.stale & bit != 0 {
                    line.stale &= !bit;
                }
            }
        }
    }

    /// A processor load of `len` bytes at `address`: the line becomes cached.
    /// Returns the stale bytes the load must see in place of memory's.
    pub fn cpu_load(&mut self, address: u64, len: usize) -> Option<StaleOverlay> {
        let mut overlay = StaleOverlay::default();
        for (line_address, from, to) in self.touched(address, len) {
            let line = self.lines.entry(line_address).or_default();
            line.cached = true;
            for i in from..to {
                if line.stale & (1u128 << i) != 0 {
                    let at = line_address + i as u64;
                    match overlay.pieces.last_mut() {
                        Some((start, bytes)) if *start + bytes.len() as u64 == at => {
                            bytes.push(line.processor[i])
                        }
                        _ => overlay.pieces.push((at, vec![line.processor[i]])),
                    }
                }
            }
        }
        (!overlay.pieces.is_empty()).then_some(overlay)
    }

    /// A device read of `buffer.len()` bytes at `address`. `buffer` holds
    /// processor-view memory; dirty bytes are replaced by what memory holds.
    pub fn device_read(&self, address: u64, buffer: &mut [u8]) {
        for (line_address, from, to) in self.touched(address, buffer.len()) {
            let Some(line) = self.lines.get(&line_address) else {
                continue;
            };
            for i in from..to {
                if line.dirty & (1u128 << i) != 0 {
                    buffer[(line_address + i as u64 - address) as usize] = line.memory[i];
                }
            }
        }
    }

    /// A device write of `bytes` at `address`. `before` holds the processor
    /// view of those bytes before the write. Processor-cached bytes stay
    /// stale for loads; bytes that are also dirty update the memory image.
    pub fn device_write(&mut self, address: u64, bytes: &[u8], before: &[u8]) {
        let width = self.line_bytes as usize;
        for (line_address, from, to) in self.touched(address, bytes.len()) {
            let Some(line) = self.lines.get_mut(&line_address) else {
                continue;
            };
            if !line.cached {
                continue;
            }
            Self::ensure(line, width);
            for i in from..to {
                let bit = 1u128 << i;
                let at = (line_address + i as u64 - address) as usize;
                if line.dirty & bit != 0 {
                    line.memory[i] = bytes[at];
                }
                if line.stale & bit == 0 {
                    // Dirty bytes keep the processor's own value.
                    if line.dirty & bit == 0 || before[at] != bytes[at] {
                        line.stale |= bit;
                        line.processor[i] = before[at];
                    }
                }
            }
        }
    }

    fn patches(line_address: u64, bits: u128, source: &[u8]) -> Vec<Patch> {
        let mut out: Vec<Patch> = Vec::new();
        for (i, &byte) in source.iter().enumerate() {
            if bits & (1u128 << i) == 0 {
                continue;
            }
            let at = line_address + i as u64;
            match out.last_mut() {
                Some(p) if p.address + p.bytes.len() as u64 == at => p.bytes.push(byte),
                _ => out.push(Patch {
                    address: at,
                    bytes: vec![byte],
                }),
            }
        }
        out
    }

    fn clean_inner(&mut self, line_address: u64) -> Vec<Patch> {
        let Some(line) = self.lines.get_mut(&line_address) else {
            return Vec::new();
        };
        if line.dirty == 0 {
            return Vec::new();
        }
        // Dirty bytes a device overwrote: the processor's value reaches
        // memory and the copies agree again.
        let conflict = line.dirty & line.stale;
        let patches = Self::patches(line_address, conflict, &line.processor);
        line.dirty = 0;
        line.stale &= !conflict;
        patches
    }

    fn invalidate_inner(&mut self, line_address: u64) -> Vec<Patch> {
        let Some(line) = self.lines.get_mut(&line_address) else {
            return Vec::new();
        };
        // Discarded stores revert memory to what it held.
        let patches = Self::patches(line_address, line.dirty, &line.memory);
        *line = Line::default();
        self.lines.remove(&line_address);
        patches
    }

    fn line_of(&self, physical: u64) -> Option<u64> {
        let line = physical & !(self.line_bytes - 1);
        self.covers(line).then_some(line)
    }

    /// DCCMVAC: write the line back to memory and keep it cached.
    pub fn clean_line(&mut self, physical: u64) -> Vec<Patch> {
        self.line_of(physical)
            .map_or_else(Vec::new, |line| self.clean_inner(line))
    }

    /// DCIMVAC: discard the line without writing it back.
    pub fn invalidate_line(&mut self, physical: u64) -> Vec<Patch> {
        self.line_of(physical)
            .map_or_else(Vec::new, |line| self.invalidate_inner(line))
    }

    /// DCCIMVAC.
    pub fn clean_invalidate_line(&mut self, physical: u64) -> Vec<Patch> {
        let Some(line) = self.line_of(physical) else {
            return Vec::new();
        };
        let mut patches = self.clean_inner(line);
        patches.extend(self.invalidate_inner(line));
        patches
    }

    /// Clean by set and way over every tracked line.
    pub fn clean_all(&mut self) -> Vec<Patch> {
        let lines: Vec<u64> = self.lines.keys().copied().collect();
        lines
            .into_iter()
            .flat_map(|l| self.clean_inner(l))
            .collect()
    }

    /// Invalidate by set and way over every tracked line.
    pub fn invalidate_all(&mut self) -> Vec<Patch> {
        let lines: Vec<u64> = self.lines.keys().copied().collect();
        lines
            .into_iter()
            .flat_map(|l| self.invalidate_inner(l))
            .collect()
    }

    /// Clean and invalidate by set and way over every tracked line.
    pub fn clean_invalidate_all(&mut self) -> Vec<Patch> {
        let lines: Vec<u64> = self.lines.keys().copied().collect();
        lines
            .into_iter()
            .flat_map(|l| {
                let mut p = self.clean_inner(l);
                p.extend(self.invalidate_inner(l));
                p
            })
            .collect()
    }
}
