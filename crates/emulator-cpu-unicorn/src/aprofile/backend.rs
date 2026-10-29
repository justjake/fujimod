use super::*;
use fujimod_emulator_cpu::memory::Backing;
use std::any::Any;

macro_rules! forward {
    ($(fn $name:ident(&self $(,$arg:ident:$ty:ty)*) -> $result:ty; )*) => {
        $(fn $name(&self,$($arg:$ty),*) -> $result {self.execution.cpu.$name($($arg),*)})*
    }
}
struct Saved {
    raw: Box<dyn Snapshot>,
    irq: bool,
    fiq: bool,
}
impl Snapshot for Saved {
    fn owner(&self) -> Identity {
        self.raw.owner()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl Cpu for Core {
    forward! {
        fn identity(&self)->Identity;
        fn model(&self)->Model;
        fn read_register(&self,r:Register,out:&mut [u8])->Result<usize,String>;
        fn write_register(&self,r:Register,bytes:&[u8])->Result<(),String>;
        fn coprocessor(&self,r:arm::Coprocessor,value:Option<u64>)->Result<u64,String>;
        fn read_memory(&self,address:u64,bytes:&mut [u8])->Result<(),String>;
        fn translate(&self,address:u64,access:Access)->Result<u64,String>;
        fn read_virtual(&self,address:u64,access:Access,bytes:&mut [u8])->Result<(),String>;
        fn memory_regions(&self)->Result<Vec<MemoryRegion>,String>;
        fn protect(&self,address:u64,bytes:usize,permissions:u32)->Result<(),String>;
    }
    fn features(&self) -> Features {
        Features {
            architectural_exits: true,
            instruction_limit: true,
            instruction_progress: true,
            tighten_instruction_limit: true,
            backing_observation: true,
            watched_stores: true,
            store_pc_control: true,
            ..Features::default()
        }
    }
    fn set_execution_policy(&self, policy: ExecutionPolicy) -> Result<(), String> {
        self.execution.cpu.set_execution_policy(policy)
    }
    fn write_memory(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
        let e = &self.execution;
        e.memory.prepare_external_write()?;
        e.cpu.write_memory(address, bytes)?;
        e.memory.written(e.id.get(), address, bytes.len())?;
        e.memory.flush()
    }
    fn memory_written(&self, address: u64, bytes: usize) -> Result<(), String> {
        let e = &self.execution;
        e.memory.written(e.id.get(), address, bytes)?;
        e.memory.prepare_external_write()
    }
    unsafe fn backing_written(
        &self,
        physical: u64,
        pointer: NonNull<u8>,
        bytes: usize,
    ) -> Result<(), String> {
        let e = &self.execution;
        e.memory
            .backing_written(physical, pointer.as_ptr() as usize, bytes)?;
        e.memory.prepare_external_write()
    }
    unsafe fn map_backing(
        &self,
        address: u64,
        bytes: usize,
        permissions: u32,
        backing: NonNull<u8>,
    ) -> Result<(), String> {
        unsafe { self.map_physical(address, address, bytes, permissions, backing) }
    }
    unsafe fn map_physical(
        &self,
        address: u64,
        physical: u64,
        bytes: usize,
        permissions: u32,
        backing: NonNull<u8>,
    ) -> Result<(), String> {
        let e = &self.execution;
        e.memory
            .validate_mapping(backing.as_ptr() as usize, bytes, Some(physical))?;
        unsafe {
            e.cpu
                .map_physical(address, physical, bytes, permissions, backing)?;
        }
        e.memory.declare(
            e.id.get(),
            address,
            backing.as_ptr() as usize,
            bytes,
            Some(physical),
        );
        e.memory.map(
            e.id.get(),
            address,
            backing.as_ptr() as usize,
            bytes,
            Some(physical),
        );
        e.memory.apply_watches()
    }
    fn map_zeroed(&self, address: u64, bytes: usize, permissions: u32) -> Result<(), String> {
        let b = Backing::aligned_zeros(bytes, 0x1000)?;
        unsafe {
            self.map_backing(
                address,
                bytes,
                permissions,
                NonNull::new(b.as_ptr()).ok_or("null A-profile backing")?,
            )?;
        }
        self.execution.owned.borrow_mut().push(b);
        Ok(())
    }
    unsafe fn map_mmio(&self, address: u64, bytes: usize, callbacks: Mmio) -> Result<(), String> {
        let e = &self.execution;
        unsafe {
            e.cpu.map_mmio(address, bytes, callbacks)?;
        }
        e.device(address, bytes)
    }
    unsafe fn map_mmio_fast(
        &self,
        address: u64,
        bytes: usize,
        callbacks: Mmio,
        _reads: Vec<FastMmioRead>,
    ) -> Result<(), String> {
        // Exact runs dispatch through the owner at every device access.
        unsafe { self.map_mmio(address, bytes, callbacks) }
    }
    fn unmap(&self, address: u64, bytes: usize) -> Result<(), String> {
        let e = &self.execution;
        e.cpu.unmap(address, bytes)?;
        e.memory.unmap(e.id.get(), address, bytes);
        e.memory.undeclare(e.id.get(), address, bytes);
        let end = address + bytes as u64;
        let affected = e
            .devices
            .borrow()
            .iter()
            .filter(|(base, (size, _))| **base < end && address < **base + *size as u64)
            .map(|(&base, &(size, hook))| (base, size, hook))
            .collect::<Vec<_>>();
        for (base, size, hook) in affected {
            e.cpu.remove_hook(hook)?;
            e.devices.borrow_mut().remove(&base);
            if base < address {
                e.device(base, (address - base) as usize)?;
            }
            if base + size as u64 > end {
                e.device(end, (base + size as u64 - end) as usize)?;
            }
        }
        Ok(())
    }
    fn invalidate_code(&self, range: Option<(u64, u64)>) -> Result<(), String> {
        self.execution.cpu.invalidate_code(range)
    }
    fn run(&self, request: Run) -> Result<Exit, ExecutionError> {
        let e = &self.execution;
        if e.active.get() {
            return Err("nested A-profile execution is refused".into());
        }
        e.count.set(0);
        e.reset();
        e.cap.set(
            request
                .instructions
                .map_or(i64::MAX as u64 / 2, |n| n as u64),
        );
        e.stopped.set(false);
        e.exit.set(None);
        e.restart.set(None);
        e.failure.borrow_mut().take();
        if request.instructions == Some(0) {
            return Ok(Exit::Stopped);
        }
        if request.watchdog_us != 0 {
            return Err("counted A-profile runs require exact limits or explicit owner stops, without a host watchdog".into());
        }
        let _reservation = e
            .memory
            .reserve_run(e.id.get(), ExecutionPolicy::Exact, false)?;
        e.memory.flush()?;
        let mut begin = request.begin;
        loop {
            e.cpu.write_register(Register::Pc, &begin.to_le_bytes())?;
            if e.deliver_interrupt()? {
                return Ok(Exit::Stopped);
            }
            e.memory.start(e.id.get())?;
            e.active.set(true);
            let result = e.cpu.run(Run {
                begin,
                instructions: None,
                ..request
            });
            let settlement = if result.is_err() {
                e.settle(e.tail(e.scalar(Register::Pc)?, true), true)
            } else {
                e.settle(0, true)
            };
            e.active.set(false);
            e.memory.end();
            settlement?;
            if let Some(error) = e.failure.borrow_mut().take() {
                return Err(error.into());
            }
            if result.is_ok() {
                e.publish()?;
            } else {
                e.stores.borrow_mut().clear();
            }
            let exit = result?;
            if let Some(word) = e.exit.take() {
                return Ok(Exit::BeforeInstruction(word));
            }
            let flush = e.memory.flush_pending();
            if flush {
                e.memory.flush()?;
            }
            if !e.stopped.get()
                && (flush || e.restart.get().is_some())
                && e.count.get() < e.cap.get()
            {
                begin = e.restart.get().unwrap_or(e.scalar(Register::Pc)?);
                continue;
            }
            return Ok(exit);
        }
    }
    fn instruction_progress(&self) -> Option<u64> {
        self.execution.progress()
    }
    fn tighten_instruction_limit(&self, total: u64) -> Result<(), String> {
        self.execution.tighten(total)
    }
    fn stop(&self) -> Result<(), String> {
        let e = &self.execution;
        if e.active.get() && !e.at_instruction.get() {
            e.settle(e.tail(e.scalar(Register::Pc)?, !e.at_block.get()), true)?;
        }
        e.stopped.set(true);
        e.reset();
        e.cpu.stop()
    }
    fn stop_before_instruction(&self) -> Result<(), String> {
        let e = &self.execution;
        if !e.active.get() || !e.at_instruction.get() || e.stopped.get() {
            return Err("instruction interception requires a live entry".into());
        }
        e.count.set(
            e.count
                .get()
                .checked_sub(1)
                .ok_or("missing A-profile entry")?,
        );
        self.stop()
    }
    fn set_interrupt(&self, line: Interrupt, asserted: bool) -> Result<(), String> {
        let e = &self.execution;
        let pin = match line {
            Interrupt::Irq => &e.irq,
            Interrupt::Fiq => &e.fiq,
        };
        if pin.replace(asserted) != asserted && e.active.get() {
            e.prepare_code_write()?;
        }
        Ok(())
    }
    fn snapshot(&self, previous: Option<&dyn Snapshot>) -> Result<Box<dyn Snapshot>, String> {
        let e = &self.execution;
        if e.active.get() {
            return Err("snapshot requires a stopped A-profile processor".into());
        }
        let previous = previous
            .and_then(|s| s.as_any().downcast_ref::<Saved>())
            .map(|s| &*s.raw);
        Ok(Box::new(Saved {
            raw: e.cpu.snapshot(previous)?,
            irq: e.irq.get(),
            fiq: e.fiq.get(),
        }))
    }
    fn restore(&self, snapshot: &dyn Snapshot) -> Result<(), String> {
        let e = &self.execution;
        let s = snapshot
            .as_any()
            .downcast_ref::<Saved>()
            .ok_or("snapshot is not A-profile")?;
        e.cpu.restore(&*s.raw)?;
        e.irq.set(s.irq);
        e.fiq.set(s.fiq);
        e.count.set(0);
        e.reset();
        Ok(())
    }
    unsafe fn install_hook(&self, hook: Hook) -> Result<HookId, String> {
        let e = &self.execution;
        if e.active.get() {
            return Err("hook installation requires a stopped processor".into());
        }
        let key = e.next.get();
        e.next.set(
            key.checked_add(1)
                .ok_or("A-profile hook identity overflow")?,
        );
        let native = match hook {
            Hook::Instruction { .. } | Hook::Block { .. } => {
                e.cpu.invalidate_code(None)?;
                None
            }
            Hook::CompletedStore { range, .. } => {
                let (begin, end) = match range {
                    HookRange::All => (0, u32::MAX as u64),
                    HookRange::Inclusive { begin, end }
                        if begin <= end && end <= u32::MAX as u64 =>
                    {
                        (begin, end)
                    }
                    _ => return Err("invalid completed-store observer range".into()),
                };
                e.cpu.watch_stores(begin, end - begin + 1)?;
                e.cpu.invalidate_code(None)?;
                None
            }
            _ => Some(unsafe { e.cpu.install_hook(hook)? }),
        };
        e.index.borrow_mut().insert(key, hook);
        e.observers
            .borrow_mut()
            .insert(key, Observer { hook, native });
        Ok(HookId {
            owner: self.identity(),
            key,
        })
    }
    fn remove_hook(&self, hook: HookId) -> Result<(), String> {
        let e = &self.execution;
        if e.active.get() || hook.owner != self.identity() {
            return Err("invalid A-profile hook owner or execution state".into());
        }
        let observer = e
            .observers
            .borrow_mut()
            .remove(&hook.key)
            .ok_or("unknown A-profile hook")?;
        if let Some(native) = observer.native {
            e.cpu.remove_hook(native)?;
        } else {
            e.cpu.invalidate_code(None)?;
        }
        e.index.borrow_mut().remove(hook.key, observer.hook);
        Ok(())
    }
    unsafe fn translation_hooks(&self, _hooks: Option<TranslationHooks>) -> Result<(), String> {
        Err("A-profile adapter owns translation policy".into())
    }
    fn retry_block(&self, _n: u32) -> Result<(), String> {
        Err("A-profile adapter owns block admission".into())
    }
    fn request_restart(&self) -> Result<(), String> {
        self.execution.cpu.request_restart()
    }
    fn accounting(&self) -> Option<NonNull<BlockAccounting>> {
        None
    }
    unsafe fn observe_backings(&self, ranges: &[BackingObserver]) -> Result<(), String> {
        unsafe { self.execution.cpu.observe_backings(ranges) }
    }
    fn watch_stores(&self, address: u64, bytes: u64) -> Result<(), String> {
        self.execution.cpu.watch_stores(address, bytes)
    }
    fn hook_watched(&self, hook: HookId, watched: bool) -> Result<(), String> {
        let observers = self.execution.observers.borrow();
        let native = observers
            .get(&hook.key)
            .and_then(|o| o.native)
            .ok_or("hook has no native watch")?;
        self.execution.cpu.hook_watched(native, watched)
    }
    fn precise_store_pc(&self, on: bool) -> Result<(), String> {
        self.execution.cpu.precise_store_pc(on)
    }
    fn exit_counters(&self) -> Result<BTreeMap<&'static str, u64>, String> {
        self.execution.cpu.exit_counters()
    }
}
