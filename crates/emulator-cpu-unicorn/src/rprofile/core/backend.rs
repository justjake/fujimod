use super::*;

impl Cpu for Core {
    fn set_loop_mode(&self, mode: fujimod_emulator_cpu::loops::LoopMode) -> Result<(), String> {
        let e = &self.execution;
        if e.active.get() {
            return Err("loop mode requires a stopped processor".into());
        }
        e.cpu.invalidate_code(None)?;
        e.loops.mode.set(mode);
        e.loops.bypass.set(None);
        Ok(())
    }
    fn set_loop_window(&self, stable: bool) -> Result<(), String> {
        let e = &self.execution;
        if e.active.get() {
            return Err("loop window requires a stopped processor".into());
        }
        e.loops.window.set(stable);
        Ok(())
    }
    fn identity(&self) -> Identity {
        self.execution.cpu.identity()
    }
    fn set_execution_policy(&self, policy: ExecutionPolicy) -> Result<(), String> {
        let e = &self.execution;
        if e.memory.run_reserved(e.id.get()) {
            return Err("execution policy requires a stopped processor".into());
        }
        e.memory.require_stopped()?;
        if e.active.get() {
            return Err("execution policy requires a stopped processor".into());
        }
        e.set_policy(policy)?;
        e.clock_overshoot.set(0);
        unsafe { (*e.record.as_ptr()).enabled = e.accounting_mode() };
        Ok(())
    }
    fn checkpoint_backings(&self) -> Result<Vec<(NonNull<u8>, usize)>, String> {
        Ok(self
            .execution
            .maps
            .borrow()
            .iter()
            .map(|m| {
                (
                    NonNull::new(m.pointer as *mut u8).expect("mapped backing"),
                    m.bytes,
                )
            })
            .collect())
    }
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        self.execution.fingerprint()
    }
    fn model(&self) -> Model {
        Model::CortexR5F
    }
    fn exclusive_domain(&self) -> Option<fujimod_emulator_cpu::exclusive::Shared> {
        Some(self.execution.memory.monitors.clone())
    }
    fn features(&self) -> Features {
        Features {
            guest_clock: true,
            instruction_limit: true,
            instruction_progress: true,
            architectural_exits: true,
            tighten_instruction_limit: true,
            backing_observation: self.execution.cpu.features().backing_observation,
            watched_stores: true,
            store_pc_control: true,
            ..Features::default()
        }
    }
    fn read_register(&self, r: Register, bytes: &mut [u8]) -> Result<usize, String> {
        self.execution.cpu.read_register(r, bytes)
    }
    fn write_register(&self, r: Register, bytes: &[u8]) -> Result<(), String> {
        self.execution.cpu.write_register(r, bytes)
    }
    fn coprocessor(&self, r: arm::Coprocessor, value: Option<u64>) -> Result<u64, String> {
        if r.cp == 15 && r.opc1 == 0 && r.crn == 9 && r.crm == 1 {
            if r.wide || r.opc2 > 1 || r.secure > 1 {
                return Err("unmodeled R-profile TCM selector".into());
            }
            let e = &self.execution;
            if let Some(value) = value {
                if e.active.get() {
                    return Err("host TCM writes require a stopped processor".into());
                }
                e.apply_tcm(
                    r.opc2 as usize,
                    u32::try_from(value).map_err(|_| "TCM value exceeds 32 bits")?,
                )?;
                return Ok(0);
            }
            return Ok(e.tcm[r.opc2 as usize].register.get() as u64);
        }
        if pmu::selected(r) {
            if r.wide || r.crn != 9 || r.opc1 != 0 || r.secure > 1 {
                return Err("unmodeled R-profile PMU selector".into());
            }
            let value = value
                .map(|v| u32::try_from(v).map_err(|_| "R-profile PMU value exceeds 32 bits"))
                .transpose()?;
            return self
                .execution
                .pmu_access(r.crm, r.opc2, value)
                .map(u64::from);
        }
        self.execution.cpu.coprocessor(r, value)
    }
    fn set_guest_clock(&self, clock: Clock) -> Result<(), String> {
        let e = &self.execution;
        if e.active.get() {
            return Err("cannot replace an active R-profile guest clock".into());
        }
        clock.after(0)?;
        if let Some(old) = e.clock.get() {
            if old.ratio != clock.ratio {
                if clock.now < old.now {
                    return Err("R-profile guest clock may not move backward".into());
                }
            } else {
                let (numerator, denominator) = clock.ratio.parts();
                let old_position =
                    u128::from(old.now) * u128::from(numerator) + u128::from(old.phase);
                let new_position =
                    u128::from(clock.now) * u128::from(numerator) + u128::from(clock.phase);
                let backstep = old_position.saturating_sub(new_position);
                if backstep != 0
                    && (e.policy.get() != ExecutionPolicy::Interactive
                        || backstep > u128::from(e.clock_overshoot.get()) * u128::from(denominator))
                {
                    return Err("R-profile guest clock may not move backward".into());
                }
            }
        }
        e.clock_overshoot.set(0);
        e.clock.set(Some(clock));
        Ok(())
    }
    fn read_memory(&self, address: u64, bytes: &mut [u8]) -> Result<(), String> {
        self.execution.cpu.read_memory(address, bytes)
    }
    fn translate(&self, address: u64, access: Access) -> Result<u64, String> {
        self.execution.cpu.translate(address, access)
    }
    fn read_virtual(&self, address: u64, access: Access, bytes: &mut [u8]) -> Result<(), String> {
        self.execution.cpu.read_virtual(address, access, bytes)
    }
    fn write_memory(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
        self.execution.mark_direct_write(address, bytes.len())?;
        self.execution.cpu.write_memory(address, bytes)?;
        self.memory_written(address, bytes.len())
    }
    fn memory_written(&self, address: u64, bytes: usize) -> Result<(), String> {
        let e = &self.execution;
        e.mark_direct_write(address, bytes)?;
        if e.memory.written(e.id.get(), address, bytes)? {
            e.memory.prepare_external_write()?;
        }
        Ok(())
    }
    unsafe fn backing_written(
        &self,
        physical: u64,
        backing: NonNull<u8>,
        bytes: usize,
    ) -> Result<(), String> {
        self.execution
            .mark_backing_write(backing.as_ptr() as usize, bytes)?;
        let memory = &self.execution.memory;
        if memory.backing_written(physical, backing.as_ptr() as usize, bytes)? {
            memory.prepare_external_write()?;
        }
        Ok(())
    }
    fn memory_regions(&self) -> Result<Vec<MemoryRegion>, String> {
        self.execution.cpu.memory_regions()
    }
    unsafe fn map_backing(
        &self,
        address: u64,
        bytes: usize,
        permissions: u32,
        backing: NonNull<u8>,
    ) -> Result<(), String> {
        let e = &self.execution;
        e.map_external(Mapping {
            address,
            bytes,
            pointer: backing.as_ptr() as usize,
            permissions,
            physical: None,
        })
    }
    unsafe fn map_physical(
        &self,
        address: u64,
        physical: u64,
        bytes: usize,
        permissions: u32,
        backing: NonNull<u8>,
    ) -> Result<(), String> {
        self.execution.map_external(Mapping {
            address,
            bytes,
            permissions,
            pointer: backing.as_ptr() as usize,
            physical: Some(physical),
        })
    }
    unsafe fn replace_physical(
        &self,
        address: u64,
        physical: u64,
        bytes: usize,
        permissions: u32,
        backing: NonNull<u8>,
    ) -> Result<(), String> {
        self.execution.replace_external(Mapping {
            address,
            bytes,
            permissions,
            pointer: backing.as_ptr() as usize,
            physical: Some(physical),
        })
    }
    fn map_zeroed(&self, address: u64, bytes: usize, permissions: u32) -> Result<(), String> {
        self.execution.memory.require_stopped()?;
        let backing = Backing::aligned_zeros(bytes, 4096)?;
        let pointer = (backing.as_ptr() as usize + 4095) & !4095;
        // Keep the allocation even if a later map/watch operation fails.
        // An error must never leave a native mapping of a freed allocation.
        self.execution.owned.borrow_mut().push(backing);
        unsafe {
            self.map_backing(
                address,
                bytes,
                permissions,
                NonNull::new(pointer as *mut u8).unwrap(),
            )
        }
    }
    unsafe fn map_mmio(&self, address: u64, bytes: usize, callbacks: Mmio) -> Result<(), String> {
        let e = &self.execution;
        if e.overlaps_tcm(address, bytes) {
            return Err("MMIO overlaps processor-private TCM".into());
        }
        unsafe {
            e.cpu.map_mmio(address, bytes, callbacks)?;
        }
        if let Err(error) = e.device_hook(address, bytes) {
            e.cpu.unmap(address, bytes)?;
            return Err(error);
        }
        Ok(())
    }
    unsafe fn map_mmio_fast(
        &self,
        address: u64,
        bytes: usize,
        callbacks: Mmio,
        fast_reads: Vec<FastMmioRead>,
    ) -> Result<(), String> {
        let e = &self.execution;
        if e.overlaps_tcm(address, bytes) {
            return Err("MMIO overlaps processor-private TCM".into());
        }
        for read in &fast_reads {
            read.set_enabled(e.policy.get() == ExecutionPolicy::Interactive);
        }
        unsafe {
            e.cpu
                .map_mmio_fast(address, bytes, callbacks, fast_reads.clone())?;
        }
        if let Err(error) = e.device_hook(address, bytes) {
            e.cpu.unmap(address, bytes)?;
            return Err(error);
        }
        e.loop_reads
            .borrow_mut()
            .extend(fast_reads.iter().cloned().map(|read| (address, read)));
        e.fast_reads.borrow_mut().extend(fast_reads);
        Ok(())
    }
    fn unmap(&self, address: u64, bytes: usize) -> Result<(), String> {
        let e = &self.execution;
        if e.overlaps_tcm(address, bytes) {
            return Err("TCM mappings are owned by the processor".into());
        }
        e.cpu.unmap(address, bytes)?;
        e.memory.unmap(e.id.get(), address, bytes);
        e.remove_external(address, bytes);
        let end = address + bytes as u64;
        e.loop_reads.borrow_mut().retain(|(base, read)| {
            let p = base + read.register_offset;
            p + 4 <= address || p >= end
        });
        let affected: Vec<_> = e
            .mmio_hooks
            .borrow()
            .iter()
            .filter(|(start, (n, _))| **start < end && address < **start + *n as u64)
            .map(|(&start, &(n, hook))| (start, n, hook))
            .collect();
        for (start, n, hook) in affected {
            e.cpu.remove_hook(hook)?;
            e.mmio_hooks.borrow_mut().remove(&start);
            if start < address {
                e.device_hook(start, (address - start) as usize)?;
            }
            if start + n as u64 > end {
                e.device_hook(end, (start + n as u64 - end) as usize)?;
            }
        }
        Ok(())
    }
    fn protect(&self, address: u64, bytes: usize, permissions: u32) -> Result<(), String> {
        let e = &self.execution;
        if e.overlaps_tcm(address, bytes) {
            return Err("TCM mappings are owned by the processor".into());
        }
        e.cpu.protect(address, bytes, permissions)?;
        let changed: Vec<_> = e
            .maps
            .borrow()
            .iter()
            .filter_map(|m| m.clip(address, address + bytes as u64))
            .collect();
        e.remove_external(address, bytes);
        for mut map in changed {
            map.permissions = permissions;
            e.memory.declare(
                e.id.get(),
                map.address,
                map.pointer,
                map.bytes,
                map.physical,
            );
            e.maps.borrow_mut().push(map);
        }
        Ok(())
    }
    fn invalidate_code(&self, range: Option<(u64, u64)>) -> Result<(), String> {
        self.execution.invalidate_loop_failures();
        self.execution.cpu.invalidate_code(range)
    }
    fn run(&self, request: Run) -> Result<Exit, ExecutionError> {
        let e = &self.execution;
        let _guest_tick_scope = crate::rprofile::GuestTickScope::new();
        if e.active.get() {
            return Err("nested R-profile execution is refused".into());
        }
        let _owner_barrier_run = e.owner_barrier_run_guard();
        let _window = loops::Window(&e.loops.window);
        e.clock_overshoot.set(0);
        e.count.set(0);
        e.last_span.set(None);
        if request.until.is_some() || request.instructions.is_none() {
            e.loops.window.set(false);
        }
        if request.instructions == Some(0) {
            return Ok(Exit::Stopped);
        }
        if request.watchdog_us != 0 {
            return Err(
                "counted R-profile host watchdog is not implemented; use explicit cancellation"
                    .into(),
            );
        }
        let _run = e
            .memory
            .reserve_run(e.id.get(), e.policy.get(), e.parallel_tcg)?;
        e.memory.flush()?;
        e.cap.set(
            request
                .instructions
                .map_or(i64::MAX as u64 / 2, |n| n as u64),
        );
        e.reset_budget();
        e.restarted.set(None);
        e.hint.set(None);
        e.exception.set(None);
        e.user_stop.set(false);
        e.at_block.set(false);
        e.at_instruction.set(false);
        e.failure.borrow_mut().take();
        e.settled_head.set(e.accounting().counters().1);
        let mut begin = request.begin;
        if e.irq.get() {
            // Entry must use the requested architectural PC, including Thumb.
            e.set(arm::PC, begin as u32)?;
            if let Some(entry) = crate::rprofile::enter_irq(&*e.cpu)? {
                e.clear_exclusive()?;
                begin = entry.vector as u64;
                e.observe_exception(arm::Exception::Irq);
            }
        }
        let result = self.execute(request, begin);
        e.clock_overshoot
            .set(if e.policy.get() == ExecutionPolicy::Interactive {
                e.count.get().saturating_sub(e.cap.get())
            } else {
                0
            });
        if let Some(clock) = e.clock.get() {
            e.clock.set(Some(clock.after(e.count.get())?));
        }
        result
    }

    fn tighten_instruction_limit(&self, total: u64) -> Result<(), String> {
        self.execution.tighten(total)
    }
    fn instruction_progress(&self) -> Option<u64> {
        self.execution.progress()
    }
    fn stop(&self) -> Result<(), String> {
        self.execution.user_stop.set(true);
        self.execution.cpu.stop()
    }
    fn stop_before_instruction(&self) -> Result<(), String> {
        let e = &self.execution;
        if !e.at_instruction.get() || e.user_stop.get() {
            return Err("instruction interception requires a live, unconsumed entry".into());
        }
        if e.thumb.get() || e.policy.get() == ExecutionPolicy::Exact {
            e.count.set(
                e.count
                    .get()
                    .checked_sub(1)
                    .ok_or("missing observed entry")?,
            );
        }
        self.stop()
    }
    fn rprofile_identity(&self, value: Option<u32>) -> Result<u32, String> {
        let e = &self.execution;
        if let Some(value) = value {
            if e.active.get() {
                return Err("identity changes require a paused processor".into());
            }
            e.identity_input.set(value);
        }
        Ok(e.identity_input.get())
    }
    fn set_interrupt(&self, line: Interrupt, asserted: bool) -> Result<(), String> {
        let e = &self.execution;
        if line != Interrupt::Irq {
            return Err("R-profile FIQ entry is not implemented".into());
        }
        e.irq.set(asserted);
        if asserted && e.active.get() && e.scalar(arm::CPSR)? & 0x80 == 0 {
            e.tighten(
                e.progress()
                    .ok_or("missing R-profile progress at IRQ input")?,
            )?;
        }
        Ok(())
    }
    fn snapshot(&self, previous: Option<&dyn Snapshot>) -> Result<Box<dyn Snapshot>, String> {
        self.capture(previous)
    }
    fn restore(&self, snapshot: &dyn Snapshot) -> Result<(), String> {
        self.restore_captured(snapshot)
    }
    fn import_snapshot(&self, bytes: &[u8]) -> Result<Box<dyn Snapshot>, String> {
        self.import_captured(bytes)
    }
    unsafe fn install_hook(&self, hook: Hook) -> Result<HookId, String> {
        let e = &self.execution;
        if e.active.get() {
            return Err("cannot install a hook during R-profile execution".into());
        }
        let key = e.next_observer.get();
        let next = key
            .checked_add(1)
            .ok_or("R-profile hook identity overflow")?;
        let native = match hook {
            Hook::Instruction { range, .. } | Hook::Block { range, .. } => {
                self.retranslate(range)?;
                None
            }
            // Observe only after backend-owned architectural entry. The
            // callback cannot replace exception classification or entry.
            Hook::Exception { .. } => None,
            // Owner synchronization is raised explicitly by the R-profile
            // execution adapter at sensitive memory operations.
            Hook::Synchronization { .. } => None,
            Hook::CompletedStore { range, .. } => {
                // The permanent coherence hook already dispatches stores on
                // watched pages. Reuse it instead of creating native hooks
                // whose deferred deletion accumulates on parked processors.
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
                None
            }
            _ => Some(unsafe { e.cpu.install_hook(hook)? }),
        };
        e.observers
            .borrow_mut()
            .insert(key, Observer { hook, native });
        e.observer_index.borrow_mut().insert(key, hook);
        e.next_observer.set(next);
        Ok(HookId {
            owner: self.identity(),
            key,
        })
    }
    fn remove_hook(&self, hook: HookId) -> Result<(), String> {
        let e = &self.execution;
        if e.active.get() || hook.owner != self.identity() {
            return Err("invalid R-profile hook owner or execution state".into());
        }
        let observers = e.observers.borrow();
        let observer = observers.get(&hook.key).ok_or("unknown R-profile hook")?;
        match observer.hook {
            Hook::Instruction { range, .. } | Hook::Block { range, .. } => {
                self.retranslate(range)?;
            }
            Hook::Exception { .. } => (),
            _ => {
                if let Some(native) = observer.native {
                    e.cpu.remove_hook(native)?;
                }
            }
        }
        e.observer_index
            .borrow_mut()
            .remove(hook.key, observer.hook);
        drop(observers);
        e.observers.borrow_mut().remove(&hook.key);
        Ok(())
    }
    unsafe fn translation_hooks(&self, _: Option<TranslationHooks>) -> Result<(), String> {
        Err("R-profile translation hooks are owned by the backend".into())
    }
    fn retry_block(&self, _: u32) -> Result<(), String> {
        Err("R-profile block retries are owned by the backend".into())
    }
    fn request_restart(&self) -> Result<(), String> {
        Err("R-profile restarts are owned by the backend".into())
    }
    fn accounting(&self) -> Option<NonNull<BlockAccounting>> {
        None
    }
    unsafe fn observe_backings(&self, ranges: &[BackingObserver]) -> Result<(), String> {
        *self.execution.observed_backings.borrow_mut() = ranges.to_vec();
        self.execution.rearm_observers()
    }
    fn watch_stores(&self, address: u64, bytes: u64) -> Result<(), String> {
        self.execution.cpu.watch_stores(address, bytes)
    }
    fn hook_watched(&self, hook: HookId, watched: bool) -> Result<(), String> {
        let observers = self.execution.observers.borrow();
        let native = observers
            .get(&hook.key)
            .filter(|_| hook.owner == self.identity())
            .and_then(|o| o.native)
            .ok_or("R-profile watched hook is not a memory hook")?;
        self.execution.cpu.hook_watched(native, watched)
    }
    fn precise_store_pc(&self, on: bool) -> Result<(), String> {
        if !on {
            return Err("R-profile code coherence requires precise store PCs".into());
        }
        Ok(())
    }
    fn exit_counters(&self) -> Result<BTreeMap<&'static str, u64>, String> {
        let mut counters = self.execution.cpu.exit_counters()?;
        let l = &self.execution.loops;
        #[cfg(debug_assertions)]
        l.log_unsupported_models();
        counters.insert("loop_poll_instructions", l.poll_instructions.get());
        counters.insert(
            "loop_clock_poll_instructions",
            l.clock_poll_instructions.get(),
        );
        counters.insert("loop_counted_instructions", l.counted_instructions.get());
        counters.insert(
            "loop_accelerated_instructions",
            l.accelerated_instructions.get(),
        );
        counters.insert("loop_candidate_entries", l.candidate_entries.get());
        counters.insert("loop_proofs_attempted", l.proofs_attempted.get());
        counters.insert(
            "loop_proofs_failed_structural",
            l.proofs_failed_structural.get(),
        );
        counters.insert(
            "loop_proofs_failed_window_inactive",
            l.proofs_failed_window_inactive.get(),
        );
        counters.insert(
            "loop_proofs_failed_memory_flush_pending",
            l.proofs_failed_memory_flush_pending.get(),
        );
        counters.insert(
            "loop_proofs_failed_instruction_observer",
            l.proofs_failed_instruction_observer.get(),
        );
        counters.insert(
            "loop_proofs_failed_block_observer",
            l.proofs_failed_block_observer.get(),
        );
        counters.insert(
            "loop_proofs_failed_read_observer",
            l.proofs_failed_read_observer.get(),
        );
        counters.insert(
            "loop_proofs_failed_loop_bytes_unavailable",
            l.proofs_failed_loop_bytes_unavailable.get(),
        );
        counters.insert(
            "loop_proofs_failed_cpsr_e_or_j",
            l.proofs_failed_cpsr_e_or_j.get(),
        );
        counters.insert(
            "loop_proofs_failed_loop_condition_not_taken",
            l.proofs_failed_loop_condition_not_taken.get(),
        );
        counters.insert(
            "loop_proofs_failed_loop_read_unavailable",
            l.proofs_failed_loop_read_unavailable.get(),
        );
        counters.insert(
            "loop_proofs_failed_iteration_model_unsupported",
            l.proofs_failed_iteration_model_unsupported.get(),
        );
        counters.insert(
            "loop_proofs_failed_iteration_model_unsupported_pcs",
            l.unsupported_model_pc_count() as u64,
        );
        counters.insert(
            "loop_proofs_failed_counter_at_boundary",
            l.proofs_failed_counter_at_boundary.get(),
        );
        counters.insert(
            "loop_proofs_failed_budget_room",
            l.proofs_failed_budget_room.get(),
        );
        counters.insert(
            "code_watch_block_hooks_elided",
            self.execution.code_watch_block_hooks_elided.get(),
        );
        counters.insert(
            "block_hook_executions",
            self.execution.block_hook_executions.get(),
        );
        counters.insert(
            "block_hook_reason_block_observer",
            self.execution.block_hook_block_observer.get(),
        );
        counters.insert(
            "block_hook_reason_thumb",
            self.execution.block_hook_thumb.get(),
        );
        counters.insert(
            "block_hook_reason_budget_fallback_remaining_budget",
            self.execution
                .block_hook_budget_fallback_remaining_budget
                .get(),
        );
        counters.insert(
            "block_hook_reason_budget_fallback_accounting_ring",
            self.execution
                .block_hook_budget_fallback_accounting_ring
                .get(),
        );
        counters.insert(
            "block_hook_reason_budget_fallback_guest_clock",
            self.execution.block_hook_budget_fallback_guest_clock.get(),
        );
        counters.insert(
            "block_hook_reason_budget_fallback_other",
            self.execution.block_hook_budget_fallback_other.get(),
        );
        counters.insert(
            "block_hook_reason_other",
            self.execution.block_hook_other.get(),
        );
        counters.insert(
            "synchronization_requested",
            self.execution.synchronization_requested.get(),
        );
        counters.insert(
            "synchronization_elided",
            self.execution.synchronization_elided.get(),
        );
        counters.insert(
            "synchronization_barriers",
            self.execution.synchronization_barriers.get(),
        );
        Ok(counters)
    }
}

/// Above this many pages one full flush is cheaper than page invalidations.
const RETRANSLATE_PAGES: usize = 4096;

impl Core {
    /// Discard the translations a code observer over `range` changes. The
    /// translator consults observers per instruction address, so a bounded
    /// observer changes only code translated from its own pages; flushing the
    /// whole cache for it re-translates all firmware on every watch.
    fn retranslate(&self, range: HookRange) -> Result<(), String> {
        let e = &self.execution;
        e.invalidate_loop_failures();
        let pages = match range {
            HookRange::Inclusive { begin, end } => {
                e.memory
                    .translated_pages(e.id.get(), begin, end.saturating_add(1))
            }
            HookRange::All => None,
        };
        match pages {
            Some(pages) if pages.len() <= RETRANSLATE_PAGES => {
                for page in pages {
                    e.cpu.invalidate_code(Some((page, page + 4096)))?;
                }
                Ok(())
            }
            _ => e.cpu.invalidate_code(None),
        }
    }
}
