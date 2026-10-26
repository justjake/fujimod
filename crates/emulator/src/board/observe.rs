//! Passive first-entry witnesses. No execution or device decision consumes them.
use super::{Board, State};
use fujimod_emulator_cpu::{Architecture, Cpu, Hook, HookRange, Register, arm};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    ffi::c_void,
    rc::{Rc, Weak},
};

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Entries(BTreeMap<(usize, u64), Value>);
pub(super) struct Watch {
    state: Weak<RefCell<State>>,
    cpus: Weak<Vec<Rc<dyn Cpu>>>,
    entries: Rc<RefCell<Entries>>,
    core: usize,
    addresses: Vec<u64>,
}
unsafe fn entered(pc: u64, _: u32, data: *mut c_void) {
    let watch = unsafe { &*data.cast::<Watch>() };
    let mut entries = watch.entries.borrow_mut();
    if entries.0.contains_key(&(watch.core, pc)) {
        return;
    }
    let (Some(state), Some(cpus)) = (watch.state.upgrade(), watch.cpus.upgrade()) else {
        return;
    };
    let state = state.borrow();
    let extra = cpus[watch.core]
        .instruction_progress()
        .unwrap_or(state.accounted[watch.core])
        .saturating_sub(state.accounted[watch.core]);
    let mut counts = state.instructions.clone();
    counts[watch.core] = counts[watch.core].saturating_add(extra);
    let elapsed = state
        .timeline
        .ratio()
        .advance_phase(state.timeline.phase(), extra)
        .map(|v| v.0)
        .unwrap_or(0)
        .min(u64::MAX as u128) as u64;
    let cpu = &*cpus[watch.core];
    let arguments: Vec<_> = (0..4)
        .map(|i| {
            super::scalar(
                cpu,
                if cpu.model().architecture() == Architecture::Arm64 {
                    Register::X(i)
                } else {
                    Register::Arm32(arm::Register::R(i))
                },
            )
            .ok()
        })
        .collect();
    // Read only bus-backed memory, bypassing MMIO and all device callbacks.
    // Unmapped/virtual-only pointers have no snapshot. The witness remains
    // bounded even when an argument names an integer rather than a pointer.
    let argument_memory: Vec<_> = arguments
        .iter()
        .map(|address| {
            let mut bytes = [0u8; 32];
            address.and_then(|address| {
                state
                    .memory
                    .read_host(address, &mut bytes)
                    .ok()
                    .map(|_| bytes.to_vec())
            })
        })
        .collect();
    entries.0.insert(
        (watch.core, pc),
        json!({"core":watch.core,"pc":pc,
        "instructions":counts,"tick":state.timeline.now().saturating_add(elapsed),
        "arguments":arguments,"argument_bus_memory":argument_memory}),
    );
}
impl Board {
    pub fn observation_topology(&self) -> Vec<(usize, Vec<u64>)> {
        self.watches
            .iter()
            .map(|w| (w.core, w.addresses.clone()))
            .collect()
    }
    /// Observe selected instruction entries once per processor. Host inspection
    /// never installs a replacement instruction or authorizes device effects.
    pub fn watch_pcs(&mut self, addresses: &[u64]) -> Result<(), String> {
        if addresses.is_empty() {
            return Ok(());
        }
        let existing: BTreeSet<_> = self
            .watches
            .iter()
            .flat_map(|w| w.addresses.iter().copied())
            .collect();
        if addresses.len() + existing.len() > 4096
            || addresses.iter().any(|p| existing.contains(p))
            || addresses.windows(2).any(|p| p[0] >= p[1])
            || addresses.iter().any(|p| p & 1 != 0)
        {
            return Err(
                "PC witnesses require new sorted unique addresses within the total bound".into(),
            );
        }
        for core in 0..self.cpus.len() {
            let watch = Box::new(Watch {
                state: Rc::downgrade(&self.state),
                cpus: Rc::downgrade(&self.cpus),
                entries: self.entries.clone(),
                core,
                addresses: addresses.to_vec(),
            });
            self.watches.push(watch);
            let data = (&**self.watches.last().unwrap() as *const Watch)
                .cast_mut()
                .cast();
            for &pc in addresses {
                // The boxed context stays at this address until Board drops
                // its CPU hooks; its CPU/state links are weak and serialized.
                let hook = unsafe {
                    self.cpus[core].install_hook(Hook::Instruction {
                        callback: entered,
                        data,
                        range: HookRange::Inclusive { begin: pc, end: pc },
                    })?
                };
                self.hooks.push((core, hook));
            }
        }
        Ok(())
    }
    pub fn pc_entries(&self) -> Vec<Value> {
        self.entries.borrow().0.values().cloned().collect()
    }
}
