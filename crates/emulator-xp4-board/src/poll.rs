//! Exact fast-forward of stable status polling in counted runs.
//!
//! A processor that spins on a device word that cannot change before the
//! next device deadline executes the same instructions over and over. When a
//! run proves that, the board charges the spun instructions to the processor
//! (guest time, per-core counts, trace) and leaves the architectural state
//! alone: the state after any whole number of iterations is the state now.
//!
//! The proof is a fixed point of one iteration, taken between two reads of
//! the same stable device word (`Device::read_stable`) that return the same
//! value:
//! - the registers, status flags, FP state and translation context recur;
//! - the pages the iteration touched (the engine records every page a
//!   translation refill reaches, a superset of what is read or written) hold
//!   the same bytes before and after it;
//! - no label or return observer ran in it.
//!
//! The fixed point stays valid for later slices while the registers, those
//! pages, the polled value and the translation context are unchanged and no
//! translation maintenance was accepted; the iterations it covers stop at the
//! slice end or one instruction before the board next has device work, so
//! nothing a device does is skipped.
use super::*;
use fujimod_emulator_cpu::BackingObserver;
use std::sync::atomic::AtomicU64;

/// A read of a stable device word: where, what and how it was read.
pub(super) type Read = (u32, u64, u32, u64);
const MAX_PERIOD: u64 = 4096;
const MAX_PAGES: usize = 16;
const PAGE_SHIFT: u32 = 12;
const MAX_BACKINGS: usize = 32;

/// Poll-loop state of one core.
#[derive(Default)]
pub(super) struct Watch {
    /// The stable read that ended the current run, if one did.
    read: Option<Read>,
    stage: Stage,
    fixed: Option<Fixed>,
    /// Stable reads left to ignore after a failed proof.
    cooldown: u32,
    failures: u32,
    bitmaps: Vec<Box<[AtomicU64]>>,
    /// Per bitmap, one bit per word that holds a set bit.
    summaries: Vec<Box<[AtomicU64]>>,
    armed: Vec<(NonNull<u8>, usize)>,
}
#[derive(Default)]
enum Stage {
    #[default]
    Idle,
    /// First read seen; the pages of the iteration after it are recorded.
    Armed(Point),
    /// Second read matched; the pages of the second iteration are recorded.
    Witness(Point, u64, Pages),
}
/// The state at the end of a run that ended at a stable read.
struct Point {
    read: Read,
    registers: Vec<u8>,
    at: u64,
    observations: u64,
}
/// Bytes of the touched pages, by host address.
#[derive(Clone)]
struct Pages {
    list: Vec<(NonNull<u8>, usize)>,
    bytes: Vec<u8>,
}
struct Fixed {
    read: Read,
    registers: Vec<u8>,
    period: u64,
    epoch: u64,
    pages: Pages,
}

impl Pages {
    fn matches(&self) -> bool {
        let mut at = 0;
        for &(page, len) in &self.list {
            let now = unsafe { std::slice::from_raw_parts(page.as_ptr(), len) };
            if now != &self.bytes[at..at + len] {
                return false;
            }
            at += len;
        }
        true
    }
}

impl Cluster {
    /// Whether this session lets a run be shortened by a poll proof.
    fn poll_permitted(&self) -> bool {
        let state = unsafe { &*self.state.get() };
        !state.poll_off && self.plain_counted()
    }
    /// Whether a counted run has no recorder, guard, watch or observer that
    /// needs to see each instruction the board performs.
    pub(super) fn plain_counted(&self) -> bool {
        let state = unsafe { &*self.state.get() };
        self.counted
            && !state.discovery.enabled
            && state.driver_guard.is_none()
            && state.recorder.is_none()
            && state.memory_watch.is_none()
            && state.pc_trace.is_none()
            && state.callee_stubs.is_empty()
            && state.pc_observers.iter().all(|set| set.is_empty())
    }
    /// A device read inside a counted run: when the model calls it stable,
    /// ends the run after this instruction so the board can look at the loop.
    pub(super) fn poll_note(&self, core: usize, pc: u32, address: u64, size: u32, value: u64) {
        let state = unsafe { &mut *self.state.get() };
        if !state.devices.as_ref().is_some_and(|d| d.last_read_stable()) || !self.poll_permitted() {
            return;
        }
        let watch = &mut state.poll[core];
        if watch.cooldown > 0 {
            watch.cooldown -= 1;
            return;
        }
        watch.read = Some((pc, address, size, value));
    }
    /// Whether a stable read asks to end the run.
    pub(super) fn poll_stop(&self, core: usize) -> bool {
        unsafe { (&*self.state.get()).poll[core].read.is_some() }
    }
    /// A run ended for any reason: forget a pending proof.
    pub(super) fn poll_abandon(&self, core: usize) {
        let state = unsafe { &mut *self.state.get() };
        let watch = &mut state.poll[core];
        watch.read = None;
        if !matches!(watch.stage, Stage::Idle) {
            watch.stage = Stage::Idle;
            let _ = self.poll_disarm(core);
        }
    }
    fn poll_fail(&self, core: usize) {
        self.poll_abandon(core);
        let watch = &mut unsafe { &mut *self.state.get() }.poll[core];
        watch.failures = (watch.failures + 1).min(10);
        watch.cooldown = (1 << watch.failures) - 1;
    }
    fn poll_disarm(&self, core: usize) -> Result<(), String> {
        unsafe { self.cpus[core].observe_backings(&[]) }
    }
    fn poll_arm(&self, core: usize) -> Result<(), String> {
        let backings = self.cpus[core].checkpoint_backings()?;
        if backings.len() > MAX_BACKINGS {
            return Err("too many backings to watch".into());
        }
        let watch = &mut unsafe { &mut *self.state.get() }.poll[core];
        watch.bitmaps.resize_with(backings.len(), Default::default);
        watch
            .summaries
            .resize_with(backings.len(), Default::default);
        let mut ranges = Vec::with_capacity(backings.len());
        for (index, &(base, bytes)) in backings.iter().enumerate() {
            let words = (bytes >> PAGE_SHIFT).div_ceil(64) + 1;
            let map = &mut watch.bitmaps[index];
            let summary = &mut watch.summaries[index];
            if map.len() != words {
                *map = (0..words).map(|_| AtomicU64::new(0)).collect();
                *summary = (0..words.div_ceil(64) + 1)
                    .map(|_| AtomicU64::new(0))
                    .collect();
            } else {
                // Only the words the summary marks can hold set bits.
                for (group, marks) in summary.iter().enumerate() {
                    let mut marks = marks.swap(0, std::sync::atomic::Ordering::Relaxed);
                    while marks != 0 {
                        let word = group * 64 + marks.trailing_zeros() as usize;
                        marks &= marks - 1;
                        map[word].store(0, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
            ranges.push(BackingObserver {
                backing: base,
                bytes,
                bitmap: NonNull::new(map.as_ptr().cast_mut()).unwrap(),
                page_shift: PAGE_SHIFT,
                summary: NonNull::new(summary.as_ptr().cast_mut()),
            });
        }
        watch.armed = backings;
        unsafe { self.cpus[core].observe_backings(&ranges) }
    }
    /// Whether the board owns the backing for as long as it lives (the pages
    /// of other backings, such as flash read pages, can be released).
    fn poll_owned(&self, base: NonNull<u8>, bytes: usize) -> bool {
        let inside = |start: *const u8, len: usize| {
            let (begin, end) = (start as usize, start as usize + len);
            (begin..end).contains(&(base.as_ptr() as usize))
                && base.as_ptr() as usize + bytes <= end
        };
        inside(self.ram.as_ptr(), self.ram.len())
            || self
                .memory
                .iter()
                .any(|window| inside(window.data.as_ptr(), window.data.len()))
    }
    /// The pages recorded since arming, with their bytes; `None` for too many
    /// or for pages of a backing the board does not own.
    fn poll_pages(&self, core: usize) -> Option<Pages> {
        let watch = &unsafe { &*self.state.get() }.poll[core];
        let mut pages = Pages {
            list: Vec::new(),
            bytes: Vec::new(),
        };
        for ((map, summary), &(base, bytes)) in
            watch.bitmaps.iter().zip(&watch.summaries).zip(&watch.armed)
        {
            // Words the summary marks, in ascending order.
            let marked = summary.iter().enumerate().flat_map(|(group, marks)| {
                let mut marks = marks.load(std::sync::atomic::Ordering::Relaxed);
                std::iter::from_fn(move || {
                    (marks != 0).then(|| {
                        let word = group * 64 + marks.trailing_zeros() as usize;
                        marks &= marks - 1;
                        word
                    })
                })
            });
            for word in marked {
                let mut bits = map[word].load(std::sync::atomic::Ordering::Relaxed);
                while bits != 0 {
                    let page = word * 64 + bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    let start = page << PAGE_SHIFT;
                    if start >= bytes {
                        continue;
                    }
                    let len = (bytes - start).min(1 << PAGE_SHIFT);
                    if pages.list.len() == MAX_PAGES || !self.poll_owned(base, bytes) {
                        return None;
                    }
                    let host = unsafe { NonNull::new_unchecked(base.as_ptr().add(start)) };
                    pages.bytes.extend_from_slice(unsafe {
                        std::slice::from_raw_parts(host.as_ptr(), len)
                    });
                    pages.list.push((host, len));
                }
            }
        }
        Some(pages)
    }
    /// Registers, flags, FP state and translation context.
    fn poll_registers(&self, core: usize) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(64 + 8 * 40);
        for register in arm::CORE {
            out.extend_from_slice(&self.reg(core, register)?.to_le_bytes());
        }
        for register in [arm::CPSR, arm::SPSR] {
            out.extend_from_slice(&self.reg(core, register).unwrap_or(0).to_le_bytes());
        }
        for (crn, crm, opc1, opc2) in [
            (1, 0, 0, 0),
            (2, 0, 0, 0),
            (2, 0, 0, 1),
            (2, 0, 0, 2),
            (3, 0, 0, 0),
            (13, 0, 0, 1),
        ] {
            out.extend_from_slice(&self.cp(core, crn, crm, opc1, opc2)?.to_le_bytes());
        }
        if self.reg(core, FPEXC).unwrap_or(0) & 0x4000_0000 != 0 {
            out.extend_from_slice(
                &self
                    .reg(core, Register::Arm32(arm::Register::Fpscr))
                    .unwrap_or(0)
                    .to_le_bytes(),
            );
            for index in 0..32 {
                let mut bytes = [0u8; 8];
                if self.cpus[core]
                    .read_register(Register::Arm32(arm::Register::D(index)), &mut bytes)
                    .is_err()
                {
                    break;
                }
                out.extend_from_slice(&bytes);
            }
        }
        Ok(out)
    }
    /// A run ended. Charges the whole polling iterations it can skip and
    /// moves `remaining` (the slice) and `window` (the scheduler window the
    /// slice belongs to, 0 when it is not this core's alone) past them.
    pub(super) fn poll_run_end(
        &self,
        core: usize,
        remaining: &mut u64,
        window: &mut u64,
        stopped: bool,
    ) -> Result<(), String> {
        let state = unsafe { &mut *self.state.get() };
        let Some(read) = state.poll[core].read.take() else {
            if !matches!(state.poll[core].stage, Stage::Idle) {
                self.poll_abandon(core);
            }
            return Ok(());
        };
        if !stopped
            || state.redirected.is_some()
            || state.failure.is_some()
            || !self.poll_permitted()
        {
            self.poll_abandon(core);
            return Ok(());
        }
        let registers = match self.poll_registers(core) {
            Ok(registers) => registers,
            Err(_) => {
                self.poll_fail(core);
                return Ok(());
            }
        };
        let at = state.per_core[core];
        let observations = state.observations;
        let epoch = state.tlbi_epoch;
        let watch = &mut state.poll[core];
        if let Some(fixed) = &watch.fixed {
            if fixed.read == read
                && fixed.registers == registers
                && fixed.epoch == epoch
                && fixed.pages.matches()
            {
                let period = fixed.period;
                self.poll_abandon(core);
                return self.poll_skip(core, period, remaining, window);
            }
            watch.fixed = None;
        }
        let point = Point {
            read,
            registers,
            at,
            observations,
        };
        match std::mem::take(&mut watch.stage) {
            Stage::Idle => match self.poll_arm(core) {
                Ok(()) => unsafe { &mut *self.state.get() }.poll[core].stage = Stage::Armed(point),
                Err(_) => self.poll_fail(core),
            },
            Stage::Armed(before) => {
                let period = at.wrapping_sub(before.at);
                let recurs = before.read == point.read
                    && before.registers == point.registers
                    && before.observations == point.observations
                    && (1..=MAX_PERIOD).contains(&period);
                let Some(pages) = recurs.then(|| self.poll_pages(core)).flatten() else {
                    self.poll_fail(core);
                    return Ok(());
                };
                match self.poll_arm(core) {
                    Ok(()) => {
                        unsafe { &mut *self.state.get() }.poll[core].stage =
                            Stage::Witness(point, period, pages)
                    }
                    Err(_) => self.poll_fail(core),
                }
            }
            Stage::Witness(before, period, pages) => {
                let recurs = before.read == point.read
                    && before.registers == point.registers
                    && before.observations == point.observations
                    && at.wrapping_sub(before.at) == period;
                let again = recurs.then(|| self.poll_pages(core)).flatten();
                let proven = again.is_some_and(|again| {
                    again.list.iter().all(|page| pages.list.contains(page)) && pages.matches()
                });
                if !proven {
                    self.poll_fail(core);
                    return Ok(());
                }
                let watch = &mut unsafe { &mut *self.state.get() }.poll[core];
                watch.failures = 0;
                watch.fixed = Some(Fixed {
                    read: point.read,
                    registers: point.registers,
                    period,
                    epoch,
                    pages,
                });
                self.poll_abandon(core);
                return self.poll_skip(core, period, remaining, window);
            }
        }
        Ok(())
    }
    /// Whether no other processor can run, and nothing the scheduler does
    /// between slices has work, until the board next has device work: every
    /// other core waits without a pending interrupt, no code-coherence or
    /// remap work is queued, and the execute-in-place source is unchanged.
    pub(super) fn poll_alone(&self, core: usize) -> bool {
        let state = unsafe { &*self.state.get() };
        (0..self.cpus.len()).all(|peer| {
            peer == core || (state.waiting[peer] != 0 && state.gic.next_irq(peer).is_none())
        }) && !state.cache_dirty
            && !(state.remap_ready && state.low_alias.is_some())
            && state.xip_source.as_ref().is_none_or(|source| {
                source.load(std::sync::atomic::Ordering::SeqCst) == state.xip_seen
            })
    }
    /// Charges whole iterations that end before the board has device work:
    /// within the slice, or, when this core runs alone in its scheduler
    /// window, across slice ends up to the window's end. The slices it
    /// crosses would each end with nothing to do, and the next begins with
    /// the same registers, so they run as one.
    fn poll_skip(
        &self,
        core: usize,
        period: u64,
        remaining: &mut u64,
        window: &mut u64,
    ) -> Result<(), String> {
        let room = self.room()?.saturating_sub(1);
        let across = *window > *remaining && self.poll_alone(core);
        let skipped = if across {
            (*window).min(room)
        } else {
            (*remaining).min(room)
        } / period
            * period;
        if skipped == 0 {
            return Ok(());
        }
        self.advance(core, skipped)?;
        unsafe { (*self.state.get()).poll_skipped += skipped };
        // The trace continues the loop.
        let state = unsafe { &mut *self.state.get() };
        let trace = &mut state.recent[core];
        let period = period as usize;
        if trace.len() >= period {
            let cycle: Vec<_> = trace.iter().rev().take(period).rev().copied().collect();
            // Entry `j` after the run is `cycle[j % period]`; the newest 32 remain.
            let first = (skipped as usize).saturating_sub(32);
            for index in first..skipped as usize {
                if trace.len() == 32 {
                    trace.pop_front();
                }
                trace.push_back(cycle[index % period]);
            }
        }
        // Slice ends inside the skip fall at multiples of the quantum from the
        // slice's end; the slice it stops in has the rest of its quantum.
        let overflow = skipped.saturating_sub(*remaining);
        *remaining = if overflow == 0 {
            *remaining - skipped
        } else {
            let quantum = state.quantum.max(1);
            match overflow % quantum {
                0 => 0,
                past => (quantum - past).min(window.saturating_sub(skipped)),
            }
        };
        *window = window.saturating_sub(skipped);
        Ok(())
    }
}
