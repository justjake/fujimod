//! Exact AArch64 and ARMv7-A (ARM state) instruction entries, code coherence,
//! and architectural exits. Guest clocks, devices, and framework register
//! values remain with the owner.
use crate::{
    Core as RawCore,
    accounting::{Accounting, Span},
    memory::{CodeOwner, Memory},
    observers,
};
use fujimod_emulator_cpu::*;
use std::{
    cell::{Cell, RefCell, UnsafeCell},
    collections::{BTreeMap, VecDeque},
    ffi::c_void,
    ptr::NonNull,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
};
mod a64;
mod backend;
pub mod coherence;
mod coherent_cpu;
#[cfg(test)]
mod coherent_cpu_tests;
mod translation_attributes;
#[cfg(test)]
mod translation_attributes_tests;
pub use coherent_cpu::{CoherenceDomain, CoherenceOptions};
#[cfg(test)]
mod tests;

struct Observer {
    hook: Hook,
    native: Option<HookId>,
}
struct Execution {
    // Engine destruction precedes callback data, ring, and owned RAM.
    cpu: Rc<RawCore>,
    memory: Memory,
    // ARMv7-A in ARM state; otherwise AArch64.
    a32: bool,
    id: Cell<usize>,
    record: NonNull<BlockAccounting>,
    policy: Cell<ExecutionPolicy>,
    pause_requested: Option<Arc<AtomicU32>>,
    clock: Cell<Option<Clock>>,
    loop_mode: Cell<loops::LoopMode>,
    loop_window: Cell<bool>,
    prove_loops: Cell<bool>,
    proofs: a64::Proofs,
    ring: UnsafeCell<Box<[u64; 64]>>,
    owned: RefCell<Vec<fujimod_emulator_cpu::memory::Backing>>,
    count: Cell<u64>,
    cap: Cell<u64>,
    synced: Cell<i64>,
    settled_head: Cell<u64>,
    manual: Cell<Option<Span>>,
    active: Cell<bool>,
    at_block: Cell<bool>,
    #[cfg(test)]
    block_callbacks: Cell<u64>,
    at_instruction: Cell<bool>,
    stopped: Cell<bool>,
    irq: Cell<bool>,
    fiq: Cell<bool>,
    // Stop before the first block entered across an EL3 boundary, and
    // whether the current run started at EL3.
    el3_transitions: Cell<bool>,
    run_at_el3: Cell<bool>,
    restart: Cell<Option<u64>>,
    exit: Cell<Option<u32>>,
    // ARMv7-A abort reported by the engine, entered after the run.
    exception: Cell<Option<arm::Exception>>,
    entries: Cell<armv7a::Entries>,
    failure: RefCell<Option<String>>,
    observers: RefCell<BTreeMap<usize, Observer>>,
    index: RefCell<observers::Index>,
    next: Cell<usize>,
    devices: RefCell<BTreeMap<u64, (usize, HookId)>>,
    coherence: RefCell<Option<coherent_cpu::Coherent>>,
    coherent: Cell<bool>,
    stores: RefCell<Vec<(Access, u64, i32, i64)>>,
    // ARMv7-A owner options (see `Core::set_owner_exclusives` and the
    // following setters).
    exclusive_exits: Cell<bool>,
    owner_mpidr: Cell<bool>,
    board_mpidr: Cell<bool>,
    owner_interrupts: Cell<bool>,
    owner_aborts: Cell<bool>,
    // Newest entered instruction addresses, oldest first, while an owner
    // keeps an entry trace (`Core::set_entry_trace`).
    trace: RefCell<Option<EntryTrace>>,
}
struct EntryTrace {
    capacity: usize,
    entries: VecDeque<u64>,
    // The ring no longer explained a settlement: the trace is unknown.
    lost: bool,
}
impl EntryTrace {
    fn push(&mut self, address: u64) {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(address);
    }
}
pub struct Core {
    execution: Rc<Execution>,
}
impl Core {
    pub fn new(cpu: RawCore, memory: Memory) -> Result<Self, String> {
        Self::with_pause(cpu, memory, None)
    }

    #[cfg(not(target_os = "emscripten"))]
    pub(crate) fn new_parallel_worker(
        cpu: RawCore,
        memory: Memory,
        pause_requested: Arc<AtomicU32>,
    ) -> Result<Self, String> {
        Self::with_pause(cpu, memory, Some(pause_requested))
    }

    #[cfg(not(target_os = "emscripten"))]
    pub(crate) fn owner_callback_parked(&self, parked: bool) -> Result<(), String> {
        let execution = &self.execution;
        if execution.active.get() {
            execution.memory.set_parked(execution.id.get(), parked)?;
        }
        Ok(())
    }

    fn with_pause(
        cpu: RawCore,
        memory: Memory,
        pause_requested: Option<Arc<AtomicU32>>,
    ) -> Result<Self, String> {
        let a32 = cpu.model() == Model::CortexA7;
        if !(a32 || cpu.model() == Model::ArmV8A) || !cpu.features().translation_filter {
            return Err("counted A-profile requires an A-profile translation frontend".into());
        }
        for (encoding, mask) in [(0xf088, 1 << 10), (0xe088, 1 << 31)]
            .into_iter()
            .filter(|_| !a32)
        {
            let register = Register::Arm64System(arm::SystemRegister::from_encoding(encoding));
            let mut bytes = [0; 8];
            cpu.read_register(register, &mut bytes)?;
            let value = u64::from_le_bytes(bytes);
            cpu.write_register(register, &(value | mask).to_le_bytes())?;
        }
        let record = cpu
            .accounting()
            .ok_or("counted A-profile requires block accounting")?;
        let e = Rc::new(Execution {
            cpu: Rc::new(cpu),
            memory,
            a32,
            id: Cell::new(usize::MAX),
            record,
            policy: Cell::new(ExecutionPolicy::Exact),
            pause_requested,
            clock: Cell::new(None),
            loop_mode: Cell::default(),
            loop_window: Cell::new(false),
            prove_loops: Cell::new(false),
            proofs: a64::Proofs::default(),
            ring: UnsafeCell::new(Box::new([0; 64])),
            owned: RefCell::new(vec![]),
            count: Cell::new(0),
            cap: Cell::new(0),
            synced: Cell::new(0),
            settled_head: Cell::new(0),
            manual: Cell::new(None),
            active: Cell::new(false),
            at_block: Cell::new(false),
            #[cfg(test)]
            block_callbacks: Cell::new(0),
            at_instruction: Cell::new(false),
            stopped: Cell::new(false),
            irq: Cell::new(false),
            fiq: Cell::new(false),
            el3_transitions: Cell::new(false),
            run_at_el3: Cell::new(false),
            restart: Cell::new(None),
            exit: Cell::new(None),
            exception: Cell::new(None),
            entries: Cell::default(),
            failure: RefCell::new(None),
            observers: RefCell::new(BTreeMap::new()),
            index: RefCell::new(observers::Index::default()),
            next: Cell::new(1),
            devices: RefCell::new(BTreeMap::new()),
            coherence: RefCell::new(None),
            coherent: Cell::new(false),
            stores: RefCell::new(vec![]),
            exclusive_exits: Cell::new(false),
            owner_mpidr: Cell::new(false),
            board_mpidr: Cell::new(false),
            owner_interrupts: Cell::new(false),
            owner_aborts: Cell::new(false),
            trace: RefCell::new(None),
        });
        let owner: Rc<dyn CodeOwner> = e.clone();
        e.id.set(e.memory.register(Rc::downgrade(&owner)));
        let data = Rc::as_ptr(&e).cast_mut().cast();
        unsafe {
            let r = &mut *record.as_ptr();
            r.ring = (*e.ring.get()).as_mut_ptr();
            r.mask = 63;
            r.enabled = 1;
            r.pause_requested = e
                .pause_requested
                .as_ref()
                .map_or(std::ptr::null(), Arc::as_ptr);
            if a32 {
                // A device access ends its block and every other stop raised
                // from a callback takes effect at the next block entry, so
                // only block entries test for requests.
                e.cpu.set_entry_exits(true)?;
            }
            e.cpu.translation_hooks(Some(TranslationHooks {
                classify,
                restart,
                data,
            }))?;
            e.cpu.install_hook(Hook::Block {
                range: HookRange::All,
                callback: block,
                data,
            })?;
            e.cpu.install_hook(Hook::Instruction {
                range: HookRange::All,
                callback: instruction,
                data,
            })?;
            let hook = e.cpu.install_hook(Hook::Memory {
                range: HookRange::All,
                read: false,
                write: true,
                callback: written,
                data,
            })?;
            e.cpu.hook_watched(hook, true)?;
            // The translation filter above watches every page this
            // processor translates, and the shared memory watches it in
            // every alias and peer. Stores to other pages can then keep the
            // engine's inline path: only watched pages hold translated code
            // or a live exclusive reservation.
            if std::env::var_os("FUJIMOD_A64_ALL_STORES").is_none() {
                e.cpu.set_page_hooks(true)?;
            }
            if a32 {
                e.cpu.install_hook(Hook::Exception {
                    callback: exception,
                    data,
                })?;
            }
        }
        Ok(Self { execution: e })
    }
    /// The raw engine this processor runs on, for owner protocols that
    /// address it directly (translation maintenance). Execution, mapping
    /// and hooks go through this processor.
    pub fn raw(&self) -> &RawCore {
        &self.execution.cpu
    }
    fn owner_option(
        &self,
        cell: impl FnOnce(&Execution) -> &Cell<bool>,
        on: bool,
    ) -> Result<(), String> {
        let e = &self.execution;
        if e.active.get() || !e.a32 {
            return Err("owner options need a stopped ARMv7-A processor".into());
        }
        cell(e).set(on);
        Ok(())
    }
    /// ARMv7-A owners that keep their own cluster-wide exclusive monitor:
    /// LDREX/STREX (all widths) and CLREX also exit before entry, like the
    /// framework instructions, so the owner performs them.
    pub fn set_owner_exclusives(&self, on: bool) -> Result<(), String> {
        self.owner_option(|e| &e.exclusive_exits, on)?;
        self.execution.cpu.invalidate_code(None)
    }
    /// ARMv7-A owners that give each processor its affinity (`mpidr`, the
    /// value MRC p15,0,Rt,c0,c0,5 returns): the engine then reads MPIDR
    /// itself instead of exiting before the instruction.
    pub fn set_owner_mpidr(&self, mpidr: u32) -> Result<(), String> {
        let actual = self.install_mpidr(mpidr)?;
        if actual != u64::from(mpidr) {
            return Err(format!("processor affinity did not take: {actual:#x}"));
        }
        Ok(())
    }
    fn install_mpidr(&self, mpidr: u32) -> Result<u64, String> {
        self.owner_option(|e| &e.owner_mpidr, false)?;
        let selector = arm::Coprocessor {
            cp: 15,
            wide: false,
            secure: 0,
            opc1: 0,
            crn: 0,
            crm: 0,
            opc2: 5,
        };
        self.execution
            .cpu
            .coprocessor(selector, Some(u64::from(mpidr)))?;
        let actual = self.execution.cpu.coprocessor(selector, None)?;
        // Cortex-A7 hardwires some bits. Retain board emulation for a
        // configured value the engine cannot represent.
        self.execution.owner_mpidr.set(actual == u64::from(mpidr));
        self.execution.cpu.invalidate_code(None)?;
        Ok(actual)
    }
    /// ARMv7-A owners that enter IRQs at their own instruction boundaries:
    /// an asserted, unmasked IRQ stops the run before the next block instead
    /// of being entered here, so the owner sees it at an exact boundary.
    pub fn set_owner_interrupts(&self, on: bool) -> Result<(), String> {
        self.owner_option(|e| &e.owner_interrupts, on)
    }
    /// ARMv7-A owners that enter aborts themselves: exception observers
    /// installed afterwards run once the abort's entry is settled, and this
    /// processor no longer enters the abort or counts it.
    pub fn set_owner_aborts(&self, on: bool) -> Result<(), String> {
        if !self.execution.index.borrow().exceptions.is_empty() {
            return Err("owner aborts must be chosen before exception observers".into());
        }
        self.owner_option(|e| &e.owner_aborts, on)
    }
    /// Keeps the addresses of the newest `capacity` entered instructions
    /// (0 stops), in entry order, as the accounting ring settles them. The
    /// owner drains them with [`Core::take_entries`].
    pub fn set_entry_trace(&self, capacity: usize) -> Result<(), String> {
        let e = &self.execution;
        if e.active.get() {
            return Err("entry trace needs a stopped processor".into());
        }
        if capacity > 32 {
            return Err("entry trace keeps at most 32 entries".into());
        }
        if e.policy.get() == ExecutionPolicy::Interactive
            && e.trace.borrow().is_some() != (capacity != 0)
        {
            e.cpu.invalidate_code(None)?;
        }
        *e.trace.borrow_mut() = (capacity != 0).then(|| EntryTrace {
            capacity,
            entries: VecDeque::new(),
            lost: false,
        });
        Ok(())
    }
    /// Drains the entry trace, oldest first. `Err` when the accounting ring
    /// could not explain a settlement since the trace began.
    pub fn take_entries(&self) -> Result<Vec<u64>, String> {
        let mut trace = self.execution.trace.borrow_mut();
        let trace = trace.as_mut().ok_or("entry trace is off")?;
        if trace.lost {
            return Err("entry trace lost a settlement".into());
        }
        Ok(trace.entries.drain(..).collect())
    }
}
/// ARMv7-A ARM-state LDREX/STREX family (any width) and CLREX encodings.
fn exclusive_instruction(word: u32) -> bool {
    word & 0x0f90_0fff == 0x0190_0f9f || word & 0x0f90_0ff0 == 0x0180_0f90 || word == 0xf57f_f01f
}
impl Drop for Execution {
    fn drop(&mut self) {
        self.memory.forget(self.id.get());
    }
}
impl Execution {
    fn scalar(&self, r: Register) -> Result<u64, String> {
        let mut v = [0; 8];
        self.cpu.read_register(r, &mut v)?;
        Ok(u64::from_le_bytes(v))
    }
    fn accounting(&self) -> Accounting {
        unsafe { Accounting::from_raw(self.record) }
    }
    fn coarse(&self) -> bool {
        !self.a32
            && self.policy.get() == ExecutionPolicy::Interactive
            && self.trace.borrow().is_none()
    }
    fn update_guest_tick(&self) -> Result<(), String> {
        crate::GuestClockTickScope::publish(
            self.clock
                .get()
                .map(|clock| clock.after(self.count.get()).map(|clock| clock.now))
                .transpose()?,
        );
        Ok(())
    }
    fn reset(&self) {
        self.accounting().reset();
        self.synced.set(0);
        self.manual.set(None);
        self.settled_head.set(self.accounting().counters().1);
    }
    fn tail(&self, pc: u64, entered: bool) -> u64 {
        self.accounting()
            .newest_since(self.settled_head.get())
            .or(self.manual.get())
            .map_or(0, |span| span.tail(pc, entered))
    }
    fn settle(&self, tail: u64, give_back: bool) -> Result<(), String> {
        let p = self
            .accounting()
            .settle(self.synced.get(), tail, give_back)?;
        self.trace_settled(p.instructions, tail);
        self.count.set(
            self.count
                .get()
                .checked_add(p.instructions)
                .ok_or("A-profile instruction count overflow")?,
        );
        self.synced.set(p.budget);
        self.settled_head.set(self.accounting().counters().1);
        self.manual.set(None);
        Ok(())
    }
    /// Appends the entries one settlement counted: every span admitted since
    /// the last settlement ran whole, except the newest, whose `tail` did not
    /// enter. Only the newest `capacity` entries are read from the ring.
    fn trace_settled(&self, instructions: u64, tail: u64) {
        let mut trace = self.trace.borrow_mut();
        let Some(trace) = trace.as_mut().filter(|t| !t.lost) else {
            return;
        };
        if instructions == 0 {
            return;
        }
        let a = self.accounting();
        let head = a.counters().1;
        let since = self.settled_head.get();
        let complete = head.saturating_sub(since) <= 64;
        let oldest = since.max(head.saturating_sub(64));
        let wanted = instructions.min(trace.capacity as u64);
        let mut newest = Vec::with_capacity(wanted as usize);
        let mut total = 0u64;
        let mut skip = tail;
        let mut index = head;
        while index > oldest {
            index -= 1;
            let span = a.span(index);
            let Some(entered) = span.instructions.checked_sub(skip) else {
                trace.lost = true;
                return;
            };
            skip = 0;
            total += entered;
            for i in (0..entered).rev() {
                if newest.len() as u64 == wanted {
                    break;
                }
                newest.push(span.address + 4 * i);
            }
            if !complete && newest.len() as u64 == wanted {
                break;
            }
        }
        if (complete && total != instructions)
            || (newest.len() as u64) < wanted
            || self.manual.get().is_some()
        {
            trace.lost = true;
            return;
        }
        for address in newest.into_iter().rev() {
            trace.push(address);
        }
    }
    fn progress(&self) -> Option<u64> {
        if !self.active.get() || self.at_instruction.get() {
            return Some(self.count.get());
        }
        let tail = self.tail(self.scalar(Register::Pc).ok()?, !self.at_block.get());
        let delta = self
            .synced
            .get()
            .checked_sub(self.accounting().counters().0)?
            .checked_sub(tail as i64)?;
        self.count.get().checked_add(u64::try_from(delta).ok()?)
    }
    fn guarded(&self, f: impl FnOnce() -> Result<(), String>) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        let error = match result {
            Ok(Ok(())) => return,
            Ok(Err(e)) => e,
            Err(_) => "A-profile callback panicked".into(),
        };
        if self.failure.borrow().is_none() {
            *self.failure.borrow_mut() = Some(error)
        }
        let _ = self.cpu.stop();
    }
    const SELF_BRANCH: u32 = 0x1400_0000;
    const NZCV: u64 = 0xf000_0000;

    /// Whether this processor proves loops at all: coarse accounting in
    /// Interactive mode with the mode's permission. Whether the current run
    /// is in a stable window is `prove_loops`, fixed when the run begins.
    fn can_prove_loops(&self) -> bool {
        self.coarse() && self.loop_mode.get() == loops::LoopMode::Accelerate
    }

    /// Whether the running window may fast-forward a loop right now.
    fn proving(&self) -> bool {
        self.prove_loops.get() && !self.irq.get() && !self.fiq.get() && !self.memory.flush_pending()
    }

    /// Whether a loop branch at `address` could be proven. Only a backward
    /// conditional branch qualifies: an unconditional `B` back cannot leave
    /// its loop, so its run already ends at the budget and ending its block
    /// would only cost a callback. `B #0` has its own proof below.
    fn loop_candidate(&self, address: u64, word: u32) -> bool {
        word & 0x7c00_0000 != 0x1400_0000 && a64::head(address, word).is_some()
    }

    /// The bytes of the loop that closes at `address`.
    fn loop_bytes(&self, head: u64, branch: u64) -> Option<Vec<u8>> {
        let bytes = usize::try_from(branch - head).ok()?.checked_add(4)?;
        let mut code = vec![0; bytes];
        self.cpu.read_virtual(head, Access::Fetch, &mut code).ok()?;
        Some(code)
    }

    /// Fast-forward a proven AArch64 loop whose branch instruction, at
    /// `address`, is `word`. Each proven round is one execution of that
    /// branch and the body after it, so the owner charges `rounds * length`
    /// entries and leaves the branch to execute once more, exactly where
    /// ordinary execution would be at that count.
    ///
    /// Nothing but the loop's instruction bytes, the architectural registers
    /// and RAM the owner declared stable takes part: no firmware address,
    /// function identity or RTOS structure.
    fn advance_proven_loop(&self, address: u64, word: u32) -> Result<bool, String> {
        if self.a32 || !self.proving() {
            return Ok(false);
        }
        if word == Self::SELF_BRANCH {
            // Every iteration from here leaves the same architectural state
            // at the same boundary: retire the whole budget.
            self.count.set(self.cap.get());
            self.cpu.stop()?;
            return Ok(true);
        }
        let proofs = &self.proofs;
        let Some(head) = a64::head(address, word) else {
            return Ok(false);
        };
        proofs.count(&proofs.candidate_entries, 1);
        // Instruction, block and store observers on the loop must see every
        // round.
        let observers = self.index.borrow();
        if observers.instructions.any_in(head, address)
            || observers.blocks.any_in(head, address)
            || !observers.stores.is_empty()
        {
            proofs.count(&proofs.failed_observer, 1);
            return Ok(false);
        }
        drop(observers);
        let Some(bytes) = self.loop_bytes(head, address) else {
            proofs.count(&proofs.failed_read_unavailable, 1);
            return Ok(false);
        };
        if proofs
            .failures
            .borrow()
            .get(&address)
            .is_some_and(|cached| *cached == bytes)
        {
            proofs.count(&proofs.failed_cache, 1);
            return Ok(false);
        }
        proofs.count(&proofs.proofs_attempted, 1);
        let Some(proof) = a64::Proof::decode(head, address, &bytes) else {
            proofs.count(&proofs.failed_structural, 1);
            proofs.failures.borrow_mut().insert(address, bytes);
            return Ok(false);
        };
        // A loop that reads RAM must not skip a page the owner is watching.
        if proof.reads() && !self.index.borrow().reads.is_empty() {
            proofs.count(&proofs.failed_observer, 1);
            return Ok(false);
        }
        let mut state = a64::State::default();
        for (index, slot) in state.registers.iter_mut().enumerate() {
            *slot = if index == 31 {
                self.scalar(Register::Sp)?
            } else {
                self.scalar(Register::X(index as u8))?
            };
        }
        // The proof keeps only the NZCV flags it models.
        state.flags = self.scalar(Register::Pstate)? as u32 & Self::NZCV as u32;
        // The branch entry is already counted. The rounds begin after it and
        // end one entry before the budget, so the branch itself always has its
        // entry left to run.
        let room = self.cap.get().saturating_sub(self.count.get());
        let rounds = room.saturating_sub(1) / proof.length;
        if rounds == 0 {
            proofs.count(&proofs.failed_budget, 1);
            return Ok(false);
        }
        let advanced = proof.advance(state, rounds, |address, bits| {
            let width = usize::from(u8::try_from(bits / 8).ok()?);
            let mut bytes = [0; 8];
            // Permission-checked and observational; a device page refuses.
            self.cpu
                .read_virtual(address, Access::Read, &mut bytes[..width])
                .ok()?;
            Some(u64::from_le_bytes(bytes))
        });
        let advance = match advanced {
            Ok(advance) => advance,
            Err(a64::Failure::Leaving) => {
                proofs.count(&proofs.failed_leaving, 1);
                return Ok(false);
            }
            Err(a64::Failure::Input) => {
                proofs.count(&proofs.failed_read_unavailable, 1);
                return Ok(false);
            }
            Err(a64::Failure::Model) => {
                // A body with no load is refused for its shape, which the
                // bytes alone decide, so the refusal is cached. A poll of
                // RAM changes with its input, so it is retried: the value
                // that ended this window may hold for the next one.
                if !proof.reads() {
                    proofs.failures.borrow_mut().insert(address, bytes);
                }
                proofs.count(&proofs.failed_model, 1);
                return Ok(false);
            }
        };
        let skipped = advance.iterations * proof.length;
        if skipped == 0 || skipped > room {
            proofs.count(&proofs.failed_budget, 1);
            return Ok(false);
        }
        let counter = if advance.counted {
            &proofs.counted_instructions
        } else {
            &proofs.poll_instructions
        };
        proofs.count(counter, skipped);
        proofs.count(&proofs.accelerated_instructions, skipped);
        for (index, value) in advance.state.registers.iter().enumerate() {
            if *value != state.registers[index] {
                let register = if index == 31 {
                    Register::Sp
                } else {
                    Register::X(index as u8)
                };
                self.cpu.write_register(register, &value.to_le_bytes())?;
            }
        }
        if advance.state.flags != state.flags {
            let pstate = self.scalar(Register::Pstate)?;
            let next = (pstate & !Self::NZCV) | u64::from(advance.state.flags & Self::NZCV as u32);
            self.cpu
                .write_register(Register::Pstate, &next.to_le_bytes())?;
        }
        self.count.set(self.count.get() + skipped);
        // The branch runs normally next; the run then ends at that entry, so
        // the owner sees the same state a full-speed run would have reached.
        Ok(false)
    }

    fn classify(&self, address: u64, word: u32) -> Result<Translation, String> {
        let physical = self.cpu.translate(address, Access::Fetch)?;
        if self.memory.note_code(self.id.get(), physical, 4)? {
            self.memory.apply_watches()?;
        }
        let framework = self.framework(word);
        let boundary = if self.a32 {
            armv7a::block_boundary(word) && !(self.owner_mpidr.get() && armv7a::mpidr_read(word))
        } else {
            word & 0xfff00000 == 0xd5100000
                || word & 0xfffff0df == 0xd50340df
                || word & 0xfffffc1f == 0xd69f0000
                || word & 0xff000000 == 0xd4000000
        };
        let coarse = self.coarse();
        let wide_exact = address > u32::MAX as u64 && !coarse;
        let instruction = if framework {
            InstructionDisposition::Replace
        } else if wide_exact
            || self.can_prove_loops() && !self.a32 && self.loop_candidate(address, word)
            || boundary
            || (self.is_coherent()
                && (coherent_cpu::maintenance(word).is_some()
                    || coherent_cpu::translation_change(word)))
            || !self.index.borrow().stores.is_empty()
            || !self.index.borrow().instructions.at(address).is_empty()
        {
            InstructionDisposition::ObserveAndEndBlock
        } else {
            InstructionDisposition::Execute
        };
        Ok(Translation {
            instruction,
            observe_block: wide_exact
                || (coarse && !matches!(instruction, InstructionDisposition::Execute))
                || !self.index.borrow().blocks.at(address).is_empty(),
        })
    }
    fn framework(&self, word: u32) -> bool {
        if self.a32 {
            (armv7a::framework_instruction(word)
                && !(self.owner_mpidr.get() && armv7a::mpidr_read(word)))
                || (self.exclusive_exits.get() && exclusive_instruction(word))
        } else {
            arm64::framework_instruction(word)
        }
    }
    fn publish(&self) -> Result<(), String> {
        for (access, address, size, value) in std::mem::take(&mut *self.stores.borrow_mut()) {
            for (_, hook) in self.index.borrow().stores.at(address) {
                if let Hook::CompletedStore { callback, data, .. } = *hook {
                    unsafe { callback(access, address, size, value, data) }
                }
            }
        }
        Ok(())
    }
    fn block(&self, b: Block) -> Result<(), String> {
        #[cfg(test)]
        self.block_callbacks.set(self.block_callbacks.get() + 1);
        self.settle(0, true)?;
        self.update_guest_tick()?;
        self.publish()?;
        self.at_block.set(true);
        let result = self.enter(b);
        self.at_block.set(false);
        result
    }
    fn enter(&self, b: Block) -> Result<(), String> {
        if self.stopped.get() {
            return Ok(());
        }
        if self
            .pause_requested
            .as_ref()
            .is_some_and(|pause| pause.load(Ordering::Acquire) != 0)
        {
            self.stopped.set(true);
            self.reset();
            return self.cpu.stop();
        }
        if self.memory.flush_pending() && self.restart.get() != Some(b.address) {
            self.reset();
            return self.cpu.stop();
        }
        if self.el3_transitions.get() && self.at_el3()? != self.run_at_el3.get() {
            self.reset();
            return self.cpu.stop();
        }
        if self.deliver_interrupt()? {
            self.reset();
            return self.cpu.stop();
        }
        for (_, hook) in self.index.borrow().blocks.at(b.address) {
            if let Hook::Block { callback, data, .. } = *hook {
                unsafe { callback(b, data) }
            }
        }
        if self.stopped.get() {
            return Ok(());
        }
        let mut room = self.cap.get().saturating_sub(self.count.get());
        if self.restart.get() == Some(b.address) {
            room = room.min(1);
            self.restart.set(None)
        }
        if room == 0 {
            self.reset();
            return self.cpu.stop();
        }
        // The translation filter and the 4-byte span ring cover ARM state only.
        if self.a32
            && b.thumb
                .map_or_else(|| Ok::<_, String>(self.scalar(arm::CPSR)? & 0x20 != 0), Ok)?
        {
            return Err("Thumb execution is unsupported by the counted ARMv7-A backend".into());
        }
        if b.bytes == 0 || !b.bytes.is_multiple_of(4) {
            return Err("invalid A-profile block span".into());
        }
        let n = u64::from(b.bytes / 4);
        if n > room {
            self.reset();
            return self.cpu.retry_block(room as u32);
        }
        let span = Span {
            address: b.address,
            instructions: n,
        };
        let coarse = self.coarse();
        if !coarse
            && b.address
                .checked_add(n * 4)
                .is_some_and(|end| end <= 1 << 32)
        {
            unsafe { (*self.record.as_ptr()).enabled = 1 };
            self.synced.set(self.accounting().admit(span, room)?);
        } else {
            // The packed ring only holds 32-bit addresses. A manually
            // admitted block can retain its entire address and skip the ring.
            unsafe {
                (*self.record.as_ptr()).enabled = if coarse { 2 } else { 0 };
                (*self.record.as_ptr()).budget = (room - n) as i64
            };
            self.synced.set(room as i64);
            self.manual.set(Some(span));
        }
        Ok(())
    }
    fn instruction(&self, address: u64, size: u32) -> Result<(), String> {
        self.settle(self.tail(address, false), false)?;
        if self.count.get() >= self.cap.get() {
            self.reset();
            return self.cpu.stop();
        }
        self.count.set(self.count.get() + 1);
        self.at_instruction.set(true);
        let traced = self.trace.borrow().is_some();
        for (_, hook) in self.index.borrow().instructions.at(address) {
            if let Hook::Instruction { callback, data, .. } = *hook {
                unsafe { callback(address, size, data) }
            }
        }
        let intercepted = self.stopped.get();
        if !intercepted && self.scalar(Register::Pc)? == address {
            let mut bytes = [0; 4];
            self.cpu.read_virtual(address, Access::Fetch, &mut bytes)?;
            let word = u32::from_le_bytes(bytes);
            if self.framework(word) {
                self.count.set(self.count.get() - 1);
                self.exit.set(Some(word));
                self.cpu.stop()?;
            } else if self.advance_proven_loop(address, word)? {
                self.cpu.stop()?;
            }
        }
        if self.is_coherent() && self.exit.get().is_none() && !intercepted {
            self.coherent_instruction(address)?;
        }
        if traced
            && self.exit.get().is_none()
            && let Some(trace) = self.trace.borrow_mut().as_mut()
        {
            trace.push(address);
        }
        self.at_instruction.set(false);
        self.reset();
        Ok(())
    }
    fn at_el3(&self) -> Result<bool, String> {
        Ok((self.scalar(Register::Pstate)? >> 2) & 3 == 3)
    }
    fn deliver_interrupt(&self) -> Result<bool, String> {
        if self.a32 {
            if self.owner_interrupts.get() {
                // Stop for the owner; it enters the IRQ itself.
                return Ok(self.irq.get() && self.scalar(arm::CPSR)? & 0x80 == 0);
            }
            // The ARMv7-A board profile routes IRQ only.
            let entered = self.irq.get() && armv7a::enter_irq(&*self.cpu)?;
            if entered {
                self.count_entry(|n| n.irq += 1);
            }
            return Ok(entered);
        }
        arm64::enter_interrupt(&*self.cpu, self.irq.get(), self.fiq.get())
    }
    fn count_entry(&self, add: impl FnOnce(&mut armv7a::Entries)) {
        let mut entries = self.entries.get();
        add(&mut entries);
        self.entries.set(entries);
    }
    fn tighten(&self, total: u64) -> Result<(), String> {
        if !self.active.get() {
            return Err("no active A-profile run to tighten".into());
        }
        if total < self.cap.get() {
            if !self.at_instruction.get() {
                self.settle(
                    self.tail(self.scalar(Register::Pc)?, !self.at_block.get()),
                    true,
                )?;
            }
            self.cap.set(total.max(self.count.get()));
            self.reset();
        }
        Ok(())
    }
    fn device(&self, address: u64, bytes: usize) -> Result<(), String> {
        let hook = unsafe {
            self.cpu.install_hook(Hook::Memory {
                range: HookRange::Inclusive {
                    begin: address,
                    end: address + bytes as u64 - 1,
                },
                read: true,
                write: true,
                callback: device,
                data: std::ptr::null_mut(),
            })
        }?;
        self.devices.borrow_mut().insert(address, (bytes, hook));
        Ok(())
    }
}
impl CodeOwner for Execution {
    fn watch_code(&self, address: u64, bytes: u64) -> Result<(), String> {
        self.cpu.watch_stores(address, bytes)
    }
    fn invalidate_backing_page(&self, backing_page: usize) -> Result<(), String> {
        self.cpu.invalidate_backing(backing_page, 4096)
    }
    fn prepare_code_write(&self) -> Result<(), String> {
        if self.active.get() && !self.at_instruction.get() {
            self.settle(self.tail(self.scalar(Register::Pc)?, true), true)?;
        }
        self.reset();
        Ok(())
    }
}
unsafe fn classify(data: *mut c_void, address: u64, word: u32) -> Translation {
    let e = unsafe { &*data.cast::<Execution>() };
    let mut result = Translation {
        instruction: InstructionDisposition::ObserveAndEndBlock,
        observe_block: true,
    };
    e.guarded(|| {
        result = e.classify(address, word)?;
        Ok(())
    });
    result
}
unsafe fn restart(data: *mut c_void, pc: u64) {
    let e = unsafe { &*data.cast::<Execution>() };
    e.guarded(|| {
        e.settle(e.tail(pc, false), true)?;
        e.reset();
        e.restart.set(Some(pc));
        Ok(())
    })
}
unsafe fn block(b: Block, data: *mut c_void) {
    let e = unsafe { &*data.cast::<Execution>() };
    e.guarded(|| e.block(b))
}
unsafe fn instruction(address: u64, size: u32, data: *mut c_void) {
    let e = unsafe { &*data.cast::<Execution>() };
    e.guarded(|| e.instruction(address, size))
}
/// A data abort retires its instruction's entry; a prefetch abort has none.
unsafe fn exception(exception: arm::Exception, data: *mut c_void) {
    let e = unsafe { &*data.cast::<Execution>() };
    e.guarded(|| {
        let owned = e.owner_aborts.get();
        if !owned
            && !matches!(
                exception,
                arm::Exception::PrefetchAbort | arm::Exception::DataAbort
            )
        {
            return Err(format!("unmodeled ARMv7-A exception {exception:?}"));
        }
        e.settle(
            e.tail(
                e.scalar(Register::Pc)?,
                exception == arm::Exception::DataAbort,
            ),
            true,
        )?;
        if owned {
            // The owner enters it (or refuses it) from its observers.
            let hooks = e.index.borrow().exceptions.clone();
            for (_, hook) in hooks {
                if let Hook::Exception { callback, data } = hook {
                    unsafe { callback(exception, data) }
                }
            }
            return e.cpu.stop();
        }
        e.exception.set(Some(exception));
        e.cpu.stop()
    })
}
unsafe fn device(_: Access, _: u64, _: i32, _: i64, _: *mut c_void) {}
/// Host-only diagnostic: with `FUJIMOD_ACCESS_HISTOGRAM` set, count the
/// stores that reach an A-profile processor's code-write hook by processor
/// and 4 KiB page, and print the most frequent pages on request.
pub mod store_histogram {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static COUNTS: RefCell<Option<HashMap<(usize, u64), u64>>> = RefCell::new(
            std::env::var_os("FUJIMOD_ACCESS_HISTOGRAM").map(|_| HashMap::new()),
        );
    }
    #[inline]
    pub(super) fn count(core: usize, address: u64) {
        COUNTS.with(|counts| {
            if let Some(counts) = counts.borrow_mut().as_mut() {
                *counts.entry((core, address >> 12)).or_default() += 1;
            }
        });
    }
    pub fn report() {
        COUNTS.with(|counts| {
            if let Some(counts) = counts.borrow_mut().as_mut() {
                let total: u64 = counts.values().sum();
                let pages = counts.len();
                let mut rows: Vec<_> = counts.drain().collect();
                rows.sort_by_key(|&(key, n)| (std::cmp::Reverse(n), key));
                eprintln!(
                    "FUJIMOD store histogram: {total} hooked A-profile stores on {pages} pages"
                );
                for ((core, page), n) in rows.into_iter().take(16) {
                    eprintln!(
                        "FUJIMOD store histogram: CPU{core} page {:#x} count={n}",
                        page << 12
                    );
                }
            }
        });
    }
}
unsafe fn written(access: Access, address: u64, size: i32, value: i64, data: *mut c_void) {
    let e = unsafe { &*data.cast::<Execution>() };
    e.guarded(|| {
        store_histogram::count(e.id.get(), address);
        if e.pause_requested.is_some() && e.policy.get() == ExecutionPolicy::Interactive {
            let observers = e.index.borrow().synchronization.clone();
            for (_, hook) in observers {
                if let Hook::Synchronization { callback, data } = hook {
                    unsafe { callback(data) };
                }
            }
        }
        if e.memory.guest_store(e.id.get(), address, size as usize)? {
            e.cpu.request_restart()?;
        }
        if !e.index.borrow().stores.at(address).is_empty() {
            e.stores.borrow_mut().push((access, address, size, value));
        }
        Ok(())
    })
}

#[cfg(test)]
mod interactive_tests {
    use super::*;
    use std::sync::{Arc, atomic::AtomicU32};

    fn core() -> Core {
        let raw = crate::test_support::library()
            .open(Model::ArmV8A, 1 << 20)
            .unwrap();
        let cpu = Core::new(raw, Memory::default()).unwrap();
        set(&cpu, Register::Pstate, 0x3cd);
        cpu
    }

    fn set(cpu: &Core, register: Register, value: u64) {
        cpu.write_register(register, &value.to_le_bytes()).unwrap();
    }

    fn value(cpu: &Core, register: Register) -> u64 {
        cpu.execution.scalar(register).unwrap()
    }

    fn run(cpu: &Core, begin: u64, instructions: usize) {
        assert!(matches!(
            cpu.run(Run {
                begin,
                until: None,
                watchdog_us: 0,
                instructions: Some(instructions),
            }),
            Ok(Exit::Stopped)
        ));
    }

    #[test]
    fn coarse_blocks_keep_exact_budgets_without_ring_writes() {
        for address in [0x1000, 0x1_0000_0000] {
            let cpu = core();
            cpu.map_zeroed(address, 0x4000, 7).unwrap();
            let code = [0x9100_0400u32, 0x9100_0400, 0x17ff_fffe];
            let bytes: Vec<_> = code.into_iter().flat_map(u32::to_le_bytes).collect();
            cpu.write_memory(address, &bytes).unwrap();
            cpu.set_execution_policy(ExecutionPolicy::Interactive)
                .unwrap();
            for budget in [1, 2, 3, 4, 196, 197, 198] {
                set(&cpu, Register::X(0), 0);
                cpu.execution.block_callbacks.set(0);
                run(&cpu, address, budget);
                assert!(cpu.execution.block_callbacks.get() <= 5);
                assert_eq!(cpu.instruction_progress(), Some(budget as u64));
                assert_eq!(value(&cpu, Register::Pc), address + (budget % 3) as u64 * 4);
                assert_eq!(value(&cpu, Register::X(0)), (budget - budget / 3) as u64);
                assert_eq!(cpu.execution.accounting().counters().1, 0);
                assert_eq!(unsafe { cpu.execution.record.as_ref() }.enabled, 2);
            }
            assert!(matches!(
                cpu.execution
                    .classify(address, code[0])
                    .unwrap()
                    .instruction,
                InstructionDisposition::Execute
            ));
        }
    }

    #[test]
    fn self_branch_proof_matches_interpreter_state_and_progress() {
        let cpu = core();
        set(&cpu, Register::Pstate, 0x3c5);
        cpu.map_zeroed(0x1000, 0x4000, 7).unwrap();
        cpu.write_memory(0x1000, &0x1400_0000u32.to_le_bytes())
            .unwrap();
        cpu.set_execution_policy(ExecutionPolicy::Interactive)
            .unwrap();
        let initial = cpu.snapshot(None).unwrap();
        let execute = |mode| {
            cpu.restore(&*initial).unwrap();
            cpu.set_loop_mode(mode).unwrap();
            cpu.set_loop_window(true).unwrap();
            run(&cpu, 0x1000, 513);
            assert_eq!(cpu.instruction_progress(), Some(513));
            cpu.state().unwrap()
        };
        assert_eq!(
            execute(loops::LoopMode::Disabled),
            execute(loops::LoopMode::Accelerate)
        );
    }

    #[test]
    fn countdown_proof_matches_native_entries_and_flags() {
        let cpu = core();
        let address = 0x1_0000_0000;
        cpu.map_zeroed(address, 0x4000, 7).unwrap();
        cpu.set_execution_policy(ExecutionPolicy::Interactive)
            .unwrap();
        for (subtract, counter) in [
            (0x7100_0400u32, (1u64 << 31) + 1),
            (0xf100_0400, (1u64 << 63) + 1),
            (0xf100_0c00, 9),
        ] {
            let words = [subtract, 0x54ff_ffe1u32, 0x1400_0000];
            let bytes: Vec<_> = words.into_iter().flat_map(u32::to_le_bytes).collect();
            cpu.write_memory(address, &bytes).unwrap();
            for budget in [4, 5, 6, 7, 99] {
                let execute = |mode| {
                    cpu.set_loop_mode(mode).unwrap();
                    set(&cpu, Register::Pstate, 0x3c5);
                    set(&cpu, Register::X(0), counter);
                    cpu.set_loop_window(true).unwrap();
                    run(&cpu, address, budget);
                    (
                        value(&cpu, Register::X(0)),
                        value(&cpu, Register::Pc),
                        value(&cpu, Register::Pstate),
                        cpu.instruction_progress(),
                    )
                };
                assert_eq!(
                    execute(loops::LoopMode::Disabled),
                    execute(loops::LoopMode::Accelerate)
                );
            }
        }
    }

    /// Run a loop for a budget both with and without the prover, and compare
    /// every architectural register, PSTATE and the entry count. The two runs
    /// must be indistinguishable.
    fn parity(code: &[u32], set_up: impl Fn(&Core), budget: usize) {
        let run_both = |mode| {
            let cpu = core();
            cpu.map_zeroed(0x1000, 0x4000, 7).unwrap();
            // A `b .` after the loop is where it lands when it leaves.
            let mut words = code.to_vec();
            words.push(0x1400_0000);
            cpu.write_memory(
                0x1000,
                &words.iter().flat_map(|word| word.to_le_bytes()).collect::<Vec<_>>(),
            )
            .unwrap();
            set_up(&cpu);
            cpu.set_execution_policy(ExecutionPolicy::Interactive)
                .unwrap();
            cpu.set_loop_mode(mode).unwrap();
            cpu.set_loop_window(true).unwrap();
            let result = cpu.run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(budget),
            });
            assert!(
                result.is_ok(),
                "{result:?} code={code:08x?} budget={budget} pc={:#x}",
                cpu.execution.scalar(Register::Pc).unwrap()
            );
            let mut state = Vec::new();
            for index in 0..=30 {
                state.push(value(&cpu, Register::X(index)));
            }
            state.push(value(&cpu, Register::Sp));
            state.push(value(&cpu, Register::Pc));
            state.push(value(&cpu, Register::Pstate) & 0xf000_0000);
            (state, cpu.instruction_progress())
        };
        assert_eq!(
            run_both(loops::LoopMode::Disabled),
            run_both(loops::LoopMode::Accelerate),
            "code={code:08x?} budget={budget}"
        );
    }

    #[test]
    fn every_proven_loop_class_matches_plain_execution() {
        // subs w1,w1,#1 ; b.ne back (a register countdown).
        let countdown = [0x7100_0421u32, b_ne(1)];
        // subs x3,x3,#4 ; cmp x3,#0 ; b.ne back (a wide countdown).
        let wide = [0xf100_1063u32, 0xf100_007f, b_ne(2)];
        // ldr w0,[x4] ; cmp w0,#1 ; b.ne back (a stable RAM flag poll).
        // The flag word lives past the code in the same page.
        let poll = [0xb940_0080u32, 0x7100_041f, b_ne(2)];
        // mrs x5,cntfrq_el0 ; ldr w0,[x4] ; cmp w0,#1 ; b.ne back.
        let clock = [
            0xd50b_7e25u32,
            0xb940_0080,
            0x7100_041f,
            b_ne(3),
        ];
        let flag = |cpu: &Core| {
            cpu.write_memory(0x2000, &1u32.to_le_bytes()).unwrap();
            set_x(cpu, 4, 0x2000);
        };
        let cases: [(&[u32], fn(&Core)); 4] = [
            (&countdown, |cpu| set_x(cpu, 1, 7)),
            (&wide, |cpu| set_x(cpu, 3, 20)),
            (&poll, flag),
            (&clock, flag),
        ];
        for (code, set_up) in cases {
            for budget in [3, 4, 5, 7, 12, 40, 513] {
                parity(code, set_up, budget);
            }
        }
    }

    fn set_x(cpu: &Core, register: u8, value: u64) {
        set(cpu, Register::X(register), value)
    }

    /// `b.ne` back to the head from `count` instructions later.
    fn b_ne(count: u32) -> u32 {
        let offset = -i64::from(count);
        0x5400_0001 | ((((offset as u64) & 0x7ffff) as u32) << 5)
    }

    unsafe fn observe_store(_: Access, _: u64, _: i32, _: i64, data: *mut c_void) {
        let stores = unsafe { &*data.cast::<Cell<u64>>() };
        stores.set(stores.get() + 1);
    }

    #[test]
    fn wide_code_does_not_watch_unrelated_store_pages() {
        let cpu = core();
        let data = [0x5000, 0x1_0000_4000];
        let high_code = 0x1_0000_0000;
        for address in [0x1000, high_code].into_iter().chain(data) {
            cpu.map_zeroed(address, 0x4000, 7).unwrap();
        }
        cpu.write_memory(high_code, &0xd503_201fu32.to_le_bytes())
            .unwrap(); // nop
        run(&cpu, high_code, 1);
        let words = [0xb900_0020u32, 0x17ff_ffff]; // str w0,[x1]; b to the store
        let bytes: Vec<_> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        cpu.write_memory(0x1000, &bytes).unwrap();
        let stores = Cell::new(0u64);
        let hook = unsafe {
            cpu.raw().install_hook(Hook::Memory {
                range: HookRange::All,
                callback: observe_store,
                data: (&raw const stores).cast_mut().cast(),
                read: false,
                write: true,
            })
        }
        .unwrap();
        cpu.raw().hook_watched(hook, true).unwrap();
        cpu.raw().set_page_hooks(true).unwrap();
        set(&cpu, Register::X(0), u64::from(0xd503_201fu32));
        for address in data {
            set(&cpu, Register::X(1), address);
            run(&cpu, 0x1000, 200);
            // An incidental slow-path store may reach the observation hook.
            assert!(stores.replace(0) < 10);
        }
        set(&cpu, Register::X(1), high_code);
        run(&cpu, 0x1000, 20);
        // Native code invalidation can retry a watched store.
        assert!(stores.get() >= 10);
        cpu.raw().remove_hook(hook).unwrap();
    }

    unsafe fn read(_: u64, _: u32, data: *mut c_void) -> u64 {
        let reads = unsafe { &*data.cast::<Cell<u32>>() };
        reads.set(reads.get() + 1);
        42
    }

    unsafe fn write(_: u64, _: u32, _: u64, _: *mut c_void) {}

    #[test]
    fn fast_reads_use_interactive_clock_and_exact_reads_use_device() {
        let cpu = core();
        cpu.map_zeroed(0x1000, 0x4000, 7).unwrap();
        let code = [0xb940_0020u32, 0xb940_0422]; // ldr w0,[x1]; ldr w2,[x1,#4]
        let bytes: Vec<_> = code.into_iter().flat_map(u32::to_le_bytes).collect();
        cpu.write_memory(0x1000, &bytes).unwrap();
        set(&cpu, Register::X(1), 0x8000);
        let reads = Cell::new(0u32);
        unsafe {
            cpu.map_mmio_fast(
                0x8000,
                0x1000,
                Mmio {
                    read,
                    write,
                    data: (&raw const reads).cast_mut().cast(),
                },
                vec![
                    FastMmioRead::atomic_u32(0, Arc::new(AtomicU32::new(123))),
                    FastMmioRead::clocked_u32(4, Arc::new(|tick| u32::try_from(tick).ok())),
                ],
            )
        }
        .unwrap();
        cpu.set_guest_clock(Clock {
            now: 100,
            phase: 0,
            ratio: timeline::Ratio::new(1, 1).unwrap(),
        })
        .unwrap();

        cpu.set_execution_policy(ExecutionPolicy::Interactive)
            .unwrap();
        run(&cpu, 0x1000, 2);
        assert_eq!(reads.get(), 0);
        assert_eq!(value(&cpu, Register::X(0)), 123);
        assert_eq!(value(&cpu, Register::X(2)), 101);
        assert_eq!(crate::approximate_guest_tick(), None);

        cpu.set_execution_policy(ExecutionPolicy::Exact).unwrap();
        run(&cpu, 0x1000, 2);
        assert_eq!(reads.get(), 2);
        assert_eq!(value(&cpu, Register::X(0)), 42);
        assert_eq!(value(&cpu, Register::X(2)), 42);
    }
}
