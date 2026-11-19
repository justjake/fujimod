//! Opt-in counted ARMv7-A execution (`FUJIMOD_XP4_COUNTED=1`): the execution
//! law of the per-instruction hook, run in native translated blocks.
//!
//! Before every instruction the hook runs `due()` (device deadlines, interval
//! timers, wake-ups), the code-coherence redirect and IRQ entry; it then
//! counts the instruction (guest time), records it in the per-core trace,
//! observes labels and call returns and performs the board's own
//! instructions (exclusives, CLREX, identity reads, WFI/WFE/SEV). Counted
//! execution keeps each of those steps at the same instruction boundary:
//! - a slice is cut into internal runs that end exactly where `due()` has
//!   work (`room`), so deadlines fire before the same instruction;
//! - a device access brings guest time up to its instruction and ends the
//!   run after it whenever `due()`, the redirect or an IRQ would act before
//!   the next one;
//! - an asserted IRQ stops the processor before the next block once it is
//!   unmasked, and the board enters it at that boundary, as the hook does;
//! - board instructions exit before entry and run through `special`;
//! - aborts are settled by the engine and entered by the board's own
//!   exception path;
//! - labels and call returns are single-address observers;
//! - the per-core trace comes from the engine's exact entry accounting;
//! - accepted shared TLB maintenance is settled after every internal run,
//!   before the board touches a processor again.
//!
//! Internal run boundaries are invisible to the round-robin scheduler, which
//! still sees each slice end where the per-instruction hook ends it.
//! Definitions and observers that need the per-instruction hook (the boot
//! ROM's Thumb code and low alias, the PMU profile, frozen NOR, the driver
//! guard, PC observers, memory and function watches, run-to) keep it: the
//! session reports why and runs exactly, or refuses the observer.
use super::*;
use fujimod_emulator_cpu::Interrupt;
use fujimod_emulator_cpu_unicorn::aprofile;

/// One processor: the raw engine with the per-instruction hook, or the
/// counted engine around it. Board code addresses both as a `Cpu`; the
/// translation-maintenance owner addresses the raw engine. One per core,
/// built once and never moved, so the exact variant stays inline (no extra
/// indirection on the per-instruction path).
#[allow(clippy::large_enum_variant)]
pub(super) enum Processor {
    Exact(Core),
    Counted(aprofile::Core),
}
impl std::ops::Deref for Processor {
    type Target = dyn Cpu;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Exact(core) => core,
            Self::Counted(core) => core,
        }
    }
}
impl std::borrow::Borrow<Core> for Processor {
    fn borrow(&self) -> &Core {
        match self {
            Self::Exact(core) => core,
            Self::Counted(core) => core.raw(),
        }
    }
}
impl Processor {
    pub(super) fn counted(&self) -> Option<&aprofile::Core> {
        match self {
            Self::Exact(_) => None,
            Self::Counted(core) => Some(core),
        }
    }
}

pub(super) fn requested() -> bool {
    std::env::var_os("FUJIMOD_XP4_COUNTED").is_some_and(|v| v != "0")
}

/// What keeps a definition on the per-instruction hook, if anything.
pub(super) fn unsupported(boot_rom: bool, pmu: bool, frozen_nor: bool) -> Option<&'static str> {
    [
        (boot_rom, "the boot ROM (Thumb code and low alias)"),
        (pmu, "the PMU profile"),
        (frozen_nor, "a frozen NOR prefix"),
    ]
    .into_iter()
    .find_map(|(present, name)| present.then_some(name))
}

/// The error of an observer that needs every instruction.
pub(super) const NEEDS_EXACT: &str =
    "This needs the per-instruction hook: run without FUJIMOD_XP4_COUNTED";

/// The 32-bit physical space outside the given backings, as inclusive
/// ranges: where a counted access can only reach a device or fault.
pub(super) fn device_space(backings: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let mut taken: Vec<_> = backings.iter().filter(|(b, e)| b < e).copied().collect();
    taken.sort_unstable();
    let mut free = Vec::new();
    let mut at = 0;
    for (begin, end) in taken {
        if begin > at {
            free.push((at, begin - 1));
        }
        at = at.max(end);
    }
    if at < 1 << 32 {
        free.push((at, (1 << 32) - 1));
    }
    free
}

/// Marks device space for the engine. Unicorn runs an access that hits an
/// unwatched memory hook as a block of its own and ends translations at it,
/// before the access and before a lazily mapped page is mapped. So the run
/// stops right after a device access (or a guest fault), as the
/// per-instruction hook does.
pub(super) unsafe fn device_marker(_: Access, _: u64, _: i32, _: i64, _: *mut c_void) {}

/// ARM-state LDREX/STREX family and CLREX (owner exclusives).
fn exclusive_word(word: u32) -> bool {
    word & 0x0f90_0fff == 0x0190_0f9f || word & 0x0f90_0ff0 == 0x0180_0f90 || word == 0xf57f_f01f
}

impl Cluster {
    /// Accounts `n` entered instructions of `core` exactly as `n` single
    /// steps of the per-instruction hook do.
    fn advance(&self, core: usize, n: u64) -> Result<(), String> {
        if n == 0 {
            return Ok(());
        }
        let state = unsafe { &mut *self.state.get() };
        state.instructions += n;
        let (ticks, phase) = self.instruction_rate.advance_phase(state.tick_phase, n)?;
        state.ticks = state
            .ticks
            .checked_add(u64::try_from(ticks).map_err(|_| "A7 guest time overflow")?)
            .ok_or("A7 guest time overflow")?;
        state.tick_phase = phase;
        state.per_core[core] += n;
        state.waiting[core] = 0;
        Ok(())
    }
    /// Instructions `core` may still enter before `due()` has work: the one
    /// whose accounting reaches the device scan tick or the next interval
    /// expiry ends the run, so `due()` runs before the following instruction.
    fn room(&self) -> Result<u64, String> {
        let state = unsafe { &*self.state.get() };
        let now = state.ticks;
        let device = (state.devices.is_some() && state.device_scan_at != u64::MAX)
            .then_some(state.device_scan_at);
        let timer = state.timer.as_ref().and_then(|t| t.next_event_tick(now));
        let Some(at) = device.into_iter().chain(timer).min() else {
            return Ok(u64::MAX);
        };
        self.instruction_rate.budget(now, state.tick_phase, at)
    }
    /// Brings guest time up to the running processor's exact entry count
    /// (the current instruction included).
    fn sync_counted(&self, core: usize) -> Result<(), String> {
        let progress = self.cpus[core]
            .instruction_progress()
            .ok_or("counted XP4 run without an entry count")?;
        self.sync_to(core, progress)
    }
    fn sync_to(&self, core: usize, progress: u64) -> Result<(), String> {
        let state = unsafe { &mut *self.state.get() };
        let delta = progress
            .checked_sub(state.accounted)
            .ok_or("counted XP4 entry count moved backward")?;
        state.accounted = progress;
        self.advance(core, delta)
    }
    /// The counted core while a counted run is inside the engine.
    pub(super) fn counted_core(&self, core: usize) -> bool {
        (unsafe { (*self.state.get()).counted_core }) == Some(core)
    }
    /// Before a device access inside a counted run: guest time as the hook
    /// sees it at this instruction.
    pub(super) fn before_access(&self, core: usize) -> Result<(), String> {
        self.sync_counted(core)
    }
    /// After a device access inside a counted run: end the run after this
    /// instruction whenever the hook would act before the next one.
    pub(super) fn after_access(&self, core: usize) -> Result<(), String> {
        let state = unsafe { &*self.state.get() };
        let pending = state.gic.next_irq(core).is_some();
        let wakes = (0..self.cpus.len())
            .any(|c| c != core && state.waiting[c] != 0 && state.gic.next_irq(c).is_some());
        let stop = state.failure.is_some()
            || state.cache_dirty
            || wakes
            || (pending && self.reg(core, arm::CPSR)? & 0x80 == 0);
        let cpu = &self.cpus[core];
        cpu.set_interrupt(Interrupt::Irq, pending)?;
        let progress = state.accounted;
        let room = if stop { 0 } else { self.room()? };
        cpu.tighten_instruction_limit(progress.saturating_add(room))
    }
    /// Installs a single-address counted observer at each address `core`
    /// does not observe yet (the processor must be stopped).
    pub(super) fn observe_addresses(&self, core: usize, addresses: &[u32]) -> Result<(), String> {
        let data = (&*self.hooks[core] as *const Hook).cast_mut().cast();
        for &address in addresses {
            let state = unsafe { &mut *self.state.get() };
            if !state.observed[core].insert(address) {
                continue;
            }
            unsafe {
                self.cpus[core].install_hook(CpuHook::Instruction {
                    callback: counted_observer,
                    data,
                    range: HookRange::Inclusive {
                        begin: u64::from(address),
                        end: u64::from(address),
                    },
                })
            }?;
        }
        Ok(())
    }
    /// What the hook does before entering the next instruction. `true` ends
    /// the slice (redirect or IRQ entry).
    fn boundary(&self, core: usize) -> Result<bool, String> {
        self.due();
        self.healthy()?;
        let state = unsafe { &mut *self.state.get() };
        if state.cache_dirty {
            self.redirect(core, self.reg(core, arm::PC)?)?;
            return Ok(true);
        }
        if state.gic.next_irq(core).is_some() && self.reg(core, arm::CPSR)? & 0x80 == 0 {
            state.irq_entries[core] += 1;
            let pc = self.reg(core, arm::PC)?;
            self.enter_exception(core, ExceptionEntry::Irq, pc)?;
            return Ok(true);
        }
        Ok(false)
    }
    /// A board instruction that exited before entry, at a boundary whose
    /// prologue already ran. `true` ends the slice (the hook's redirect).
    fn perform(&self, core: usize, word: u32) -> Result<bool, String> {
        let address = self.reg(core, arm::PC)?;
        if self.word(core, u64::from(address))? != word {
            return Err(format!("Counted XP4 exit word changed at {address:#x}"));
        }
        self.advance(core, 1)?;
        let state = unsafe { &mut *self.state.get() };
        state.last_observed[core] = Some((address, state.instructions));
        self.record(core, address, word);
        observe(self, core, u64::from(address))?;
        special(self, core, u64::from(address), word)?;
        let state = unsafe { &*self.state.get() };
        if state.redirected.is_some() || state.failure.is_some() {
            return Ok(true);
        }
        // The hook let the engine execute it: a failed condition, or an
        // identity or PMU access the board does not model. Do what the
        // engine does.
        let cpsr = self.reg(core, arm::CPSR)?;
        if word >> 28 != 15 && !condition_passes(word >> 28, cpsr) {
            self.write_reg(core, arm::PC, address + 4)?;
            return Ok(false);
        }
        if word & 0x0f00_0010 != 0x0e00_0010 || (word >> 8) & 15 != 15 {
            return Err(format!(
                "Counted XP4 exit for an unperformed instruction {word:#010x} at {address:#x}"
            ));
        }
        let rt = ((word >> 12) & 15) as usize;
        if rt == 15 {
            return Err(format!(
                "Counted XP4 coprocessor transfer to APSR at {address:#x}"
            ));
        }
        let selector = arm::Coprocessor {
            cp: 15,
            wide: false,
            secure: 0,
            opc1: (word >> 21) & 7,
            crn: (word >> 16) & 15,
            crm: word & 15,
            opc2: (word >> 5) & 7,
        };
        if word & (1 << 20) != 0 {
            let value = self.cpus[core].coprocessor(selector, None)?;
            self.write_reg(core, arm::CORE[rt], value as u32)?;
        } else {
            let value = self.reg(core, arm::CORE[rt])?;
            self.cpus[core].coprocessor(selector, Some(u64::from(value)))?;
        }
        self.write_reg(core, arm::PC, address + 4)?;
        Ok(false)
    }
    /// The hook's per-core trace entry.
    fn record(&self, core: usize, address: u32, word: u32) {
        let trace = unsafe { &mut (&mut (*self.state.get()).recent)[core] };
        if trace.len() == 32 {
            trace.pop_front();
        }
        trace.push_back((address, word));
    }
    /// Moves the engine's settled entries into the per-core trace.
    fn record_entries(&self, core: usize) -> Result<(), String> {
        let cpu = self.cpus[core]
            .counted()
            .ok_or("counted XP4 core without its engine")?;
        for address in cpu.take_entries()? {
            let address = u32::try_from(address).map_err(|_| "counted XP4 entry above 4 GiB")?;
            self.record(core, address, self.word(core, u64::from(address))?);
        }
        Ok(())
    }
    /// One scheduler slice of at most `budget` entered instructions, ended
    /// where the per-instruction hook ends it.
    pub(super) fn run_counted(&self, core: usize, budget: usize) -> Result<(), String> {
        let mut remaining = budget as u64;
        let mut exit_word: Option<u32> = None;
        loop {
            // The hook's instruction budget stops before the next entry, so
            // no boundary work happens once the slice is spent.
            if remaining == 0 {
                return Ok(());
            }
            if self.boundary(core)? {
                return Ok(());
            }
            if let Some(word) = exit_word.take() {
                remaining -= 1;
                if self.perform(core, word)? {
                    return Ok(());
                }
                continue;
            }
            let requests =
                std::mem::take(unsafe { &mut (&mut (*self.state.get()).return_requests)[core] });
            self.observe_addresses(core, &requests)?;
            // A deadline `due()` left due still runs one instruction before
            // the next `due()`, as with the hook.
            let room = self.room()?.max(1);
            let cpu = &self.cpus[core];
            let pending = unsafe { (*self.state.get()).gic.next_irq(core).is_some() };
            cpu.set_interrupt(Interrupt::Irq, pending)?;
            unsafe {
                (*self.state.get()).accounted = 0;
                (*self.state.get()).counted_core = Some(core);
            }
            let begin = u64::from(self.reg(core, arm::PC)?);
            let mut exit = cpu.run(Run {
                begin,
                until: None,
                watchdog_us: 0,
                instructions: Some(usize::try_from(remaining.min(room)).unwrap_or(usize::MAX)),
            });
            unsafe {
                (*self.state.get()).counted_core = None;
            }
            // A full maintenance queue yields the engine after its MCR, and
            // the exact run ends its slice there.
            let full = std::borrow::Borrow::<Core>::borrow(&self.cpus[core])
                .translation_maintenance_status()
                .is_ok_and(|s| s.pending >= fujimod_emulator_cpu_unicorn::maintenance::CAPACITY);
            // Entered instructions count even when settlement fails: the
            // per-instruction hook counted them before they executed.
            let entered = cpu.instruction_progress().unwrap_or(0);
            let accounted = unsafe { (*self.state.get()).accounted };
            let counted = self
                .advance(core, entered.saturating_sub(accounted))
                .and_then(|()| self.record_entries(core));
            let raw_error = exit.as_ref().err().map(ToString::to_string);
            self.maintenance
                .settle(&self.cpus, core, raw_error.as_deref())?;
            counted?;
            remaining = remaining.saturating_sub(entered);
            self.healthy()?;
            if matches!(&exit, Err(ExecutionError::InvalidInstruction(_)))
                && self.disabled_vfp_exception(core)?
            {
                exit = Ok(Exit::Stopped);
            }
            match exit {
                Ok(Exit::BeforeInstruction(word)) => exit_word = Some(word),
                Ok(Exit::Stopped) => {}
                Ok(other) => return Err(format!("Unexpected counted ARMv7-A exit {other:?}")),
                Err(error) => return Err(error.to_string()),
            }
            // An abort or undefined entry, or a yield to translation maintenance.
            if full || unsafe { (*self.state.get()).redirected.is_some() } {
                return Ok(());
            }
        }
    }
}

fn condition_passes(condition: u32, cpsr: u32) -> bool {
    let (n, z, c, v) = (
        cpsr >> 31 != 0,
        cpsr & (1 << 30) != 0,
        cpsr & (1 << 29) != 0,
        cpsr & (1 << 28) != 0,
    );
    match condition {
        0 => z,
        1 => !z,
        2 => c,
        3 => !c,
        4 => n,
        5 => !n,
        6 => v,
        7 => !v,
        8 => c && !z,
        9 => !c || z,
        10 => n == v,
        11 => n != v,
        12 => !z && n == v,
        13 => z || n != v,
        _ => true,
    }
}

/// Counted observer (labels and call returns): the hook's passive
/// observations at an entered instruction, after it counts.
pub(super) unsafe fn counted_observer(address: u64, _size: u32, data: *mut c_void) {
    let hook = unsafe { &*data.cast::<Hook>() };
    let c = unsafe { &*hook.cluster };
    let core = hook.core;
    let result = (|| -> Result<(), String> {
        // A board instruction exits right after this callback without being
        // entered; `perform` observes it at the boundary.
        let word = c.word(core, address)?;
        if fujimod_emulator_cpu::armv7a::framework_instruction(word) || exclusive_word(word) {
            return Ok(());
        }
        let progress = c.cpus[core]
            .instruction_progress()
            .ok_or("counted XP4 observer without an entry count")?;
        let state = unsafe { &mut *c.state.get() };
        let entered = state.instructions + (progress - state.accounted);
        // A restarted instruction is observed once per entry.
        if state.last_observed[core] == Some((address as u32, entered)) {
            return Ok(());
        }
        state.last_observed[core] = Some((address as u32, entered));
        c.sync_to(core, progress)?;
        let before = unsafe { (*c.state.get()).calls.len() };
        observe(c, core, address)?;
        let state = unsafe { &mut *c.state.get() };
        if state.calls.len() != before {
            let lr = state.calls[before].lr;
            if !state.observed[core].contains(&lr) {
                // Install its return observer at the next boundary, before
                // the callee can return.
                state.return_requests[core].push(lr);
                c.cpus[core].tighten_instruction_limit(state.accounted)?;
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        c.fail(core, error);
    }
}
