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
    reservation: Option<exclusive::Saved>,
}
impl Snapshot for Saved {
    fn owner(&self) -> Identity {
        self.raw.owner()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn export(&self) -> Result<Vec<u8>, String> {
        let raw = self.raw.export()?;
        if let Some(reservation) = self.reservation {
            crate::durable::encode(&(raw, self.irq, self.fiq, reservation.to_bytes().to_vec()))
        } else {
            crate::durable::encode(&(raw, self.irq, self.fiq))
        }
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
            guest_clock: !self.execution.a32,
            instruction_progress: true,
            tighten_instruction_limit: true,
            backing_observation: true,
            watched_stores: true,
            store_pc_control: true,
            ..Features::default()
        }
    }
    fn exclusive_domain(&self) -> Option<exclusive::Shared> {
        self.execution
            .a64_monitor
            .get()
            .map(|_| self.execution.memory.monitors.clone())
    }
    fn reports_exclusive_writes(&self) -> bool {
        // The engine watches reservation pages installed by AArch64 loads.
        // A companion can also reserve RAM without installing these watches.
        false
    }
    fn set_execution_policy(&self, policy: ExecutionPolicy) -> Result<(), String> {
        let e = &self.execution;
        e.memory.require_stopped()?;
        if e.policy.get() != policy {
            e.cpu.set_execution_policy(policy)?;
            if !e.a32 {
                if e.pause_requested.is_some() {
                    e.cpu
                        .set_parallel_cpus(policy == ExecutionPolicy::Interactive)?;
                }
                e.cpu
                    .set_entry_exits(policy == ExecutionPolicy::Interactive)?;
            }
            e.cpu.invalidate_code(None)?;
            e.policy.set(policy);
        }
        Ok(())
    }
    fn set_loop_mode(&self, mode: loops::LoopMode) -> Result<(), String> {
        let e = &self.execution;
        e.memory.require_stopped()?;
        if e.loop_mode.get() != mode {
            e.cpu.invalidate_code(None)?;
            e.loop_mode.set(mode);
        }
        Ok(())
    }
    fn set_loop_window(&self, stable: bool) -> Result<(), String> {
        let e = &self.execution;
        if e.active.get() {
            return Err("loop window requires a stopped processor".into());
        }
        e.loop_window.set(stable);
        Ok(())
    }
    fn set_guest_clock(&self, clock: Clock) -> Result<(), String> {
        let e = &self.execution;
        if e.active.get() {
            return Err("guest clock anchor requires a stopped processor".into());
        }
        clock.after(0)?;
        e.clock.set(Some(clock));
        Ok(())
    }
    fn exception_entries(&self) -> Option<armv7a::Entries> {
        self.execution.a32.then(|| self.execution.entries.get())
    }
    fn set_armv7a_affinity(&self, mpidr: u32) -> Result<(), String> {
        let supported = self.install_mpidr(mpidr)? == u64::from(mpidr);
        self.execution.board_mpidr.set(supported);
        Ok(())
    }
    fn set_armv7a_owner_exclusives(&self, on: bool) -> Result<(), String> {
        self.set_owner_exclusives(on)
    }
    fn select_armv7a_affinity_policy(&self, policy: ExecutionPolicy) -> Result<(), String> {
        let e = &self.execution;
        if e.board_mpidr.get() {
            // Interactive runs can switch cores at an instruction exit.
            // Preserve the existing MPIDR scheduling boundary there.
            let native = policy == ExecutionPolicy::Exact;
            if e.owner_mpidr.get() != native {
                e.cpu.invalidate_code(None)?;
                e.owner_mpidr.set(native);
            }
        }
        Ok(())
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
    unsafe fn replace_physical(
        &self,
        address: u64,
        physical: u64,
        bytes: usize,
        permissions: u32,
        backing: NonNull<u8>,
    ) -> Result<(), String> {
        let e = &self.execution;
        if e.active.get() {
            return Err("physical replacement requires a stopped A-profile processor".into());
        }
        e.memory
            .validate_mapping(backing.as_ptr() as usize, bytes, Some(physical))?;
        unsafe {
            e.cpu
                .replace_physical(address, physical, bytes, permissions, backing)?;
        }
        e.memory.unmap(e.id.get(), address, bytes);
        e.memory.undeclare(e.id.get(), address, bytes);
        let pointer = backing.as_ptr() as usize;
        e.memory
            .declare(e.id.get(), address, pointer, bytes, Some(physical));
        e.memory
            .map(e.id.get(), address, pointer, bytes, Some(physical));
        e.memory.apply_watches()?;
        e.cpu.invalidate_code(None)
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
        reads: Vec<FastMmioRead>,
    ) -> Result<(), String> {
        let e = &self.execution;
        if e.a32 {
            return unsafe { self.map_mmio(address, bytes, callbacks) };
        }
        unsafe { e.cpu.map_mmio_fast(address, bytes, callbacks, reads)? };
        if let Err(error) = e.device(address, bytes) {
            e.cpu.unmap(address, bytes)?;
            return Err(error);
        }
        Ok(())
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
        let _guest_tick = crate::GuestClockTickScope::new(e.policy.get());
        // `FUJIMOD_A64_LOOP_PROOFS=0` is a host-only measurement switch: it
        // runs the same code with the prover off, so a benchmark can compare
        // the two without a second build.
        // Read once per process: runs are too frequent for a getenv each.
        static PROOFS_ALLOWED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let proofs_allowed = *PROOFS_ALLOWED
            .get_or_init(|| std::env::var("FUJIMOD_A64_LOOP_PROOFS").as_deref() != Ok("0"));
        e.prove_loops.set(
            e.loop_window.replace(false)
                && proofs_allowed
                && e.can_prove_loops()
                && request.until.is_none()
                && request.instructions.is_some(),
        );
        e.count.set(0);
        unsafe { (*e.record.as_ptr()).enabled = if e.coarse() { 2 } else { 1 } };
        e.reset();
        e.update_guest_tick()?;
        e.cap.set(
            request
                .instructions
                .map_or(i64::MAX as u64 / 2, |n| n as u64),
        );
        e.stopped.set(false);
        e.exit.set(None);
        e.exception.set(None);
        e.restart.set(None);
        e.failure.borrow_mut().take();
        if request.instructions == Some(0) {
            return Ok(Exit::Stopped);
        }
        if request.watchdog_us != 0 {
            return Err("counted A-profile runs require exact limits or explicit owner stops, without a host watchdog".into());
        }
        let _reservation =
            e.memory
                .reserve_run(e.id.get(), e.policy.get(), e.pause_requested.is_some())?;
        if e.a64_monitor.get().is_some() && e.pause_requested.is_some() {
            e.memory.apply_watches()?;
        }
        if e.memory.flush_pending() {
            e.memory.flush()?;
        }
        if e.el3_transitions.get() {
            e.run_at_el3.set(e.at_el3()?);
        }
        let mut begin = request.begin;
        loop {
            // The engine installs the start PC itself. Seed it early only
            // when an interrupt might read it before any guest execution.
            if e.irq.get() || e.fiq.get() {
                e.cpu.write_register(Register::Pc, &begin.to_le_bytes())?;
                if e.deliver_interrupt()? {
                    return Ok(Exit::Stopped);
                }
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
            if let Some(exception) = e.exception.take() {
                let fetch = exception == arm::Exception::PrefetchAbort;
                armv7a::abort::enter(&*e.cpu, fetch)?;
                e.count_entry(|n| {
                    if fetch {
                        n.prefetch_abort += 1
                    } else {
                        n.data_abort += 1
                    }
                });
                return Ok(Exit::Stopped);
            }
            let exit = result?;
            if let Some(word) = e.exit.take() {
                return Ok(Exit::BeforeInstruction(word));
            }
            let flush = e.memory.flush_pending();
            if flush {
                e.memory.flush()?;
            }
            // A pause requested while this run was executing has to end it.
            // The engine's block hook honors the pause before the owner's
            // block callback runs, so a restart requested by a store is left
            // pending when the pause stops the guest. Retrying in place would
            // re-enter the engine at the same PC and be stopped before that
            // block ever runs, forever. Finish the run instead, leaving the
            // stopped state and the pending restart exactly as a pause at a
            // block entry leaves them.
            if e.pause_requested
                .as_ref()
                .is_some_and(|pause| pause.load(std::sync::atomic::Ordering::Acquire) != 0)
            {
                e.stopped.set(true);
                return Ok(exit);
            }
            if !e.stopped.get()
                && (flush || e.restart.get().is_some())
                && e.count.get() < e.cap.get()
                // Accepted shared translation maintenance must reach the
                // owner before any internal retry.
                && !e.cpu.translation_maintenance_status().is_ok_and(|s| s.pending != 0 || s.poison != 0)
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
    fn stop_on_el3_transitions(&self) -> Result<(), String> {
        if self.execution.a32 {
            return Err("AArch32 A-profile processors have no EL3 transitions to stop on".into());
        }
        self.execution.el3_transitions.set(true);
        Ok(())
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
    /// Every mapped backing is external: the processor snapshot holds
    /// registers only, and the board refuses any backing it does not own.
    fn checkpoint_backings(&self) -> Result<Vec<(NonNull<u8>, usize)>, String> {
        Ok(self.execution.memory.backings(self.execution.id.get()))
    }
    fn import_snapshot(&self, bytes: &[u8]) -> Result<Box<dyn Snapshot>, String> {
        let (raw, irq, fiq, reservation) =
            match crate::durable::decode::<(Vec<u8>, bool, bool, Vec<u8>)>(bytes) {
                Ok((raw, irq, fiq, saved)) => {
                    (raw, irq, fiq, Some(exclusive::Saved::from_bytes(&saved)?))
                }
                Err(_) => {
                    let (raw, irq, fiq) = crate::durable::decode(bytes)?;
                    (raw, irq, fiq, None)
                }
            };
        Ok(Box::new(Saved {
            raw: self.execution.cpu.import_snapshot(&raw)?,
            irq,
            fiq,
            reservation,
        }))
    }
    fn snapshot(&self, previous: Option<&dyn Snapshot>) -> Result<Box<dyn Snapshot>, String> {
        let e = &self.execution;
        if e.active.get() {
            return Err("snapshot requires a stopped A-profile processor".into());
        }
        let previous = previous
            .and_then(|s| s.as_any().downcast_ref::<Saved>())
            .map(|s| &*s.raw);
        let reservation = e
            .a64_monitor
            .get()
            .map(|owner| {
                e.memory
                    .monitors
                    .lock()
                    .map(|monitor| monitor.save(owner))
                    .map_err(|_| "exclusive monitors poisoned")
            })
            .transpose()?;
        Ok(Box::new(Saved {
            raw: e.cpu.snapshot(previous)?,
            irq: e.irq.get(),
            fiq: e.fiq.get(),
            reservation,
        }))
    }
    fn restore(&self, snapshot: &dyn Snapshot) -> Result<(), String> {
        let e = &self.execution;
        let s = snapshot
            .as_any()
            .downcast_ref::<Saved>()
            .ok_or("snapshot is not A-profile")?;
        e.cpu.restore(&*s.raw)?;
        if let Some(owner) = e.a64_monitor.get() {
            let mut monitors = e
                .memory
                .monitors
                .lock()
                .map_err(|_| "exclusive monitors poisoned")?;
            if let Some(saved) = s.reservation {
                monitors.restore(owner, saved);
            } else {
                monitors.clear(owner);
            }
        }
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
            Hook::Instruction {
                range: HookRange::Inclusive { begin, end },
                ..
            } if end >= begin
                && end - begin < 4096
                && e.cpu.translate(begin, Access::Fetch).is_ok()
                && e.cpu.translate(end, Access::Fetch).is_ok() =>
            {
                // Only translations covering the observed instructions need
                // translating again, and only where the engine can name them.
                e.cpu.invalidate_code(Some((begin, end + 4)))?;
                None
            }
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
            // Owner aborts: the abort settles first, then its observers run.
            Hook::Exception { .. } if e.owner_aborts.get() => None,
            Hook::Synchronization { .. } if e.pause_requested.is_some() => None,
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
        let mut counters = self.execution.cpu.exit_counters()?;
        let p = &self.execution.proofs;
        p.report();
        if !self.execution.a32 {
            counters.insert("loop_candidate_entries", p.candidate_entries.get());
            counters.insert("loop_proofs_attempted", p.proofs_attempted.get());
            counters.insert("loop_poll_instructions", p.poll_instructions.get());
            counters.insert("loop_counted_instructions", p.counted_instructions.get());
            counters.insert(
                "loop_accelerated_instructions",
                p.accelerated_instructions.get(),
            );
            counters.insert("loop_proofs_failed_structural", p.failed_structural.get());
            counters.insert(
                "loop_proofs_failed_window_inactive",
                p.failed_window_inactive.get(),
            );
            counters.insert("loop_proofs_failed_observer", p.failed_observer.get());
            counters.insert("loop_proofs_failed_cache", p.failed_cache.get());
            counters.insert(
                "loop_proofs_failed_loop_bytes_unavailable",
                p.failed_read_unavailable.get(),
            );
            counters.insert(
                "loop_proofs_failed_loop_condition_not_taken",
                p.failed_leaving.get(),
            );
            counters.insert(
                "loop_proofs_failed_iteration_model_unsupported",
                p.failed_model.get(),
            );
            counters.insert("loop_proofs_failed_budget_room", p.failed_budget.get());
        }
        Ok(counters)
    }
}
