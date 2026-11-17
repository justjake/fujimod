//! Passive first-entry witnesses. No execution or device decision consumes them.
use super::{Board, State};
use fujimod_emulator_cpu::{Architecture, Cpu, Hook, HookRange, Register, arm};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    ffi::c_void,
    rc::{Rc, Weak},
};

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Entries(BTreeMap<(usize, u64), Value>);

const PAGE: u64 = 0x1000;
/// Bus-backed memory `[address, address + length)` summarized without side effects: the SHA-256 of
/// the bytes, how many 4 KiB pages hold a nonzero byte and the runs of such pages (at most 64
/// are listed). A range with no complete backing reports the error instead.
pub(super) fn range_digest(memory: &super::memory::Memory, address: u64, length: u64) -> Value {
    const LISTED: usize = 64;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    let (mut pages, mut runs, mut open): (u64, Vec<(u64, u64)>, Option<u64>) = (0, vec![], None);
    let mut runs_dropped = 0u64;
    let mut at = address;
    let end = address.saturating_add(length);
    while at < end {
        let n = ((end - at) as usize).min(buffer.len());
        if let Err(error) = memory.read_host(at, &mut buffer[..n]) {
            return json!({"address":address,"length":length,"error":error,"failed_at":at});
        }
        hasher.update(&buffer[..n]);
        for (index, page) in buffer[..n].chunks(PAGE as usize).enumerate() {
            let start = at + index as u64 * PAGE;
            if page.iter().any(|&byte| byte != 0) {
                pages += 1;
                open.get_or_insert(start);
            } else if let Some(first) = open.take() {
                if runs.len() < LISTED { runs.push((first, start)); } else { runs_dropped += 1; }
            }
        }
        at += n as u64;
    }
    if let Some(first) = open {
        if runs.len() < LISTED { runs.push((first, end)); } else { runs_dropped += 1; }
    }
    let digest: String = hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    json!({"address":address,"length":length,"sha256":digest,"nonzero_pages_4k":pages,
        "nonzero_runs":runs,"nonzero_runs_not_listed":runs_dropped})
}

/// `FUJIMOD_WATCH_HASH=ADDR:LEN[;ADDR:LEN...]` (decimal or `0x`): the digests of these ranges are
/// added to a watched PC's first-entry record on core 0, taken at the moment it first reaches it.
fn watch_hash_ranges() -> Vec<(u64, u64)> {
    let Ok(spec) = std::env::var("FUJIMOD_WATCH_HASH") else { return vec![] };
    let number = |text: &str| match text.trim().strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => text.trim().parse().ok(),
    };
    spec.split(';')
        .filter(|item| !item.trim().is_empty())
        .filter_map(|item| {
            let (address, length) = item.split_once(':')?;
            Some((number(address)?, number(length)?))
        })
        .collect()
}
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
    // XT5_WATCH_ALL: record every entry (bounded), keyed by a sequence number.
    let all = std::env::var_os("XT5_WATCH_ALL").is_some();
    let key = if all {
        let n = entries.0.len();
        if n >= 200_000 {
            return;
        }
        (watch.core + 16 * (n + 1), pc)
    } else {
        (watch.core, pc)
    };
    if entries.0.contains_key(&key) {
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
    // AArch64 frame-pointer backtrace (x30, then saved {fp, lr} pairs).
    let mut backtrace = vec![];
    if cpu.model().architecture() == Architecture::Arm64 {
        if let Ok(lr) = super::scalar(cpu, Register::X(30)) {
            backtrace.push(lr);
        }
        let mut fp = super::scalar(cpu, Register::X(29)).unwrap_or(0);
        for _ in 0..16 {
            let mut frame = [0u8; 16];
            if fp == 0 || fp & 7 != 0 || state.memory.read_host(fp, &mut frame).is_err() {
                break;
            }
            let next = u64::from_le_bytes(frame[..8].try_into().unwrap());
            backtrace.push(u64::from_le_bytes(frame[8..].try_into().unwrap()));
            if next <= fp {
                break;
            }
            fp = next;
        }
    }
    let sp = super::scalar(cpu, Register::Sp).unwrap_or(0);
    // Digests are taken on core 0's first entry only: they read all of the requested ranges.
    let ranges = if watch.core == 0 { watch_hash_ranges() } else { vec![] };
    let mut record = json!({"core":watch.core,"pc":pc,
        "instructions":counts,"tick":state.timeline.now().saturating_add(elapsed),
        "arguments":arguments,"argument_bus_memory":argument_memory,"backtrace":backtrace,"sp":sp});
    // The `digests` key exists only when FUJIMOD_WATCH_HASH names ranges, so records of runs that
    // do not ask for it are unchanged.
    if !ranges.is_empty() {
        let digests: Vec<Value> = ranges
            .into_iter()
            .map(|(address, length)| range_digest(&state.memory, address, length))
            .collect();
        record["digests"] = Value::from(digests);
    }
    entries.0.insert(key, record);
}
impl Board {
    /// Digest of bus-backed physical memory, as in a watched PC's `digests` (host-side, no guest
    /// access or device effect).
    pub fn range_digest(&self, address: u64, length: u64) -> Value {
        range_digest(&self.physical_memory, address, length)
    }
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
