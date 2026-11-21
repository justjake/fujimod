//! Passive physical-memory write watch (`--watch-writes`). Guest CPU stores, model-bus (DMA)
//! writes and host writes that overlap a watched range are recorded as notes for the host to
//! print. No execution or device decision consumes them, and they are outside board state,
//! keyframes and snapshots. With no range set, the store paths only test an empty slot.
use super::{Board, State};
use fujimod_emulator_cpu::{Architecture, Cpu, Hook, HookRange, Register, arm};
use std::ffi::c_void;

/// Notes kept between two `take_write_notes` calls; later ones are counted as dropped.
const NOTE_LIMIT: usize = 100_000;
/// Bytes of a value or prior value kept per note.
const VALUE_BYTES: usize = 16;

/// What performed a watched write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteSource {
    /// A store instruction executed by this processor.
    Core(usize),
    /// A device model's bus write (DMA).
    Dma,
    /// A host write (pokes, loaders, patches).
    Host,
}

/// One write that overlapped a watched range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteNote {
    pub source: WriteSource,
    /// The store instruction's address and link register (CPU stores only).
    pub pc: Option<u64>,
    pub lr: Option<u64>,
    /// Physical address and size of the store, or for a DMA/host write the part of it that
    /// overlaps the watched ranges.
    pub address: u64,
    pub size: u64,
    /// The whole DMA/host write as (physical address, bytes) when it is larger than the overlap.
    pub span: Option<(u64, u64)>,
    /// The bytes written at `address` (at most 16; empty when the store value is unavailable).
    pub value: Vec<u8>,
    /// The bytes at `address` before the write (at most 16; empty when unreadable).
    pub old: Vec<u8>,
    /// Guest time in board ticks.
    pub tick: u64,
}

fn hex_value(bytes: &[u8], size: u64) -> String {
    if bytes.is_empty() {
        return "?".into();
    }
    if matches!(size, 1 | 2 | 4 | 8) && bytes.len() as u64 == size {
        let mut word = [0u8; 8];
        word[..bytes.len()].copy_from_slice(bytes);
        return format!("{:#0width$x}", u64::from_le_bytes(word), width = 2 + 2 * bytes.len());
    }
    let mut text = bytes.iter().map(|byte| format!("{byte:02x}")).collect::<Vec<_>>().join(" ");
    if (bytes.len() as u64) < size {
        text += " ...";
    }
    format!("[{text}]")
}

impl WriteNote {
    /// One log line without a prefix, with guest time in seconds at `ticks_per_second`:
    /// `core 1 pc 0x... lr 0x... addr 0x257ccd9 size 1 value 0x0c old 0x00 at 62.021300 s (tick N)`.
    pub fn line(&self, ticks_per_second: u64) -> String {
        let mut line = match self.source {
            WriteSource::Core(core) => format!("core {core}"),
            WriteSource::Dma => "DMA".into(),
            WriteSource::Host => "host".into(),
        };
        if let Some(pc) = self.pc {
            line += &format!(" pc {pc:#x}");
        }
        if let Some(lr) = self.lr {
            line += &format!(" lr {lr:#x}");
        }
        line += &format!(" addr {:#x} size {}", self.address, self.size);
        if let Some((address, size)) = self.span {
            line += &format!(" (of write {address:#x} size {size})");
        }
        line += &format!(" value {} old {}", hex_value(&self.value, self.size), hex_value(&self.old, self.size));
        let seconds = self.tick as f64 / ticks_per_second.max(1) as f64;
        line += &format!(" at {seconds:.6} s (tick {})", self.tick);
        line
    }
}

/// Sorted, merged, half-open watched ranges and the notes recorded since the last take.
#[derive(Default)]
pub(super) struct Watch {
    ranges: Vec<(u64, u64)>,
    pub(super) notes: Vec<WriteNote>,
    dropped: u64,
}

impl Watch {
    /// Ranges as (address, length); empty or overflowing ones are refused.
    pub(super) fn new(ranges: &[(u64, u64)]) -> Result<Self, String> {
        let mut spans = ranges
            .iter()
            .map(|&(address, length)| match address.checked_add(length) {
                Some(end) if length > 0 => Ok((address, end)),
                _ => Err(format!("write watch range {address:#x}:{length:#x} is empty or overflows")),
            })
            .collect::<Result<Vec<_>, String>>()?;
        spans.sort_unstable();
        let mut merged: Vec<(u64, u64)> = vec![];
        for (begin, end) in spans {
            match merged.last_mut() {
                Some(last) if begin <= last.1 => last.1 = last.1.max(end),
                _ => merged.push((begin, end)),
            }
        }
        Ok(Self { ranges: merged, ..Self::default() })
    }
    /// The part of `[address, address + size)` that the watched ranges cover, from the first
    /// covered byte to the last.
    pub(super) fn overlap(&self, address: u64, size: u64) -> Option<(u64, u64)> {
        let end = address.saturating_add(size);
        let first = self.ranges.partition_point(|range| range.1 <= address);
        let covered = self.ranges[first..].iter().take_while(|range| range.0 < end);
        let (mut low, mut high) = (u64::MAX, 0);
        for &(begin, finish) in covered {
            low = low.min(begin.max(address));
            high = high.max(finish.min(end));
        }
        (low < high).then_some((low, high))
    }
    fn push(&mut self, note: WriteNote) {
        if self.notes.len() < NOTE_LIMIT {
            self.notes.push(note);
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }
    fn take(&mut self) -> (Vec<WriteNote>, u64) {
        (std::mem::take(&mut self.notes), std::mem::take(&mut self.dropped))
    }
}

impl super::memory::Memory {
    /// Record the part of a host or DMA write at bus address `address` that overlaps a watched
    /// range. Call before the write, so the prior bytes are still in place.
    pub(super) fn watch_transfer(&self, source: WriteSource, address: u64, bytes: &[u8], tick: u64) {
        let mut slot = self.write_watch.borrow_mut();
        let Some(watch) = slot.as_mut() else { return };
        let mut offset = 0usize;
        for (physical, len) in self.physical_spans(address, bytes.len()) {
            if let Some((low, high)) = watch.overlap(physical, len as u64) {
                let start = offset + (low - physical) as usize;
                let kept = ((high - low) as usize).min(VALUE_BYTES);
                let mut old = vec![0; kept];
                let old = match self.read_backing(low, &mut old) {
                    Ok(()) => old,
                    Err(_) => vec![],
                };
                watch.push(WriteNote {
                    source,
                    pc: None,
                    lr: None,
                    address: low,
                    size: high - low,
                    span: ((low, high) != (physical, physical + len as u64)).then_some((physical, len as u64)),
                    value: bytes[start..start + kept].to_vec(),
                    old,
                    tick,
                });
            }
            offset += len;
        }
    }
    pub(super) fn watching_writes(&self) -> bool {
        self.write_watch.borrow().is_some()
    }
}

/// The guest time at this core's current instruction, as for a watched PC entry.
pub(super) fn core_tick(state: &State, cpu: &dyn Cpu, core: usize) -> u64 {
    let extra = cpu
        .instruction_progress()
        .unwrap_or(state.accounted[core])
        .saturating_sub(state.accounted[core]);
    let elapsed = state
        .timeline
        .ratio()
        .advance_phase(state.timeline.phase(), extra)
        .map(|v| v.0)
        .unwrap_or(0)
        .min(u64::MAX as u128) as u64;
    state.timeline.now().saturating_add(elapsed)
}

/// Record a store by `core` at processor address `physical` (already translated) if it
/// overlaps a watched range. The memory hook runs before the store lands, so the backing still
/// holds the prior bytes and `value` is the stored value.
pub(super) fn observe_store(state: &State, cpu: &dyn Cpu, core: usize, physical: u64, size: usize, value: i64) {
    let physical = match &state.mprofile[core] {
        // M-profile aliases resolve to the shared RAM; peripheral stores are not RAM writes.
        Some(profile) => match profile.physical(physical, size) {
            Ok(address) => address,
            Err(_) => return,
        },
        None => physical,
    };
    let mut slot = state.memory.write_watch.borrow_mut();
    let Some(watch) = slot.as_mut() else { return };
    if watch.overlap(physical, size as u64).is_none() {
        return;
    }
    let kept = size.min(VALUE_BYTES);
    let value = if size <= 8 { value.to_le_bytes()[..size].to_vec() } else { vec![] };
    let mut old = vec![0; kept];
    let old = match state.memory.read_backing(physical, &mut old) {
        Ok(()) => old,
        Err(_) => vec![],
    };
    let (pc, lr) = if cpu.model().architecture() == Architecture::Arm64 {
        (Register::Pc, Register::X(30))
    } else {
        (Register::Pc, Register::Arm32(arm::Register::R(14)))
    };
    watch.push(WriteNote {
        source: WriteSource::Core(core),
        pc: super::scalar(cpu, pc).ok(),
        lr: super::scalar(cpu, lr).ok(),
        address: physical,
        size: size as u64,
        span: None,
        value,
        old,
        tick: core_tick(state, cpu, core),
    });
}

/// Store hook for processors that have no board store hook: it only feeds the write watch.
unsafe fn watched_store(
    _: fujimod_emulator_cpu::Access,
    address: u64,
    size: i32,
    value: i64,
    data: *mut c_void,
) {
    let binding = unsafe { &*data.cast::<super::Binding>() };
    let Some(cpus) = binding.cpus.upgrade() else { return };
    let Ok(size) = usize::try_from(size) else { return };
    let cpu = &*cpus[binding.core];
    let Ok(physical) = cpu.translate(address, fujimod_emulator_cpu::Access::Write) else { return };
    let Ok(state) = binding.state.try_borrow() else { return };
    observe_store(&state, cpu, binding.core, physical, size, value);
}

impl Board {
    /// Watch guest writes to physical `ranges` (address, length): every processor store, DMA
    /// write and host write that overlaps one is recorded for `take_write_notes`. A second call
    /// replaces the ranges. Processor stores need a store hook, which the hypervisor backend
    /// does not have, so a board with such a processor refuses.
    pub fn watch_writes(&mut self, ranges: &[(u64, u64)]) -> Result<(), String> {
        if ranges.is_empty() {
            return Ok(());
        }
        let watch = Watch::new(ranges)?;
        if self.cpus.iter().any(|cpu| cpu.features().shared_physical_maps) {
            return Err("write watches need processor store hooks, which the hypervisor engine lacks; \
                        run with --engine unicorn"
                .into());
        }
        if !self.physical_memory.watching_writes() {
            // The board store hook (`written`) serves every core when an M-profile core is
            // present (installed on demand while a watch is armed on cores that otherwise do not
            // need it); otherwise each core gets a watch-only store hook.
            let board_hooked = self
                .cpus
                .iter()
                .any(|cpu| cpu.model() == fujimod_emulator_cpu::Model::CortexM4);
            if !board_hooked {
                for core in 0..self.cpus.len() {
                    let data = (&*self.bindings[core] as *const super::Binding).cast_mut().cast();
                    // The binding box stays at this address until Board drops its CPU hooks.
                    let hook = unsafe {
                        self.cpus[core].install_hook(Hook::Memory {
                            callback: watched_store,
                            data,
                            read: false,
                            write: true,
                            range: HookRange::All,
                        })?
                    };
                    self.hooks.push((core, hook));
                }
            }
        }
        *self.physical_memory.write_watch.borrow_mut() = Some(watch);
        Ok(())
    }
    /// Take the watched writes recorded so far and the count of notes dropped over the bound.
    pub fn take_write_notes(&self) -> (Vec<WriteNote>, u64) {
        self.physical_memory
            .write_watch
            .borrow_mut()
            .as_mut()
            .map(Watch::take)
            .unwrap_or_default()
    }
}
