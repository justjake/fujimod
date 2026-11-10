//! Private, non-coherent write-back L1 data caches of R-profile processors
//! that share RAM.
//!
//! Each processor's data cache is private and nothing keeps the caches
//! coherent with each other, with the other cached alias or with bus masters.
//! A line one processor wrote through a write-back mapping stays dirty in that
//! processor until it cleans the line; its eviction then writes the whole line
//! back over whatever another agent stored there meanwhile. Execution models
//! no cache contents, so it cannot reproduce the corruption. It refuses the
//! access that would expose it instead: a cached access, or a write to the
//! line's memory by any other path, while another processor (or, for memory
//! writes, the same processor's cache) holds the line dirty.
//!
//! The state is one owner per physical line, set by the first write-back store
//! after the owner's last clean of the line. Cleaning by address or by set/way
//! and the whole-cache operations clear it. Eviction is not modeled: a line
//! stays dirty until its owner cleans it, so a refusal names an access that is
//! hazardous on the processor whenever the line has not been evicted yet.
//! Cacheability comes from the processor's SCTLR and PMSAv7 regions.
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// L1 data-cache geometry, as the processor's CCSIDR reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Geometry {
    pub line_bytes: u32,
    pub sets: u32,
}
impl Geometry {
    fn validate(self) -> Result<(), String> {
        if !self.line_bytes.is_power_of_two()
            || !(16..=4096).contains(&self.line_bytes)
            || !self.sets.is_power_of_two()
            || self.sets > 1 << 15
        {
            return Err("invalid R-profile data-cache geometry".into());
        }
        Ok(())
    }
    fn shift(self) -> u32 {
        self.line_bytes.trailing_zeros()
    }
}

/// How a data access through one address uses the L1 data cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Policy {
    WriteBack,
    WriteThrough,
    NonCacheable,
}

/// PMSAv7 data memory attributes for `address`. Regions with a higher number
/// take priority. An address no enabled region covers, a disabled MPU or a
/// disabled data cache is non-cacheable. Shareable Normal memory is not
/// cached by the Cortex-R4/R5 L1 data cache.
pub(crate) fn policy(sctlr: u32, regions: &[(u32, u32, u32)], address: u32) -> Policy {
    if sctlr & 1 == 0 || sctlr & (1 << 2) == 0 {
        return Policy::NonCacheable;
    }
    for &(base, size, access) in regions.iter().rev() {
        if size & 1 == 0 {
            continue;
        }
        let bits = ((size >> 1) & 31) + 1;
        if bits < 5 {
            continue;
        }
        let mask = if bits >= 32 {
            u32::MAX
        } else {
            (1u32 << bits) - 1
        };
        if address & !mask != base & !mask {
            continue;
        }
        if bits >= 8 {
            let subregion = (address & mask) >> (bits - 3);
            if size >> 8 & (1 << subregion) != 0 {
                continue;
            }
        }
        return attributes(access);
    }
    Policy::NonCacheable
}

fn attributes(access: u32) -> Policy {
    let (tex, shareable, cb) = ((access >> 3) & 7, access & 4 != 0, access & 3);
    let inner = match (tex, cb) {
        (0, 2) => Policy::WriteThrough,
        (0, 3) | (1, 3) => Policy::WriteBack,
        (4..=7, 1 | 3) => Policy::WriteBack,
        (4..=7, 2) => Policy::WriteThrough,
        _ => Policy::NonCacheable,
    };
    if shareable {
        Policy::NonCacheable
    } else {
        inner
    }
}

/// One contiguous processor address range with a single policy, and the
/// physical address of its first byte.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Span {
    pub begin: u64,
    pub end: u64,
    pub physical: u64,
    pub policy: Policy,
}

/// Split each mapped range at every region and subregion edge, then give
/// each piece the policy of the region that wins there.
pub(crate) fn spans(
    sctlr: u32,
    regions: &[(u32, u32, u32)],
    maps: &[(u64, u64, u64)],
) -> Vec<Span> {
    let mut edges = Vec::new();
    for &(base, size, _) in regions {
        if size & 1 == 0 {
            continue;
        }
        let bits = ((size >> 1) & 31) + 1;
        let bytes = 1u64 << bits;
        let base = base as u64 & !(bytes - 1);
        for part in 0..=8 {
            edges.push(base + bytes / 8 * part);
        }
    }
    let mut result = Vec::new();
    for &(begin, end, physical) in maps {
        let mut cuts: Vec<u64> = edges
            .iter()
            .copied()
            .filter(|&edge| begin < edge && edge < end)
            .collect();
        cuts.push(end);
        cuts.sort_unstable();
        cuts.dedup();
        let mut cursor = begin;
        for cut in cuts {
            let policy = policy(sctlr, regions, cursor as u32);
            match result.last_mut() {
                Some(Span {
                    end: last_end,
                    physical: last_physical,
                    begin: last_begin,
                    policy: last_policy,
                }) if *last_end == cursor
                    && *last_policy == policy
                    && *last_physical + (cursor - *last_begin) == physical + (cursor - begin) =>
                {
                    *last_end = cut;
                }
                _ => result.push(Span {
                    begin: cursor,
                    end: cut,
                    physical: physical + (cursor - begin),
                    policy,
                }),
            }
            cursor = cut;
        }
    }
    result
}

const CHUNK_LINES: usize = 4096;
const OWNER_SHIFT: u32 = 56;

/// The dirty owner of every physical line, shared by all processors of one
/// memory space. An entry holds the owner's index plus one in its top byte
/// and the PC of the store that made the line dirty in its low word.
pub(crate) struct Lines {
    geometry: Geometry,
    chunks: Box<[OnceLock<Box<[AtomicU64]>>]>,
}
impl Lines {
    pub fn new(geometry: Geometry) -> Result<Self, String> {
        geometry.validate()?;
        let lines = (1u64 << 32) >> geometry.shift();
        Ok(Self {
            geometry,
            chunks: (0..(lines as usize).div_ceil(CHUNK_LINES))
                .map(|_| OnceLock::new())
                .collect(),
        })
    }
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }
    fn entry(&self, line: u32) -> &AtomicU64 {
        let chunk = self.chunks[line as usize / CHUNK_LINES]
            .get_or_init(|| (0..CHUNK_LINES).map(|_| AtomicU64::new(0)).collect());
        &chunk[line as usize % CHUNK_LINES]
    }
    fn peek(&self, line: u32) -> u64 {
        self.chunks[line as usize / CHUNK_LINES]
            .get()
            .map_or(0, |chunk| {
                chunk[line as usize % CHUNK_LINES].load(Ordering::Relaxed)
            })
    }
}

/// A line held dirty by a processor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Dirty {
    pub core: usize,
    pub pc: u32,
}
fn decode(entry: u64) -> Option<Dirty> {
    (entry != 0).then(|| Dirty {
        core: (entry >> OWNER_SHIFT) as usize - 1,
        pc: entry as u32,
    })
}

/// A data access the cache model refuses.
pub(crate) struct Hazard {
    pub line: u64,
    pub holder: Dirty,
}

/// One processor's view: its dirty lines indexed by set, for set/way
/// maintenance. An index may hold lines since cleaned by address; those are
/// dropped when the set is next walked or compacted.
pub(crate) struct Cache {
    core: usize,
    lines: std::sync::Arc<Lines>,
    sets: Vec<Vec<u32>>,
    compact_at: usize,
    dirty: usize,
}
impl Cache {
    pub fn new(core: usize, lines: std::sync::Arc<Lines>) -> Self {
        let sets = lines.geometry().sets as usize;
        Self {
            core,
            lines,
            sets: vec![Vec::new(); sets],
            compact_at: 1 << 16,
            dirty: 0,
        }
    }
    pub fn geometry(&self) -> Geometry {
        self.lines.geometry()
    }
    fn line(&self, physical: u64) -> u32 {
        (physical >> self.geometry().shift()) as u32
    }
    fn set(&self, line: u32) -> usize {
        line as usize & (self.sets.len() - 1)
    }
    pub fn line_address(&self, line: u64) -> u64 {
        line << self.geometry().shift()
    }
    /// Physical lines touched by `bytes` bytes starting at `physical`.
    pub fn lines_of(&self, physical: u64, bytes: usize) -> std::ops::RangeInclusive<u64> {
        let shift = self.geometry().shift();
        (physical >> shift)..=((physical + bytes.max(1) as u64 - 1) >> shift)
    }
    /// A processor load or store through `policy`. `pc` is read only when
    /// a store makes a line dirty.
    pub fn access(
        &mut self,
        write: bool,
        policy: Policy,
        physical: u64,
        bytes: usize,
        pc: impl Fn() -> Result<u32, String>,
    ) -> Result<Option<Hazard>, String> {
        for line in self.lines_of(physical, bytes) {
            let line = line as u32;
            let current = decode(self.lines.peek(line));
            let mine = current.is_some_and(|d| d.core == self.core);
            let hazard = match (policy, write) {
                // The cache serves this access only if no other cache holds
                // newer bytes of the line.
                (Policy::WriteBack | Policy::WriteThrough, false) => current.is_some() && !mine,
                (Policy::WriteBack, true) => {
                    if current.is_none() {
                        let entry = ((self.core as u64 + 1) << OWNER_SHIFT) | pc()? as u64;
                        self.lines.entry(line).store(entry, Ordering::Relaxed);
                        let set = self.set(line);
                        self.sets[set].push(line);
                        self.dirty += 1;
                        if self.dirty >= self.compact_at {
                            self.compact();
                        }
                    }
                    current.is_some() && !mine
                }
                // Memory changes under a dirty line, which an eviction will
                // later overwrite, including from this processor's own cache.
                (Policy::WriteThrough | Policy::NonCacheable, true) => current.is_some(),
                (Policy::NonCacheable, false) => false,
            };
            if hazard {
                return Ok(Some(Hazard {
                    line: line as u64,
                    holder: current.unwrap(),
                }));
            }
        }
        Ok(None)
    }
    /// Clean (and possibly invalidate) the line holding `physical`. An
    /// invalidate discards the line; either way this cache no longer holds it
    /// dirty.
    pub fn clean_line(&mut self, physical: u64) {
        let line = self.line(physical);
        self.release(line);
    }
    /// Clean every dirty line of one set, for a set/way operation on any way.
    pub fn clean_set(&mut self, set: usize) {
        let lines = std::mem::take(&mut self.sets[set]);
        for line in lines {
            self.release(line);
        }
    }
    pub fn clean_all(&mut self) {
        for set in 0..self.sets.len() {
            self.clean_set(set);
        }
        self.dirty = 0;
    }
    fn release(&mut self, line: u32) {
        let entry = self.lines.peek(line);
        if decode(entry).is_some_and(|d| d.core == self.core) {
            self.lines.entry(line).store(0, Ordering::Relaxed);
        }
    }
    fn compact(&mut self) {
        let (core, lines) = (self.core, &self.lines);
        let mut live = 0;
        for set in &mut self.sets {
            set.sort_unstable();
            set.dedup();
            set.retain(|&line| decode(lines.peek(line)).is_some_and(|d| d.core == core));
            live += set.len();
        }
        self.dirty = live;
        self.compact_at = (live * 2).max(1 << 16);
    }
    /// This processor's dirty lines and the PCs that dirtied them.
    pub fn save(&mut self) -> Vec<(u32, u32)> {
        self.compact();
        let mut saved: Vec<(u32, u32)> = self
            .sets
            .iter()
            .flatten()
            .map(|&line| (line, self.lines.peek(line) as u32))
            .collect();
        saved.sort_unstable();
        saved
    }
    pub fn restore(&mut self, saved: &[(u32, u32)]) {
        self.clean_all();
        // Saved states hold disjoint lines. A line another processor holds
        // now is that processor's pre-restore state, which its own restore
        // discards.
        for &(line, pc) in saved {
            self.lines.entry(line).store(
                ((self.core as u64 + 1) << OWNER_SHIFT) | pc as u64,
                Ordering::Relaxed,
            );
            let set = self.set(line);
            self.sets[set].push(line);
        }
        self.dirty = saved.len();
    }
}

/// Instructions the cache model consumes: SCTLR and MPU writes change
/// cacheability; cache maintenance changes dirty state.
pub(crate) enum Operation {
    Attributes,
    CleanLine,
    CleanSet,
    CleanAll,
}
pub(crate) fn operation(opc1: u32, crn: u32, crm: u32, opc2: u32) -> Option<Operation> {
    match (opc1, crn, crm, opc2) {
        (0, 1, 0, 0) | (0, 6, _, _) => Some(Operation::Attributes),
        // DCIMVAC, DCCMVAC, DCCMVAU, DCCIMVAC.
        (0, 7, 6 | 10 | 11 | 14, 1) => Some(Operation::CleanLine),
        // DCISW, DCCSW, DCCISW.
        (0, 7, 6 | 10 | 14, 2) => Some(Operation::CleanSet),
        // Cortex-R4/R5 invalidate all data cache.
        (0, 15, 5, 0) => Some(Operation::CleanAll),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
