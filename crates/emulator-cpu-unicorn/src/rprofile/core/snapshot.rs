//! Exact stopped-processor context, including the execution owner's PMU,
//! rational clock phase, private TCM bytes and block-accounting continuation.
//! Ordinary physical RAM and shared monitor rollback belong to the board.
use super::*;
use fujimod_emulator_cpu::memory::pages::Image;
use sha2::{Digest, Sha256};
use std::any::Any;
use std::sync::atomic::Ordering;
mod portable;

struct Saved {
    context: Box<dyn Snapshot>,
    identity_input: u32,
    clock: Option<Clock>,
    clock_overshoot: u64,
    policy: ExecutionPolicy,
    pmu: pmu::Counter,
    tcm: [(u32, Image); 2],
    maps: Vec<Mapping>,
    mmio: Vec<(u64, usize)>,
    reservation: fujimod_emulator_cpu::exclusive::Saved,
    ring: [u64; 64],
    budget: i64,
    head: u64,
    count: u64,
    cap: u64,
    synced: i64,
    settled_head: u64,
    thumb: bool,
    restarted: Option<u64>,
    user_stop: bool,
    irq: bool,
    hint: Option<Hint>,
    exception: Option<arm::Exception>,
    failure: Option<String>,
}
impl Snapshot for Saved {
    fn owner(&self) -> Identity {
        self.context.owner()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn export(&self) -> Result<Vec<u8>, String> {
        portable::export(self)
    }
}
impl Execution {
    pub(super) fn mark_backing_write(&self, pointer: usize, bytes: usize) -> Result<(), String> {
        let end = pointer.checked_add(bytes).ok_or("backing write overflow")?;
        for observer in self.observed_backings.borrow().iter() {
            let base = observer.backing.as_ptr() as usize;
            let start = base.max(pointer);
            let stop = (base + observer.bytes).min(end);
            if start < stop {
                let first = (start - base) >> observer.page_shift;
                let last = (stop - base - 1) >> observer.page_shift;
                for page in first..=last {
                    unsafe {
                        (*observer.bitmap.as_ptr().add(page / 64))
                            .fetch_or(1 << (page % 64), Ordering::Relaxed);
                    }
                }
            }
        }
        for bank in &self.tcm {
            let base = bank.memory.as_ptr() as usize;
            let start = base.max(pointer);
            let stop = (base + bank.memory.len()).min(end);
            if start < stop {
                bank.tracker.mark(start - base, stop - start)?;
            }
        }
        Ok(())
    }
    pub(super) fn mark_direct_write(&self, address: u64, bytes: usize) -> Result<(), String> {
        let end = address
            .checked_add(bytes as u64)
            .ok_or("direct write overflow")?;
        for bank in &self.tcm {
            if let Some((base, count)) = bank.extent() {
                let start = base.max(address);
                let stop = (base + count as u64).min(end);
                if start < stop {
                    bank.tracker
                        .mark((start - base) as usize, (stop - start) as usize)?;
                }
            }
        }
        for map in self.maps.borrow().iter() {
            let start = map.address.max(address);
            let stop = map.end().min(end);
            if start < stop {
                self.mark_backing_write(
                    map.pointer + (start - map.address) as usize,
                    (stop - start) as usize,
                )?;
            }
        }
        Ok(())
    }
    pub(super) fn rearm_observers(&self) -> Result<(), String> {
        let mut ranges = self.observed_backings.borrow().clone();
        for bank in &self.tcm {
            if let Some(observer) = bank
                .tracker
                .observer(NonNull::new(bank.memory.as_ptr()).unwrap())
            {
                ranges.push(observer);
            }
        }
        unsafe { self.cpu.observe_backings(&ranges) }
    }
    pub(super) fn fingerprint(&self) -> Result<serde_json::Value, String> {
        self.checkpoint_boundary()?;
        let memory = self
            .tcm
            .iter()
            .map(|bank| {
                let bytes =
                    unsafe { std::slice::from_raw_parts(bank.memory.as_ptr(), bank.memory.len()) };
                serde_json::json!({"register":bank.register.get(),"memory":{
                "bytes":bytes.len(),"sha256":format!("{:x}",Sha256::digest(bytes))}})
            })
            .collect::<Vec<_>>();
        let mut maps = self.checkpoint_maps();
        maps.sort_by_key(|m| m.address);
        let reservation = self
            .memory
            .monitors
            .lock()
            .map_err(|_| "exclusive monitors poisoned")?
            .reservation(self.monitor);
        Ok(
            serde_json::json!({"architecture":self.cpu.architecture_state()?, "identity_input":self.identity_input.get(), "pmu":self.pmu.get().fingerprint(),
            "clock":self.clock.get().map(|c|serde_json::json!({"now":c.now,"phase":c.phase,"ratio":c.ratio.parts()})),
            "execution_policy":self.policy.get().name(),
            "tcm":memory,"irq":self.irq.get(),"reservation":reservation.map(|r|(r.address,r.size)),
            "mappings":portable::maps_description(&maps) }),
        )
    }
    fn checkpoint_boundary(&self) -> Result<(), String> {
        self.memory.require_stopped()?;
        if self.active.get()
            || self.at_block.get()
            || self.at_instruction.get()
            || self.tcm_pending.get().is_some()
        {
            return Err("R-profile checkpoint requires a completed execution boundary".into());
        }
        Ok(())
    }
    fn checkpoint_maps(&self) -> Vec<Mapping> {
        let mut maps = self.maps.borrow().clone();
        maps.sort_by_key(|map| map.address);
        let mut normalized: Vec<Mapping> = Vec::with_capacity(maps.len());
        for map in maps {
            if let Some(last) = normalized.last_mut()
                && last.end() == map.address
                && last.pointer + last.bytes == map.pointer
                && last.permissions == map.permissions
                && match (last.physical, map.physical) {
                    (Some(a), Some(b)) => a + last.bytes as u64 == b,
                    (None, None) => true,
                    _ => false,
                }
            {
                last.bytes += map.bytes;
            } else {
                normalized.push(map);
            }
        }
        normalized
    }
    fn checkpoint_mmio(&self) -> Vec<(u64, usize)> {
        self.mmio_hooks
            .borrow()
            .iter()
            .map(|(&base, &(bytes, _))| (base, bytes))
            .collect()
    }
}
impl Core {
    fn checked_snapshot<'a>(&self, snapshot: &'a dyn Snapshot) -> Result<&'a Saved, String> {
        if snapshot.owner() != self.identity() {
            return Err("CPU snapshot belongs to a different processor".into());
        }
        snapshot
            .as_any()
            .downcast_ref::<Saved>()
            .ok_or("CPU snapshot belongs to a different backend".into())
    }
    pub(super) fn capture(
        &self,
        previous: Option<&dyn Snapshot>,
    ) -> Result<Box<dyn Snapshot>, String> {
        let e = &self.execution;
        e.checkpoint_boundary()?;
        if let Some(previous) = previous {
            if previous.owner() != self.identity()
                || !(previous.as_any().is::<Saved>()
                    || previous.as_any().is::<portable::Imported>())
            {
                return Err("CPU snapshot belongs to a different processor/backend".into());
            }
        }
        let context = e.cpu.snapshot(None)?;
        let tcm = std::array::from_fn(|bank| {
            let b = &e.tcm[bank];
            // All processors sharing these bytes are stopped. Images retain
            // only immutable copies, never an alias of a guest allocation.
            (b.register.get(), unsafe {
                b.tracker.capture(b.memory.as_ptr())
            })
        });
        e.rearm_observers()?;
        let (budget, head) = e.accounting().counters();
        Ok(Box::new(Saved {
            context,
            identity_input: e.identity_input.get(),
            clock: e.clock.get(),
            clock_overshoot: e.clock_overshoot.get(),
            policy: e.policy.get(),
            pmu: e.pmu.get(),
            tcm,
            maps: e.checkpoint_maps(),
            mmio: e.checkpoint_mmio(),
            reservation: e
                .memory
                .monitors
                .lock()
                .map_err(|_| "exclusive monitors poisoned")?
                .save(e.monitor),
            ring: unsafe { **e.ring.get() },
            budget,
            head,
            count: e.count.get(),
            cap: e.cap.get(),
            synced: e.synced.get(),
            settled_head: e.settled_head.get(),
            thumb: e.thumb.get(),
            restarted: e.restarted.get(),
            user_stop: e.user_stop.get(),
            irq: e.irq.get(),
            hint: e.hint.get(),
            exception: e.exception.get(),
            failure: e.failure.borrow().clone(),
        }))
    }
    pub(super) fn restore_captured(&self, snapshot: &dyn Snapshot) -> Result<(), String> {
        if snapshot.owner() != self.identity() {
            return Err("CPU snapshot belongs to a different processor".into());
        }
        if let Some(imported) = snapshot.as_any().downcast_ref::<portable::Imported>() {
            let saved = imported.bind(self)?;
            return self.restore_saved(&saved);
        }
        self.restore_saved(self.checked_snapshot(snapshot)?)
    }
    fn restore_saved(&self, saved: &Saved) -> Result<(), String> {
        let e = &self.execution;
        e.checkpoint_boundary()?;
        if saved.maps != e.checkpoint_maps() || saved.mmio != e.checkpoint_mmio() {
            return Err(
                "CPU snapshot external mappings differ; restore the board memory decode first"
                    .into(),
            );
        }
        // The saved reservation is eligible only if no external memory write
        // followed capture. Internal TCM rollback below must not itself count
        // as a conflicting *external* write. Every CPU is stopped throughout.
        let reservation = {
            let mut monitors = e
                .memory
                .monitors
                .lock()
                .map_err(|_| "exclusive monitors poisoned")?;
            monitors.restore(e.monitor, saved.reservation);
            monitors.reservation(e.monitor)
        };
        // Disable both banks first: a restored bank can occupy the current
        // other bank's window, including a complete ATCM/BTCM position swap.
        for bank in 0..2 {
            e.apply_tcm(bank, e.tcm[bank].register.get() & !1)?;
        }
        for bank in 0..2 {
            let b = &e.tcm[bank];
            e.apply_tcm(bank, saved.tcm[bank].0)?;
            let changed = unsafe { b.tracker.restore(&saved.tcm[bank].1, b.memory.as_ptr()) }?;
            for (offset, bytes) in changed {
                e.memory.backing_written(
                    e.tcm_physical(bank)? + offset as u64,
                    b.memory.as_ptr() as usize + offset,
                    bytes,
                )?;
            }
        }
        e.memory.flush()?;
        e.cpu.restore(saved.context.as_ref())?;
        e.cpu.invalidate_code(None)?;
        {
            let mut monitors = e
                .memory
                .monitors
                .lock()
                .map_err(|_| "exclusive monitors poisoned")?;
            monitors.clear(e.monitor);
            if let Some(r) = reservation {
                monitors.load(e.monitor, r.address, r.size);
            }
        }
        e.clock.set(saved.clock);
        e.identity_input.set(saved.identity_input);
        e.clock_overshoot.set(saved.clock_overshoot);
        e.set_policy(saved.policy)?;
        e.pmu.set(saved.pmu);
        unsafe {
            **e.ring.get() = saved.ring;
            (*e.record.as_ptr()).budget = saved.budget;
            (*e.record.as_ptr()).head = saved.head;
            (*e.record.as_ptr()).enabled = e.accounting_mode();
        }
        e.count.set(saved.count);
        e.cap.set(saved.cap);
        e.synced.set(saved.synced);
        e.settled_head.set(saved.settled_head);
        e.thumb.set(saved.thumb);
        e.restarted.set(saved.restarted);
        e.user_stop.set(saved.user_stop);
        e.irq.set(saved.irq);
        e.hint.set(saved.hint);
        e.exception.set(saved.exception);
        *e.failure.borrow_mut() = saved.failure.clone();
        e.rearm_observers()?;
        Ok(())
    }
    pub(super) fn import_captured(&self, bytes: &[u8]) -> Result<Box<dyn Snapshot>, String> {
        portable::import(self, bytes)
    }
}

#[cfg(test)]
mod tests;
