//! Exact AArch64 instruction entries, code coherence, and architectural exits.
//! Guest clocks, devices, and framework register values remain with the owner.
use crate::{
    Core as RawCore,
    accounting::{Accounting, Span},
    memory::{CodeOwner, Memory},
    observers,
};
use fujimod_emulator_cpu::*;
use std::{
    cell::{Cell, RefCell, UnsafeCell},
    collections::BTreeMap,
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
    failure: RefCell<Option<String>>,
    observers: RefCell<BTreeMap<usize, Observer>>,
    index: RefCell<observers::Index>,
    next: Cell<usize>,
    devices: RefCell<BTreeMap<u64, (usize, HookId)>>,
    stores: RefCell<Vec<(Access, u64, i32, i64)>>,
}
pub struct Core {
    execution: Rc<Execution>,
}
impl Core {
    pub fn new(cpu: RawCore, memory: Memory) -> Result<Self, String> {
        if cpu.model() != Model::ArmV8A || !cpu.features().translation_filter {
            return Err("counted A-profile requires the AArch64 translation frontend".into());
        }
        for (encoding, mask) in [(0xf088, 1 << 10), (0xe088, 1 << 31)] {
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
            failure: RefCell::new(None),
            observers: RefCell::new(BTreeMap::new()),
            index: RefCell::new(observers::Index::default()),
            next: Cell::new(1),
            devices: RefCell::new(BTreeMap::new()),
            stores: RefCell::new(vec![]),
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
        }
        Ok(Self { execution: e })
    }
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
        let framework = arm64::framework_instruction(word);
        let boundary = word & 0xfff00000 == 0xd5100000
            || word & 0xfffff0df == 0xd50340df
            || word & 0xfffffc1f == 0xd69f0000
            || word & 0xff000000 == 0xd4000000;
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
        if b.bytes == 0 || b.bytes % 4 != 0 {
            return Err("invalid AArch64 block span".into());
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
            if arm64::framework_instruction(word) {
                self.count.set(self.count.get() - 1);
                self.exit.set(Some(word));
                self.cpu.stop()?;
            }
        }
        self.at_instruction.set(false);
        self.reset();
        Ok(())
    }
    fn deliver_interrupt(&self) -> Result<bool, String> {
        arm64::enter_interrupt(&*self.cpu, self.irq.get(), self.fiq.get())
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
