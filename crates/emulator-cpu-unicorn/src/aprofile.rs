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
};
mod backend;
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
    ring: UnsafeCell<Box<[u64; 64]>>,
    owned: RefCell<Vec<fujimod_emulator_cpu::memory::Backing>>,
    count: Cell<u64>,
    cap: Cell<u64>,
    synced: Cell<i64>,
    settled_head: Cell<u64>,
    manual: Cell<Option<Span>>,
    active: Cell<bool>,
    at_block: Cell<bool>,
    at_instruction: Cell<bool>,
    stopped: Cell<bool>,
    irq: Cell<bool>,
    fiq: Cell<bool>,
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
    write_hook: Cell<Option<HookId>>,
    wide_code_watch: Cell<bool>,
    stores: RefCell<Vec<(Access, u64, i32, i64)>>,
    // ARMv7-A owner options (see `Core::set_owner_exclusives` and the
    // following setters).
    exclusive_exits: Cell<bool>,
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
            ring: UnsafeCell::new(Box::new([0; 64])),
            owned: RefCell::new(vec![]),
            count: Cell::new(0),
            cap: Cell::new(0),
            synced: Cell::new(0),
            settled_head: Cell::new(0),
            manual: Cell::new(None),
            active: Cell::new(false),
            at_block: Cell::new(false),
            at_instruction: Cell::new(false),
            stopped: Cell::new(false),
            irq: Cell::new(false),
            fiq: Cell::new(false),
            restart: Cell::new(None),
            exit: Cell::new(None),
            exception: Cell::new(None),
            entries: Cell::default(),
            failure: RefCell::new(None),
            observers: RefCell::new(BTreeMap::new()),
            index: RefCell::new(observers::Index::default()),
            next: Cell::new(1),
            devices: RefCell::new(BTreeMap::new()),
            write_hook: Cell::new(None),
            wide_code_watch: Cell::new(false),
            stores: RefCell::new(vec![]),
            exclusive_exits: Cell::new(false),
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
            e.write_hook.set(Some(hook));
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
    fn classify(&self, address: u64, word: u32) -> Result<Translation, String> {
        let physical = self.cpu.translate(address, Access::Fetch)?;
        self.memory.note_code(self.id.get(), physical, 4)?;
        self.memory.apply_watches()?;
        let framework = self.framework(word);
        let boundary = if self.a32 {
            armv7a::block_boundary(word)
        } else {
            word & 0xfff00000 == 0xd5100000
                || word & 0xfffff0df == 0xd50340df
                || word & 0xfffffc1f == 0xd69f0000
                || word & 0xff000000 == 0xd4000000
        };
        Ok(Translation {
            instruction: if framework {
                InstructionDisposition::Replace
            } else if address > u32::MAX as u64
                || boundary
                || !self.index.borrow().stores.is_empty()
                || !self.index.borrow().instructions.at(address).is_empty()
            {
                InstructionDisposition::ObserveAndEndBlock
            } else {
                InstructionDisposition::Execute
            },
            observe_block: address > u32::MAX as u64
                || !self.index.borrow().blocks.at(address).is_empty(),
        })
    }
    fn framework(&self, word: u32) -> bool {
        if self.a32 {
            armv7a::framework_instruction(word)
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
        self.settle(0, true)?;
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
        if self.memory.flush_pending() && self.restart.get() != Some(b.address) {
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
        if b.address
            .checked_add(n * 4)
            .is_some_and(|end| end <= 1 << 32)
        {
            unsafe { (*self.record.as_ptr()).enabled = 1 };
            self.synced.set(self.accounting().admit(span, room)?);
        } else {
            // High virtual addresses retain exact entries through single-
            // instruction observations; the packed span ring stays 32-bit.
            unsafe {
                (*self.record.as_ptr()).enabled = 0;
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
            }
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
        if self.wide_code_watch.get() {
            return Ok(());
        }
        if address.checked_add(bytes).is_none_or(|end| end > 1 << 32) {
            // The optimized native watch bitmap covers 32-bit physical pages.
            // Keep full-width code coherent by observing every write once a
            // high page is translated; do not truncate or omit that dependency.
            self.cpu.hook_watched(
                self.write_hook.get().ok_or("missing code-write hook")?,
                false,
            )?;
            self.wide_code_watch.set(true);
            if std::env::var_os("FUJIMOD_EXECUTION_COUNTERS").is_some() {
                eprintln!(
                    "FUJIMOD execution counters: CPU{} observes every store from now on: code at {address:#x} is beyond the store watch bitmap",
                    self.id.get()
                );
            }
            Ok(())
        } else {
            self.cpu.watch_stores(address, bytes)
        }
    }
    fn invalidate_code_page(&self, _address: u64) -> Result<(), String> {
        self.cpu.invalidate_code(None)
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
unsafe fn written(access: Access, address: u64, size: i32, value: i64, data: *mut c_void) {
    let e = unsafe { &*data.cast::<Execution>() };
    e.guarded(|| {
        if e.memory.guest_store(e.id.get(), address, size as usize)? {
            e.cpu.request_restart()?;
        }
        if !e.index.borrow().stores.at(address).is_empty() {
            e.stores.borrow_mut().push((access, address, size, value));
        }
        Ok(())
    })
}
