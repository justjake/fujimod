//! Bounded aggregate RAM-byte preservation over CPU stores and bus writes.
use super::{
    Board, State,
    factory::MemoryMap,
    observation::{tcm_overlap, union},
    scalar,
};
use fujimod_emulator_cpu::{Access, Cpu, Hook, HookId, HookRange, Model, Register};
use serde::{Serialize, ser::SerializeSeq as _};
use std::{
    cell::RefCell,
    ffi::c_void,
    rc::{Rc, Weak},
};

const MAX_CPU_STORE: usize = 8;
/// A wider store starting this far before a protected byte can still overlap.
const HOOK_REACH: u64 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct PhysicalAddress(u64);

impl PhysicalAddress {
    pub fn new(value: u64) -> Result<Self, String> {
        if value > u32::MAX as u64 {
            return Err("physical address exceeds 32 bits".into());
        }
        Ok(Self(value))
    }
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct CpuAddress(u64);

impl CpuAddress {
    pub fn new(value: u64) -> Result<Self, String> {
        if value > u32::MAX as u64 {
            return Err("CPU address exceeds 32 bits".into());
        }
        Ok(Self(value))
    }
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysicalRange {
    start: PhysicalAddress,
    end: PhysicalAddress,
}

impl PhysicalRange {
    pub fn new(start: u64, end: u64) -> Result<Self, String> {
        let start = PhysicalAddress::new(start)?;
        let end = PhysicalAddress::new(end)?;
        if start >= end {
            return Err("protected range must be nonempty".into());
        }
        Ok(Self { start, end })
    }
    pub fn start(self) -> u64 {
        self.start.get()
    }
    pub fn end(self) -> u64 {
        self.end.get()
    }
    fn overlaps(self, physical: PhysicalAddress, bytes: WriteSize) -> bool {
        self.overlaps_bytes(physical, bytes.get())
    }
    fn overlaps_bytes(self, physical: PhysicalAddress, bytes: u64) -> bool {
        physical.get() < self.end.get() && self.start.get() < physical.get().saturating_add(bytes)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtectedRanges(Vec<PhysicalRange>);

impl ProtectedRanges {
    pub fn new(ranges: Vec<PhysicalRange>) -> Result<Self, String> {
        if ranges.is_empty() || ranges.len() > 8 {
            return Err("preservation requires 1..8 protected ranges".into());
        }
        Ok(Self(ranges))
    }
    pub fn ranges(&self) -> &[PhysicalRange] {
        &self.0
    }
}

impl Serialize for ProtectedRanges {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for range in &self.0 {
            sequence.serialize_element(&[range.start(), range.end()])?;
        }
        sequence.end()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CpuRange {
    start: CpuAddress,
    end: CpuAddress,
}

impl CpuRange {
    fn new(start: CpuAddress, end: CpuAddress) -> Result<Self, String> {
        if start >= end {
            return Err("CPU range must be nonempty".into());
        }
        Ok(Self { start, end })
    }
    fn start(self) -> u64 {
        self.start.get()
    }
    fn end(self) -> u64 {
        self.end.get()
    }
    fn overlaps(self, address: CpuAddress, width: u64) -> bool {
        address.get() < self.end.get() && self.start.get() < address.get().saturating_add(width)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct WriteCount(u64);

impl WriteCount {
    pub fn get(self) -> u64 {
        self.0
    }
    fn increment(&mut self) -> bool {
        match self.0.checked_add(1) {
            Some(next) => {
                self.0 = next;
                false
            }
            None => {
                self.0 = u64::MAX;
                true
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct InstructionCount(u64);

impl InstructionCount {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct CoreId(u32);

impl CoreId {
    pub fn new(core: usize) -> Result<Self, String> {
        u32::try_from(core)
            .map(Self)
            .map_err(|_| "core exceeds 32 bits".into())
    }
    pub fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct StoreSize(u8);

impl StoreSize {
    fn new(bytes: usize) -> Result<Self, String> {
        if !(1..=MAX_CPU_STORE).contains(&bytes) {
            return Err("store size exceeds the verified width".into());
        }
        Ok(Self(bytes as u8))
    }
    pub fn get(self) -> usize {
        usize::from(self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct WriteSize(u64);

impl WriteSize {
    pub(super) fn new(bytes: usize) -> Result<Self, String> {
        if bytes == 0 {
            return Err("write size must be nonzero".into());
        }
        Ok(Self(bytes as u64))
    }
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreservationBytes {
    bytes: [u8; MAX_CPU_STORE],
    len: u8,
}

impl PreservationBytes {
    fn new(bytes: &[u8]) -> Self {
        let mut out = [0; MAX_CPU_STORE];
        out[..bytes.len()].copy_from_slice(bytes);
        Self {
            bytes: out,
            len: bytes.len() as u8,
        }
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

impl Serialize for PreservationBytes {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut text = [0u8; MAX_CPU_STORE * 2];
        for (index, &byte) in self.as_slice().iter().enumerate() {
            text[index * 2] = HEX[usize::from(byte >> 4)];
            text[index * 2 + 1] = HEX[usize::from(byte & 0x0f)];
        }
        let len = usize::from(self.len) * 2;
        let text = std::str::from_utf8(&text[..len])
            .map_err(|_| serde::ser::Error::custom("hex digits"))?;
        serializer.serialize_str(text)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PreservationBusSource {
    HostWrite,
    ModelBusWrite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PreservationFault {
    UnresolvedMapping,
    UnsupportedAccess,
    UnreadablePreimage,
    CounterOverflow,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PreservationViolation {
    CpuChange {
        core: CoreId,
        pc: CpuAddress,
        instructions: InstructionCount,
        address: CpuAddress,
        physical: PhysicalAddress,
        size: StoreSize,
        before: PreservationBytes,
        after: PreservationBytes,
    },
    UnverifiableWrite {
        source: PreservationBusSource,
        address: CpuAddress,
        physical: PhysicalAddress,
        size: WriteSize,
    },
    IntegrityFault {
        reason: PreservationFault,
    },
}

/// Integrity faults override earlier effects, mark counters incomplete and remain stable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PreservationState {
    enabled: bool,
    #[serde(serialize_with = "serialize_state_ranges")]
    ranges: Option<ProtectedRanges>,
    identity_cpu_writes: WriteCount,
    changed_cpu_writes: WriteCount,
    host_writes: WriteCount,
    model_bus_writes: WriteCount,
    violation: Option<PreservationViolation>,
}

fn serialize_state_ranges<S: serde::Serializer>(
    ranges: &Option<ProtectedRanges>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match ranges {
        Some(ranges) => ranges.serialize(serializer),
        None => serializer.serialize_seq(Some(0))?.end(),
    }
}

impl PreservationState {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ranges: None,
            identity_cpu_writes: WriteCount::default(),
            changed_cpu_writes: WriteCount::default(),
            host_writes: WriteCount::default(),
            model_bus_writes: WriteCount::default(),
            violation: None,
        }
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn ranges(&self) -> Option<&ProtectedRanges> {
        self.ranges.as_ref()
    }
    pub fn identity_cpu_writes(&self) -> WriteCount {
        self.identity_cpu_writes
    }
    pub fn changed_cpu_writes(&self) -> WriteCount {
        self.changed_cpu_writes
    }
    pub fn host_writes(&self) -> WriteCount {
        self.host_writes
    }
    pub fn model_bus_writes(&self) -> WriteCount {
        self.model_bus_writes
    }
    pub fn violation(&self) -> Option<&PreservationViolation> {
        self.violation.as_ref()
    }
}

pub(super) struct PreservationRecords {
    ranges: ProtectedRanges,
    merged: Vec<PhysicalRange>,
    identity_cpu_writes: WriteCount,
    changed_cpu_writes: WriteCount,
    host_writes: WriteCount,
    model_bus_writes: WriteCount,
    violation: Option<PreservationViolation>,
}

impl PreservationRecords {
    pub(super) fn state(&self) -> PreservationState {
        PreservationState {
            enabled: true,
            ranges: Some(self.ranges.clone()),
            identity_cpu_writes: self.identity_cpu_writes,
            changed_cpu_writes: self.changed_cpu_writes,
            host_writes: self.host_writes,
            model_bus_writes: self.model_bus_writes,
            violation: self.violation.clone(),
        }
    }
    pub(super) fn overlaps(&self, physical: PhysicalAddress, bytes: WriteSize) -> bool {
        self.merged
            .iter()
            .any(|range| range.overlaps(physical, bytes))
    }
    pub(super) fn fault(&mut self, reason: PreservationFault) {
        if matches!(
            self.violation,
            Some(PreservationViolation::IntegrityFault { .. })
        ) {
            return;
        }
        self.violation = Some(PreservationViolation::IntegrityFault { reason });
    }
    fn cpu_store(&mut self, changed: Option<PreservationViolation>) {
        if let Some(violation) = changed {
            let overflow = self.changed_cpu_writes.increment();
            if self.violation.is_none() {
                self.violation = Some(violation);
            }
            if overflow {
                self.fault(PreservationFault::CounterOverflow);
            }
        } else if self.identity_cpu_writes.increment() {
            self.fault(PreservationFault::CounterOverflow);
        }
    }
    pub(super) fn bus_write(
        &mut self,
        source: PreservationBusSource,
        physical: PhysicalAddress,
        address: CpuAddress,
        len: WriteSize,
    ) {
        if !self.overlaps(physical, len) {
            return;
        }
        let overflow = match source {
            PreservationBusSource::HostWrite => self.host_writes.increment(),
            PreservationBusSource::ModelBusWrite => self.model_bus_writes.increment(),
        };
        if self.violation.is_none() {
            self.violation = Some(PreservationViolation::UnverifiableWrite {
                source,
                address,
                physical,
                size: len,
            });
        }
        if overflow {
            self.fault(PreservationFault::CounterOverflow);
        }
    }
    pub(super) fn unresolved(&mut self) {
        self.fault(PreservationFault::UnresolvedMapping);
    }
}

pub struct Preservation {
    memory: Weak<super::memory::Memory>,
    records: Rc<RefCell<PreservationRecords>>,
    contexts: Vec<PreservationHooks>,
    ended: bool,
}

// Each core's callbacks share one stable allocation. Keep it only while at
// least one of that core's hooks may still reference it.
struct PreservationHooks {
    context: Box<PreservationContext>,
    hooks: Vec<HookId>,
}

impl Preservation {
    pub fn state(&self) -> PreservationState {
        if self.ended {
            PreservationState::disabled()
        } else {
            self.records.borrow().state()
        }
    }
    pub fn end(&mut self) -> Result<(), String> {
        if self.ended {
            return Ok(());
        }
        let mut failure = None;
        self.contexts.retain_mut(|entry| {
            let Some(cpu) = entry.context.cpu.upgrade() else {
                return false;
            };
            entry.hooks.retain(|hook| match cpu.remove_hook(*hook) {
                Ok(()) => false,
                Err(error) => {
                    failure.get_or_insert(error);
                    true
                }
            });
            !entry.hooks.is_empty()
        });
        if let Some(error) = failure {
            self.records
                .borrow_mut()
                .fault(PreservationFault::UnsupportedAccess);
            return Err(error);
        }
        if let Some(memory) = self.memory.upgrade() {
            memory.clear_preservation(&self.records);
        }
        self.ended = true;
        Ok(())
    }
}

impl Drop for Preservation {
    fn drop(&mut self) {
        if self.end().is_err() {
            // Failed removals may leave callbacks installed. Only their data
            // must outlive this guard; successfully detached contexts are gone.
            for entry in std::mem::take(&mut self.contexts) {
                std::mem::forget(entry.context);
            }
        }
    }
}

struct PreservationContext {
    state: Weak<RefCell<State>>,
    cpu: Weak<dyn Cpu>,
    records: Rc<RefCell<PreservationRecords>>,
    maps: Vec<MemoryMap>,
    spans: Vec<CpuRange>,
    merged: Vec<PhysicalRange>,
    core: CoreId,
}

impl PreservationContext {
    fn fault(&self, reason: PreservationFault) {
        let mut records = self.records.borrow_mut();
        records.fault(reason);
    }
    fn store(&self, address: u64, size: i32, value: i64) {
        let (Some(state), Some(cpu)) = (self.state.upgrade(), self.cpu.upgrade()) else {
            self.fault(PreservationFault::UnresolvedMapping);
            return;
        };
        let state = state.borrow();
        if size <= 0 {
            self.fault(PreservationFault::UnsupportedAccess);
            return;
        }
        let Ok(cpu_address) = CpuAddress::new(address) else {
            self.fault(PreservationFault::UnsupportedAccess);
            return;
        };
        let Ok(width) = StoreSize::new(size as usize) else {
            if self.overlaps_spans(cpu_address, size as u64) {
                self.fault(PreservationFault::UnsupportedAccess);
            }
            return;
        };
        if !self.overlaps_spans(cpu_address, width.get() as u64) {
            return;
        }
        match tcm_overlap(&*cpu, address, width.get() as u64) {
            Ok(false) => {}
            Ok(true) | Err(_) => {
                self.fault(PreservationFault::UnsupportedAccess);
                return;
            }
        }
        let physical = state
            .decoders
            .observed_physical(self.core.get() as usize, address, width.get() as u64)
            .or_else(|| {
                self.maps
                    .iter()
                    .find(|map| {
                        map.address <= address
                            && address.saturating_add(width.get() as u64)
                                <= map.address + map.size as u64
                    })
                    .map(|map| map.physical + address - map.address)
            });
        let Some(physical) = physical else {
            self.fault(PreservationFault::UnresolvedMapping);
            return;
        };
        let Ok(physical) = PhysicalAddress::new(physical) else {
            self.fault(PreservationFault::UnresolvedMapping);
            return;
        };
        if !self
            .merged
            .iter()
            .any(|range| range.overlaps_bytes(physical, width.get() as u64))
        {
            return;
        }
        let mut before = [0; MAX_CPU_STORE];
        if cpu
            .read_memory(address, &mut before[..width.get()])
            .is_err()
        {
            self.fault(PreservationFault::UnreadablePreimage);
            return;
        }
        let byte_order = match scalar(&*cpu, Register::Pstate) {
            Ok(pstate) => pstate & (1 << 9) != 0,
            Err(_) => {
                self.fault(PreservationFault::UnsupportedAccess);
                return;
            }
        };
        let mask = if width.get() == MAX_CPU_STORE {
            u64::MAX
        } else {
            (1u64 << (width.get() * 8)) - 1
        };
        let stored = value as u64 & mask;
        let mut after = [0; MAX_CPU_STORE];
        for index in 0..width.get() {
            let shift = if byte_order {
                (width.get() - 1 - index) * 8
            } else {
                index * 8
            };
            after[index] = (stored >> shift) as u8;
        }
        let mut changed = None;
        for range in &self.merged {
            let start = range.start().max(physical.get());
            let end = range.end().min(physical.get() + width.get() as u64);
            if start >= end {
                continue;
            }
            let offset = (start - physical.get()) as usize;
            let len = (end - start) as usize;
            if before[offset..offset + len] == after[offset..offset + len] {
                continue;
            }
            let progress = cpu
                .instruction_progress()
                .unwrap_or(state.accounted[self.core.get() as usize]);
            let Some(extra) = progress.checked_sub(state.accounted[self.core.get() as usize])
            else {
                self.fault(PreservationFault::CounterOverflow);
                return;
            };
            let Some(counted) = state
                .instructions
                .iter()
                .try_fold(0u64, |total, count| total.checked_add(*count))
            else {
                self.fault(PreservationFault::CounterOverflow);
                return;
            };
            let Some(instructions) = counted.checked_add(extra) else {
                self.fault(PreservationFault::CounterOverflow);
                return;
            };
            let Ok(pc) = scalar(&*cpu, Register::Pc)
                .map_err(|_| "pc".to_owned())
                .and_then(CpuAddress::new)
            else {
                self.fault(PreservationFault::UnsupportedAccess);
                return;
            };
            let Ok(store_size) = StoreSize::new(len) else {
                self.fault(PreservationFault::UnsupportedAccess);
                return;
            };
            let Ok(intersection) = CpuAddress::new(address + offset as u64) else {
                self.fault(PreservationFault::UnsupportedAccess);
                return;
            };
            let Ok(physical_start) = PhysicalAddress::new(start) else {
                self.fault(PreservationFault::UnsupportedAccess);
                return;
            };
            changed = Some(PreservationViolation::CpuChange {
                core: self.core,
                pc,
                instructions: InstructionCount(instructions),
                address: intersection,
                physical: physical_start,
                size: store_size,
                before: PreservationBytes::new(&before[offset..offset + len]),
                after: PreservationBytes::new(&after[offset..offset + len]),
            });
            break;
        }
        self.records.borrow_mut().cpu_store(changed);
    }
    fn overlaps_spans(&self, address: CpuAddress, width: u64) -> bool {
        self.spans.iter().any(|span| span.overlaps(address, width))
    }
    fn host_write(&self, address: u64, bytes: usize) {
        if bytes == 0 {
            return;
        }
        let Some(state) = self.state.upgrade() else {
            self.fault(PreservationFault::UnresolvedMapping);
            return;
        };
        let state = state.borrow();
        let Ok(size) = WriteSize::new(bytes) else {
            self.fault(PreservationFault::UnsupportedAccess);
            return;
        };
        let Ok(cpu_address) = CpuAddress::new(address) else {
            self.fault(PreservationFault::UnsupportedAccess);
            return;
        };
        let Some(end) = address
            .checked_add(size.get())
            .filter(|end| *end <= 1 << 32)
        else {
            self.fault(PreservationFault::UnresolvedMapping);
            return;
        };
        let mut cursor = cpu_address.get();
        let mut first = None;
        // Validate the entire write, but record only its first protected chunk.
        // A public write counts once even when it crosses several RAM windows.
        while cursor < end {
            let map = state
                .decoders
                .observed_mapping(self.core.get() as usize, cursor)
                .or_else(|| {
                    self.maps
                        .iter()
                        .find(|map| map.address <= cursor && cursor < map.address + map.size as u64)
                });
            let Some(map) = map else {
                self.fault(PreservationFault::UnresolvedMapping);
                return;
            };
            let chunk_end = end.min(map.address + map.size as u64);
            let typed = (|| {
                Ok::<_, String>((
                    PhysicalAddress::new(map.physical + cursor - map.address)?,
                    CpuAddress::new(cursor)?,
                    WriteSize::new((chunk_end - cursor) as usize)?,
                ))
            })();
            let Ok((physical, logical, chunk_size)) = typed else {
                self.fault(PreservationFault::UnresolvedMapping);
                return;
            };
            if first.is_none() && self.records.borrow().overlaps(physical, chunk_size) {
                first = Some((physical, logical, chunk_size));
            }
            cursor = chunk_end;
        }
        if let Some((physical, logical, chunk_size)) = first {
            self.records.borrow_mut().bus_write(
                PreservationBusSource::HostWrite,
                physical,
                logical,
                chunk_size,
            );
        }
    }
}

unsafe fn write(_: Access, address: u64, size: i32, value: i64, data: *mut c_void) {
    let context = unsafe { &*data.cast::<PreservationContext>() };
    context.store(address, size, value);
}

unsafe fn host_write(address: u64, bytes: usize, data: *mut c_void) {
    let context = unsafe { &*data.cast::<PreservationContext>() };
    context.host_write(address, bytes);
}

impl Board {
    pub(super) fn preservation_active(&self) -> bool {
        self.physical_memory
            .preservation
            .borrow()
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some()
    }
    pub fn preservation_begin(&self, ranges: ProtectedRanges) -> Result<Box<Preservation>, String> {
        if self.cpus.iter().any(|cpu| cpu.model() != Model::CortexR5F) {
            return Err("Preservation observation requires R-profile processors".into());
        }
        self.state
            .try_borrow_mut()
            .map_err(|_| "Preservation observation requires a paused board")?;
        if self.preservation_active() {
            return Err("A preservation observation is already armed".into());
        }
        if self
            .physical_memory
            .observation
            .borrow()
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some()
        {
            return Err("A memory watch conflicts with preservation observation".into());
        }
        for range in ranges.ranges() {
            let mut cursor = range.start();
            while cursor < range.end() {
                let (base, backing) = self
                    .physical_memory
                    .regions()
                    .iter()
                    .find(|(base, backing)| {
                        *base <= cursor && cursor < *base + backing.len() as u64
                    })
                    .ok_or("Preservation range has no complete physical RAM backing")?;
                cursor = range.end().min(*base + backing.len() as u64);
            }
        }
        let merged = ranges
            .ranges()
            .iter()
            .map(|range| (range.start(), range.end()))
            .collect::<Vec<_>>();
        let merged = union(merged)
            .into_iter()
            .map(|(start, end)| PhysicalRange::new(start, end))
            .collect::<Result<Vec<_>, _>>()?;
        let mut views = self.mappings.clone();
        {
            let state = self.state.borrow();
            for core in 0..self.cpus.len() {
                views.extend(state.decoders.observation_maps(core));
            }
        }
        views.sort_by_key(|map| (map.core, map.address, map.physical, map.size));
        views.dedup();
        let mut spans: Vec<Vec<CpuRange>> = vec![Vec::new(); self.cpus.len()];
        for map in &views {
            if map.core >= self.cpus.len() {
                continue;
            }
            for range in &merged {
                let start = range.start().max(map.physical);
                let end = range.end().min(map.physical + map.size as u64);
                if start < end {
                    spans[map.core].push(CpuRange::new(
                        CpuAddress::new(map.address + start - map.physical)?,
                        CpuAddress::new(map.address + end - map.physical)?,
                    )?);
                }
            }
        }
        for (core, core_spans) in spans.iter_mut().enumerate() {
            *core_spans = union(
                core_spans
                    .iter()
                    .map(|span| (span.start(), span.end()))
                    .collect(),
            )
            .into_iter()
            .map(|(start, end)| CpuRange::new(CpuAddress::new(start)?, CpuAddress::new(end)?))
            .collect::<Result<Vec<_>, String>>()?;
            for span in core_spans.iter() {
                if tcm_overlap(&*self.cpus[core], span.start(), span.end() - span.start())
                    .map_err(|error| format!("Preservation TCM inspection: {error}"))?
                {
                    return Err("Preservation range intersects a CPU-private TCM overlay".into());
                }
            }
        }
        let records = Rc::new(RefCell::new(PreservationRecords {
            ranges,
            merged,
            identity_cpu_writes: WriteCount::default(),
            changed_cpu_writes: WriteCount::default(),
            host_writes: WriteCount::default(),
            model_bus_writes: WriteCount::default(),
            violation: None,
        }));
        let mut preservation = Box::new(Preservation {
            memory: Rc::downgrade(&self.physical_memory),
            records: records.clone(),
            contexts: Vec::new(),
            ended: false,
        });
        self.physical_memory
            .preservation
            .replace(Some(Rc::downgrade(&records)));
        for (core, core_spans) in spans.into_iter().enumerate() {
            let ranges = union(
                core_spans
                    .iter()
                    .map(|span| (span.start().saturating_sub(HOOK_REACH), span.end()))
                    .collect(),
            );
            let context_merged = records.borrow().merged.clone();
            preservation.contexts.push(PreservationHooks {
                context: Box::new(PreservationContext {
                    state: Rc::downgrade(&self.state),
                    cpu: Rc::downgrade(&self.cpus[core]),
                    records: records.clone(),
                    maps: self
                        .mappings
                        .iter()
                        .filter(|map| map.core == core)
                        .cloned()
                        .collect(),
                    merged: context_merged,
                    spans: core_spans,
                    core: CoreId::new(core)?,
                }),
                hooks: Vec::new(),
            });
            let entry = preservation.contexts.last_mut().unwrap();
            let data = (&*entry.context as *const PreservationContext)
                .cast_mut()
                .cast();
            match unsafe {
                self.cpus[core].install_hook(Hook::HostWrite {
                    callback: host_write,
                    data,
                    range: HookRange::All,
                })
            } {
                Ok(id) => entry.hooks.push(id),
                Err(error) => {
                    self.preservation_end(&mut preservation)?;
                    return Err(error);
                }
            }
            for (begin, end) in ranges {
                // Ordinary pre-store hooks isolate the instruction before any
                // callback runs. Keep this hook unwatched: a watched callback
                // can run before coherence restarts an uncompleted store.
                match unsafe {
                    self.cpus[core].install_hook(Hook::Memory {
                        callback: write,
                        data,
                        read: false,
                        write: true,
                        range: HookRange::Inclusive {
                            begin,
                            end: end - 1,
                        },
                    })
                } {
                    Ok(id) => entry.hooks.push(id),
                    Err(error) => {
                        self.preservation_end(&mut preservation)?;
                        return Err(error);
                    }
                }
            }
        }
        Ok(preservation)
    }
    pub fn preservation_end(&self, preservation: &mut Preservation) -> Result<(), String> {
        preservation.end()
    }
}

#[cfg(test)]
mod cleanup_tests {
    use super::*;

    unsafe fn end_during_host_write(_: u64, _: usize, data: *mut c_void) {
        let preservation = unsafe { &mut *data.cast::<Preservation>() };
        // The CPU refuses to remove its hooks while dispatch holds the index.
        assert!(preservation.end().is_err());
    }

    fn fail_during_host_write(board: &Board, preservation: &mut Preservation) -> HookId {
        let hook = unsafe {
            board.cpus[0].install_hook(Hook::HostWrite {
                callback: end_during_host_write,
                data: (preservation as *mut Preservation).cast(),
                range: HookRange::All,
            })
        }
        .unwrap();
        board.cpu(0).unwrap().write_memory(0x10080, &[1]).unwrap();
        hook
    }

    #[test]
    fn failed_removal_releases_other_core_contexts_and_can_be_retried() {
        let board = super::super::preservation_tests::two_core_board();
        let ranges =
            ProtectedRanges::new(vec![PhysicalRange::new(0x10000, 0x12000).unwrap()]).unwrap();
        let mut preservation = board.preservation_begin(ranges.clone()).unwrap();
        let records = Rc::downgrade(&preservation.records);
        assert_eq!(records.strong_count(), 3);
        let hook = fail_during_host_write(&board, &mut preservation);
        assert_eq!(
            records.strong_count(),
            2,
            "the successfully detached core must release its context"
        );
        assert!(matches!(
            preservation.state().violation(),
            Some(PreservationViolation::IntegrityFault {
                reason: PreservationFault::UnsupportedAccess
            })
        ));
        assert!(board.preservation_begin(ranges.clone()).is_err());
        board.cpus[0].remove_hook(hook).unwrap();
        preservation.end().unwrap();
        assert!(!preservation.state().enabled());
        drop(preservation);
        assert_eq!(records.strong_count(), 0);
        board.preservation_begin(ranges).unwrap().end().unwrap();
    }

    #[test]
    fn failed_cleanup_releases_context_when_its_cpu_is_gone() {
        let board = super::super::preservation_tests::two_core_board();
        let ranges =
            ProtectedRanges::new(vec![PhysicalRange::new(0x10000, 0x12000).unwrap()]).unwrap();
        let mut preservation = board.preservation_begin(ranges).unwrap();
        let records = Rc::downgrade(&preservation.records);
        fail_during_host_write(&board, &mut preservation);
        drop(board);
        preservation.end().unwrap();
        assert_eq!(records.strong_count(), 1);
        drop(preservation);
        assert_eq!(records.strong_count(), 0);
    }
}
