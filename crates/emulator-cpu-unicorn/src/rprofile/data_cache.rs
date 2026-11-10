//! Private, non-coherent L1 data caches of R-profile processors that share
//! RAM.
//!
//! Each processor's data cache is private and nothing keeps the caches
//! coherent with each other, with another alias of the same memory or with
//! bus masters. Execution keeps one coherent copy of memory, so it cannot
//! show what a stale or late-evicted line does on the processor. This model
//! tracks which lines each cache holds, from every load and store to external
//! RAM, the cache maintenance instructions and its own evictions, and refuses
//! the access that would expose an incoherent line:
//!
//! - a cache hit on a line whose memory another agent changed since the
//!   line was filled (a load returns old bytes; a store makes old bytes
//!   dirty, and their eviction later overwrites the other agent's), and
//! - any other access to memory while a cache holds the line dirty: a cache
//!   fill or a load through an uncached alias reads bytes the dirty line will
//!   replace, and a memory write is lost when the dirty line is evicted.
//!
//! Each set replaces its ways round-robin, the Cortex-R4/R5 deterministic
//! replacement policy, so a line written long ago leaves the cache the way it
//! would on the processor. Cacheability comes from the processor's SCTLR and
//! PMSAv7 regions.
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// L1 data-cache geometry, as the processor's CCSIDR reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Geometry {
    pub line_bytes: u32,
    pub sets: u32,
    pub ways: u32,
}
impl Geometry {
    fn validate(self) -> Result<(), String> {
        if !self.line_bytes.is_power_of_two()
            || !(16..=4096).contains(&self.line_bytes)
            || !self.sets.is_power_of_two()
            || self.sets > 1 << 15
            || !self.ways.is_power_of_two()
            || self.ways > 32
        {
            return Err("invalid R-profile data-cache geometry".into());
        }
        Ok(())
    }
    fn shift(self) -> u32 {
        self.line_bytes.trailing_zeros()
    }
    /// CCSIDR for the level-1 data cache: write-through, write-back,
    /// read-allocate and write-allocate supported.
    pub fn ccsidr(self) -> u32 {
        0xf << 28 | (self.sets - 1) << 13 | (self.ways - 1) << 3 | (self.shift() - 4)
    }
}

/// The data-cache model of one processor: its geometry and the physical
/// ranges whose accesses the model follows. Following every access to RAM
/// costs a slow-path callback per load and store, so a run names the memory
/// it checks; an empty list checks nothing. Accesses elsewhere are not seen,
/// so their evictions are not either: a checked line stays resident until
/// checked traffic or maintenance removes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub line_bytes: u32,
    pub sets: u32,
    pub ways: u32,
    /// Physical `[begin, end)` ranges.
    #[serde(default)]
    pub checked: Vec<(u64, u64)>,
}
impl Options {
    pub fn geometry(&self) -> Geometry {
        Geometry {
            line_bytes: self.line_bytes,
            sets: self.sets,
            ways: self.ways,
        }
    }
}

/// How a data access through one address uses the L1 data cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Policy {
    /// Write-back, write-allocate.
    WriteBackAllocate,
    /// Write-back; a store that misses writes memory without allocating.
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
        (0, 2) | (4..=7, 2) => Policy::WriteThrough,
        (0, 3) | (4..=7, 3) => Policy::WriteBack,
        (1, 3) | (4..=7, 1) => Policy::WriteBackAllocate,
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
const MAX_PROCESSORS: usize = 8;

/// Per physical line: the PC of the store that made it dirty (low word), the
/// dirty holder's index plus one, the caches holding it, and the caches whose
/// copy is older than memory or another cache's dirty copy.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Line(u64);
impl Line {
    fn pc(self) -> u32 {
        self.0 as u32
    }
    fn owner(self) -> Option<usize> {
        match (self.0 >> 32) as u8 {
            0 => None,
            owner => Some(owner as usize - 1),
        }
    }
    fn held(self) -> u8 {
        (self.0 >> 40) as u8
    }
    fn stale(self) -> u8 {
        (self.0 >> 48) as u8
    }
    fn with(owner: Option<usize>, pc: u32, held: u8, stale: u8) -> Self {
        let owner = owner.map_or(0, |core| core as u64 + 1);
        Self(pc as u64 | owner << 32 | (held as u64) << 40 | (stale as u64) << 48)
    }
    fn set_owner(self, owner: Option<usize>, pc: u32) -> Self {
        Self::with(owner, pc, self.held(), self.stale())
    }
    fn set_held(self, held: u8, stale: u8) -> Self {
        Self::with(self.owner(), self.pc(), held, stale)
    }
}

/// Line state shared by every data cache of one memory space.
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
    fn get(&self, line: u32) -> Line {
        Line(
            self.chunks[line as usize / CHUNK_LINES]
                .get()
                .map_or(0, |chunk| {
                    chunk[line as usize % CHUNK_LINES].load(Ordering::Relaxed)
                }),
        )
    }
    fn update(&self, line: u32, f: impl Fn(Line) -> Line) {
        let chunk = self.chunks[line as usize / CHUNK_LINES]
            .get_or_init(|| (0..CHUNK_LINES).map(|_| AtomicU64::new(0)).collect());
        let _ = chunk[line as usize % CHUNK_LINES].try_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |entry| Some(f(Line(entry)).0),
        );
    }
}

/// An access the model refuses.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Hazard {
    /// This cache hit a line whose memory changed after the fill.
    Stale { line: u64 },
    /// Memory or a fill met a line another cache, or this one, holds dirty.
    Dirty { line: u64, holder: usize, pc: u32 },
}

const EMPTY: u32 = u32::MAX;

/// One processor's data cache: the line in each way of each set.
pub(crate) struct Cache {
    core: usize,
    checked: Vec<(u64, u64)>,
    lines: std::sync::Arc<Lines>,
    ways: Vec<u32>,
    next: Vec<u8>,
}
impl Cache {
    pub fn new(
        core: usize,
        lines: std::sync::Arc<Lines>,
        checked: Vec<(u64, u64)>,
    ) -> Result<Self, String> {
        if checked
            .iter()
            .any(|&(begin, end)| begin >= end || end > 1 << 32)
        {
            return Err("invalid checked data-cache range".into());
        }
        if core >= MAX_PROCESSORS {
            return Err("data-cache model supports at most eight processors".into());
        }
        let geometry = lines.geometry();
        Ok(Self {
            core,
            checked,
            lines,
            ways: vec![EMPTY; (geometry.sets * geometry.ways) as usize],
            next: vec![0; geometry.sets as usize],
        })
    }
    pub fn geometry(&self) -> Geometry {
        self.lines.geometry()
    }
    /// The part of `begin..end` (physical) this model follows.
    pub fn checked_parts(&self, begin: u64, end: u64) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.checked
            .iter()
            .map(move |&(b, e)| (b.max(begin), e.min(end)))
            .filter(|&(b, e)| b < e)
    }
    pub fn checks(&self, physical: u64) -> bool {
        self.checked
            .iter()
            .any(|&(b, e)| b <= physical && physical < e)
    }
    fn mine(&self) -> u8 {
        1 << self.core
    }
    fn set(&self, line: u32) -> usize {
        line as usize & (self.geometry().sets as usize - 1)
    }
    fn slots(&self, set: usize) -> std::ops::Range<usize> {
        let ways = self.geometry().ways as usize;
        set * ways..(set + 1) * ways
    }
    fn find(&self, line: u32) -> Option<usize> {
        self.slots(self.set(line))
            .find(|&slot| self.ways[slot] == line)
    }
    pub fn line_address(&self, line: u64) -> u64 {
        line << self.geometry().shift()
    }
    /// Physical lines touched by `bytes` bytes starting at `physical`.
    fn lines_of(&self, physical: u64, bytes: usize) -> std::ops::RangeInclusive<u32> {
        let shift = self.geometry().shift();
        (physical >> shift) as u32..=((physical + bytes.max(1) as u64 - 1) >> shift) as u32
    }
    /// Memory changes: every other cache's copy is now older than memory.
    fn memory_written(&self, line: u32) {
        let mine = self.mine();
        self.lines.update(line, |l| {
            l.set_held(l.held(), l.stale() | (l.held() & !mine))
        });
    }
    /// Memory is read or written by something other than a cache hit. A
    /// dirty line anywhere holds newer bytes, or will overwrite these.
    fn memory_access(&self, line: u32) -> Option<Hazard> {
        let state = self.lines.get(line);
        state.owner().map(|holder| Hazard::Dirty {
            line: line as u64,
            holder,
            pc: state.pc(),
        })
    }
    /// Drop a line, writing it back if dirty. The model holds no bytes, so a
    /// writeback and a discard differ only in memory contents.
    fn evict(&mut self, slot: usize) {
        let victim = std::mem::replace(&mut self.ways[slot], EMPTY);
        if victim == EMPTY {
            return;
        }
        let (core, mine) = (self.core, self.mine());
        self.lines.update(victim, |l| {
            let l = if l.owner() == Some(core) {
                l.set_owner(None, 0)
            } else {
                l
            };
            l.set_held(l.held() & !mine, l.stale() & !mine)
        });
    }
    /// Write a dirty line back and keep it.
    fn clean(&mut self, slot: usize) {
        let line = self.ways[slot];
        if line == EMPTY {
            return;
        }
        let core = self.core;
        self.lines.update(line, |l| {
            if l.owner() == Some(core) {
                l.set_owner(None, 0)
            } else {
                l
            }
        });
    }
    fn fill(&mut self, line: u32) -> Result<usize, Hazard> {
        if let Some(hazard) = self.memory_access(line) {
            return Err(hazard);
        }
        let set = self.set(line);
        let slot = self.slots(set).start + self.next[set] as usize;
        self.next[set] = ((self.next[set] as u32 + 1) % self.geometry().ways) as u8;
        self.evict(slot);
        self.ways[slot] = line;
        let mine = self.mine();
        self.lines
            .update(line, |l| l.set_held(l.held() | mine, l.stale() & !mine));
        Ok(slot)
    }
    /// A processor load or store through `policy`. `pc` is read only when a
    /// store makes a line dirty.
    pub fn access(
        &mut self,
        write: bool,
        policy: Policy,
        physical: u64,
        bytes: usize,
        pc: impl Fn() -> Result<u32, String>,
    ) -> Result<Option<Hazard>, String> {
        for line in self.lines_of(physical, bytes) {
            let hit = self.find(line);
            if hit.is_some() && self.lines.get(line).stale() & self.mine() != 0 {
                return Ok(Some(Hazard::Stale { line: line as u64 }));
            }
            match (policy, write, hit) {
                (Policy::NonCacheable, _, _)
                | (Policy::WriteBack | Policy::WriteThrough, true, None) => {
                    if let Some(hazard) = self.memory_access(line) {
                        return Ok(Some(hazard));
                    }
                    if write {
                        self.memory_written(line);
                    }
                }
                (Policy::WriteThrough, true, Some(_)) => self.memory_written(line),
                (_, false, Some(_)) => {}
                (_, false, None) => {
                    if let Err(hazard) = self.fill(line) {
                        return Ok(Some(hazard));
                    }
                }
                (Policy::WriteBackAllocate | Policy::WriteBack, true, hit) => {
                    if hit.is_none() {
                        if let Err(hazard) = self.fill(line) {
                            return Ok(Some(hazard));
                        }
                    }
                    if self.lines.get(line).owner() != Some(self.core) {
                        let (core, pc) = (self.core, pc()?);
                        let mine = self.mine();
                        self.lines.update(line, |l| {
                            l.set_owner(Some(core), pc)
                                .set_held(l.held(), l.stale() | (l.held() & !mine))
                        });
                    }
                }
            }
        }
        Ok(None)
    }
    /// Maintenance of the line holding `physical`, if this cache holds it.
    pub fn maintain_line(&mut self, physical: u64, clean: bool, invalidate: bool) {
        let line = (physical >> self.geometry().shift()) as u32;
        if let Some(slot) = self.find(line) {
            self.maintain(slot, clean, invalidate);
        }
    }
    /// Maintenance by set and way.
    pub fn maintain_way(&mut self, set: usize, way: usize, clean: bool, invalidate: bool) {
        let slot = self.slots(set).start + way;
        self.maintain(slot, clean, invalidate);
    }
    fn maintain(&mut self, slot: usize, clean: bool, invalidate: bool) {
        if invalidate {
            self.evict(slot);
        } else if clean {
            self.clean(slot);
        }
    }
    pub fn invalidate_all(&mut self) {
        for slot in 0..self.ways.len() {
            self.evict(slot);
        }
    }
    /// The held lines by slot, with the dirty PC (or none) and staleness,
    /// and each set's next victim.
    pub fn save(&self) -> Saved {
        let mine = self.mine();
        Saved {
            ways: self
                .ways
                .iter()
                .map(|&line| {
                    if line == EMPTY {
                        return None;
                    }
                    let state = self.lines.get(line);
                    Some((
                        line,
                        (state.owner() == Some(self.core)).then(|| state.pc()),
                        state.stale() & mine != 0,
                    ))
                })
                .collect(),
            next: self.next.clone(),
        }
    }
    pub fn restore(&mut self, saved: &Saved) -> Result<(), String> {
        if saved.ways.len() != self.ways.len() || saved.next.len() != self.next.len() {
            return Err("data-cache snapshot geometry differs".into());
        }
        // Drop this cache's current lines without counting writebacks; the
        // board restores memory itself. Snapshots of the other processors
        // restore their own lines.
        self.invalidate_all();
        let (core, mine) = (self.core, self.mine());
        for (slot, entry) in saved.ways.iter().enumerate() {
            let Some((line, dirty, stale)) = *entry else {
                continue;
            };
            self.ways[slot] = line;
            self.lines.update(line, |l| {
                let l = match dirty {
                    Some(pc) => l.set_owner(Some(core), pc),
                    None => l,
                };
                let stale = if stale {
                    l.stale() | mine
                } else {
                    l.stale() & !mine
                };
                l.set_held(l.held() | mine, stale)
            });
        }
        self.next = saved.next.clone();
        Ok(())
    }
}

/// A cache's lines for a processor snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Saved {
    ways: Vec<Option<(u32, Option<u32>, bool)>>,
    next: Vec<u8>,
}

/// Instructions the cache model consumes.
pub(crate) enum Operation {
    /// SCTLR and MPU writes change cacheability.
    Attributes,
    Line {
        clean: bool,
        invalidate: bool,
    },
    Way {
        clean: bool,
        invalidate: bool,
    },
    InvalidateAll,
}
pub(crate) fn operation(opc1: u32, crn: u32, crm: u32, opc2: u32) -> Option<Operation> {
    let kind = |crm| match crm {
        6 => Some((false, true)),
        10 | 11 => Some((true, false)),
        14 => Some((true, true)),
        _ => None,
    };
    match (opc1, crn, crm, opc2) {
        (0, 1, 0, 0) | (0, 6, _, _) => Some(Operation::Attributes),
        // DCIMVAC, DCCMVAC, DCCMVAU, DCCIMVAC.
        (0, 7, crm, 1) => {
            kind(crm).map(|(clean, invalidate)| Operation::Line { clean, invalidate })
        }
        // DCISW, DCCSW, DCCISW.
        (0, 7, crm @ (6 | 10 | 14), 2) => {
            kind(crm).map(|(clean, invalidate)| Operation::Way { clean, invalidate })
        }
        // Cortex-R4/R5 invalidate all data cache.
        (0, 15, 5, 0) => Some(Operation::InvalidateAll),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
