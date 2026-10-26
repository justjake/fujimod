//! Explicit execution experiment. Never selected by a device or counted as a boot.
use super::{Board, bus::Mode};
use fujimod_emulator_cpu::{Cpu, Hook, HookRange, Model, Register};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::c_void,
    rc::{Rc, Weak},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

/// A caller-requested routine substitution. It is only valid on a discovery
/// board, and its address/value remain visible in every board-state report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalleeStub {
    pub name: String,
    pub core: usize,
    pub address: u64,
    pub value: u64,
}

pub(super) struct CalleeStubContext {
    pub(super) spec: CalleeStub,
    cpus: Weak<Vec<Rc<dyn Cpu>>>,
    hits: AtomicU64,
    pub(super) failure: Mutex<Option<String>>,
}

impl CalleeStubContext {
    fn record_failure(&self, error: impl Into<String>) {
        if let Ok(mut failure) = self.failure.lock() {
            failure.get_or_insert_with(|| error.into());
        }
    }

    pub(super) fn state(&self) -> Value {
        json!({
            "kind": "callee_stub",
            "name": self.spec.name,
            "core": self.spec.core,
            "address": format!("{:#x}", self.spec.address),
            "value": self.spec.value,
            "hits": self.hits.load(Ordering::Relaxed),
            "failure": self.failure.lock().ok().and_then(|failure| failure.clone()),
        })
    }
}

unsafe fn substitute_callee(address: u64, _: u32, data: *mut c_void) {
    let context = unsafe { &*data.cast::<CalleeStubContext>() };
    if address != context.spec.address {
        return;
    }
    let Some(cpus) = context.cpus.upgrade() else {
        context.record_failure("processor was destroyed before discovery substitution");
        return;
    };
    let Some(cpu) = cpus.get(context.spec.core) else {
        context.record_failure("discovery substitution names an absent processor");
        return;
    };
    let mut link = [0; 8];
    if let Err(error) = cpu.read_register(Register::X(30), &mut link) {
        context.record_failure(format!("read return address: {error}"));
        return;
    }
    if let Err(error) = cpu.write_register(Register::X(0), &context.spec.value.to_le_bytes()) {
        context.record_failure(format!("write return value: {error}"));
        return;
    }
    if let Err(error) = cpu.write_register(Register::Pc, &link) {
        context.record_failure(format!("return from substituted routine: {error}"));
        return;
    }
    context.hits.fetch_add(1, Ordering::Relaxed);
}

impl Board {
    /// Install explicit AArch64 callee substitutions on a discovery board.
    /// The instruction hook redirects to the caller's return address; it does
    /// not inspect instruction bytes or infer a routine from program state.
    pub fn install_discovery_callee_stubs(&mut self, stubs: &[CalleeStub]) -> Result<(), String> {
        if stubs.is_empty() {
            return Ok(());
        }
        if self.state.borrow().bus.mode() != Mode::Discovery {
            return Err("callee substitutions require explicit discovery execution".into());
        }
        if stubs.len() + self.discovery_stub_contexts.len() > 4096
            || stubs.iter().any(|stub| {
                stub.address & 3 != 0
                    || stub.core >= self.cpus.len()
                    || self.cpus[stub.core].model() != Model::ArmV8A
            })
        {
            return Err("discovery callee substitutions require aligned AArch64 entries".into());
        }
        let mut unique = BTreeSet::new();
        for context in &self.discovery_stub_contexts {
            unique.insert((context.spec.core, context.spec.address));
        }
        if stubs
            .iter()
            .any(|stub| !unique.insert((stub.core, stub.address)))
        {
            return Err("duplicate discovery callee substitution".into());
        }

        let original_hooks = self.hooks.len();
        let original_contexts = self.discovery_stub_contexts.len();
        for spec in stubs {
            self.discovery_stub_contexts
                .push(Box::new(CalleeStubContext {
                    spec: spec.clone(),
                    cpus: Rc::downgrade(&self.cpus),
                    hits: AtomicU64::new(0),
                    failure: Mutex::new(None),
                }));
            let context = &mut **self.discovery_stub_contexts.last_mut().unwrap();
            let data = (context as *mut CalleeStubContext).cast();
            match unsafe {
                self.cpus[spec.core].install_hook(Hook::Instruction {
                    callback: substitute_callee,
                    data,
                    range: HookRange::Inclusive {
                        begin: spec.address,
                        end: spec.address,
                    },
                })
            } {
                Ok(hook) => self.hooks.push((spec.core, hook)),
                Err(error) => {
                    for (core, hook) in self.hooks.drain(original_hooks..) {
                        let _ = self.cpus[core].remove_hook(hook);
                    }
                    self.discovery_stub_contexts.truncate(original_contexts);
                    return Err(format!("install discovery callee substitution: {error}"));
                }
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct Discovery {
    pub enabled: bool,
    next: u64,
    entries: BTreeMap<Access, (u64, u64)>,
    shadow: BTreeMap<u64, u8>,
    unowned: BTreeSet<u64>,
    polls: BTreeMap<(usize, u32, u64, u32), Poll>,
    active_trials: Vec<(usize, u32, u64, u32)>,
    overrides: BTreeMap<(u64, u32), (u64, u64)>, // forced mask and bits
    pub interventions: Vec<Intervention>,
}

#[derive(Default)]
struct Poll {
    value: u64,
    last: u64,
    count: u64,
    next_trial: u32,
    trial: Option<(usize, u64, u64)>, // ledger row, candidate, start instructions
    kept: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Intervention {
    pub kind: String,
    pub cpu: usize,
    pub address: u64,
    pub pc: u32,
    pub size: u32,
    pub before: u64,
    pub value: u64,
    pub instructions: u64,
    pub outcome: String,
    pub evidence: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Access {
    cpu: usize,
    address: u64,
    pc: u32,
    size: u32,
    write: bool,
    value: u64,
    kind: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontierEntry {
    pub first: u64,
    pub count: u64,
    pub cpu: usize,
    pub address: u64,
    pub pc: u32,
    pub size: u32,
    pub write: bool,
    pub value: u64,
    pub kind: String,
}

impl Discovery {
    /// Ownership is framework input, never inferred from guest instructions.
    /// Extra whole-value trials are safe only in the unowned shadow bank.
    pub fn fallback_owner(&mut self, address: u64, size: u32, unowned: bool) {
        for byte in address..address + u64::from(size) {
            if unowned {
                self.unowned.insert(byte);
            } else {
                self.unowned.remove(&byte);
            }
        }
    }

    fn unowned(&self, address: u64, size: u32) -> bool {
        (address..address + u64::from(size)).all(|byte| self.unowned.contains(&byte))
    }

    fn trial_limit(&self, address: u64, size: u32) -> u32 {
        size * 8
            + if self.unowned(address, size) {
                2 + size * 16
            } else {
                0
            }
    }
    pub fn record(
        &mut self,
        cpu: usize,
        address: u64,
        pc: u32,
        size: u32,
        write: bool,
        value: u64,
    ) {
        self.record_kind(cpu, address, pc, size, write, value, "unsupported");
    }

    pub fn record_kind(
        &mut self,
        cpu: usize,
        address: u64,
        pc: u32,
        size: u32,
        write: bool,
        value: u64,
        kind: &'static str,
    ) {
        let row = self
            .entries
            .entry(Access {
                cpu,
                address,
                pc,
                size,
                write,
                value,
                kind,
            })
            .or_insert((self.next, 0));
        row.1 += 1;
        self.next += 1;
    }

    /// One framework fallback, shared across cores and overlapping widths.
    pub fn fallback(&mut self, address: u64, size: u32, write: bool, value: u64) -> u64 {
        if write {
            for i in 0..size {
                self.shadow
                    .insert(address + u64::from(i), (value >> (i * 8)) as u8);
            }
            value
        } else {
            (0..size).fold(0, |word, i| {
                word | (u64::from(*self.shadow.get(&(address + u64::from(i))).unwrap_or(&0))
                    << (i * 8))
            })
        }
    }

    /// Guest-written fallback bytes only; experimental read overrides are
    /// reported separately in the intervention ledger. This is observational.
    pub fn shadow_bytes(&self) -> Vec<(u64, u8)> {
        self.shadow
            .iter()
            .map(|(&address, &byte)| (address, byte))
            .collect()
    }

    pub fn settle_trials(&mut self, cpu: usize, instructions: u64) {
        for &(core, pc, address, size) in &self.active_trials {
            if core != cpu {
                continue;
            }
            let poll = self.polls.get_mut(&(core, pc, address, size)).unwrap();
            if let Some((row, candidate, _)) = poll.trial {
                if instructions.saturating_sub(poll.last) >= 4096 {
                    poll.trial = None;
                    poll.kept = Some(candidate);
                    poll.count = 0;
                    let entry = &mut self.interventions[row];
                    entry.outcome = "kept".into();
                    entry.evidence = format!(
                        "Poll site not reread for {} CPU instructions; workflow progress is not implied",
                        instructions - poll.last
                    );
                    poll.last = instructions;
                }
            }
        }
        self.active_trials
            .retain(|key| self.polls[key].trial.is_some());
    }

    pub fn observe_write(&mut self, address: u64, size: u32) {
        for row in &mut self.interventions {
            if row.outcome == "trial"
                && row.address + u64::from(row.size) > address
                && address + u64::from(size) > row.address
            {
                row.outcome = "superseded-by-guest-write".into();
                row.evidence =
                    "Guest wrote the register; removed the experimental bit override".into();
            }
        }
        self.overrides
            .retain(|&(a, n), _| a + u64::from(n) <= address || address + u64::from(size) <= a);
        self.polls.retain(|&(_, _, a, n), _| {
            a + u64::from(n) <= address || address + u64::from(size) <= a
        });
        self.active_trials
            .retain(|key| self.polls.contains_key(key));
    }

    fn overridden(&self, address: u64, size: u32, value: u64) -> Option<u64> {
        let mut word = value;
        let mut applied = false;
        for (&(base, width), &(mask, bits)) in &self.overrides {
            for byte in 0..size {
                let a = address + u64::from(byte);
                if a >= base && a < base + u64::from(width) {
                    let source = (a - base) * 8;
                    let target = byte * 8;
                    let m = ((mask >> source) & 0xff) << target;
                    let b = ((bits >> source) & 0xff) << target;
                    word = (word & !m) | (b & m);
                    applied = true;
                }
            }
        }
        applied.then_some(word)
    }

    pub fn observe_read(
        &mut self,
        cpu: usize,
        pc: u32,
        address: u64,
        size: u32,
        value: u64,
        instructions: u64,
    ) -> u64 {
        self.settle_trials(cpu, instructions);
        let limit = self.trial_limit(address, size);
        let override_value = self.overridden(address, size, value);
        if let Some(override_value) = override_value {
            if !self
                .polls
                .get(&(cpu, pc, address, size))
                .is_some_and(|p| p.trial.is_some() || p.kept.is_some())
            {
                return override_value;
            }
        }
        let poll = self.polls.entry((cpu, pc, address, size)).or_default();
        if poll.kept.is_some() {
            if value != poll.value || instructions.saturating_sub(poll.last) > 512 {
                poll.count = 0;
            }
            poll.value = value;
            poll.last = instructions;
            poll.count += 1;
            if poll.count < 1024 {
                return override_value.unwrap_or(value);
            }
            // Interrupts and task switches can create a long read gap even
            // when a candidate did not satisfy the poll. Resume the bounded
            // experiment if that same loop demonstrably starts polling again.
            if let Some(entry) = self.interventions.iter_mut().rev().find(|entry| {
                entry.cpu == cpu
                    && entry.pc == pc
                    && entry.address == address
                    && entry.size == size
                    && entry.outcome == "kept"
            }) {
                entry.outcome = "reverted".into();
                entry.evidence =
                    "Same poll site resumed for 1024 unchanged reads after retention".into();
            }
            poll.kept = None;
            poll.count = 0;
            self.overrides.remove(&(address, size));
        }
        if let Some((row, candidate, start)) = poll.trial {
            poll.last = instructions;
            if instructions.saturating_sub(start) < 4096 {
                return override_value.unwrap_or(candidate);
            }
            self.interventions[row].outcome = "reverted".into();
            self.interventions[row].evidence =
                "Same poll site still reread after 4096 CPU instructions".into();
            poll.trial = None;
            self.overrides.remove(&(address, size));
            poll.count = 0;
        }
        if value != poll.value || instructions.saturating_sub(poll.last) > 512 {
            poll.count = 0;
            if value != poll.value {
                poll.next_trial = 0;
            }
        }
        poll.value = value;
        poll.last = instructions;
        poll.count += 1;
        if poll.count < 1024 || poll.next_trial >= limit {
            return value;
        }
        let (candidate, kind) = if poll.next_trial < size * 8 {
            (value ^ (1u64 << poll.next_trial), "poll-bit-probe")
        } else if poll.next_trial == size * 8 {
            (0, "unowned-fallback-zero-probe")
        } else if poll.next_trial == size * 8 + 1 {
            (u64::MAX >> (64 - size * 8), "unowned-fallback-ones-probe")
        } else if poll.next_trial < size * 16 + 2 {
            (
                1u64 << (poll.next_trial - size * 8 - 2),
                "unowned-fallback-one-hot-probe",
            )
        } else {
            (
                (u64::MAX >> (64 - size * 8)) ^ (1u64 << (poll.next_trial - size * 16 - 2)),
                "unowned-fallback-one-clear-probe",
            )
        };
        poll.next_trial += 1;
        if candidate == value {
            return value;
        }
        let row = self.interventions.len();
        poll.trial = Some((row, candidate, instructions));
        self.active_trials.push((cpu, pc, address, size));
        self.overrides
            .insert((address, size), (value ^ candidate, candidate));
        self.interventions.push(Intervention {
            kind: kind.into(),
            cpu,
            address,
            pc,
            size,
            before: value,
            value: candidate,
            instructions,
            outcome: "trial".into(),
            evidence: format!(
                "1024 unchanged reads at the same PC, gaps <=512 CPU instructions; {kind}"
            ),
        });
        self.record_kind(cpu, address, pc, size, false, candidate, kind);
        candidate
    }

    pub fn frontier(&self, by_count: bool) -> Vec<FrontierEntry> {
        let mut rows: Vec<_> = self
            .entries
            .iter()
            .map(|(access, &(first, count))| FrontierEntry {
                first,
                count,
                cpu: access.cpu,
                address: access.address,
                pc: access.pc,
                size: access.size,
                write: access.write,
                value: access.value,
                kind: access.kind.into(),
            })
            .collect();
        if by_count {
            rows.sort_by_key(|r| (std::cmp::Reverse(r.count), r.first));
        } else {
            rows.sort_by_key(|r| r.first);
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fallback_is_byte_coherent_across_widths_and_unknown_bytes_are_zero() {
        let mut d = Discovery::default();
        assert_eq!(d.fallback(0xfff12300, 4, false, 0), 0);
        d.fallback(0xfff12300, 4, true, 0x12345678);
        d.fallback(0xfff12301, 1, true, 0xaa);
        let before = d.shadow_bytes();
        assert_eq!(d.fallback(0xfff12300, 4, false, 0), 0x1234aa78);
        assert_eq!(d.fallback(0xfff12302, 4, false, 0), 0x1234);
        assert_eq!(d.shadow_bytes(), before);
        assert_eq!(
            before,
            vec![
                (0xfff12300, 0x78),
                (0xfff12301, 0xaa),
                (0xfff12302, 0x34),
                (0xfff12303, 0x12)
            ]
        );
    }

    #[test]
    fn changing_values_and_spaced_reads_never_trigger_a_poll_probe() {
        let mut d = Discovery::default();
        for i in 0..10_000 {
            assert_eq!(d.observe_read(0, 0x2000, 0xfff12300, 4, i & 1, i), i & 1);
            assert_eq!(d.observe_read(1, 0x2000, 0xfff12300, 4, 0, i * 1024), 0);
        }
        assert!(d.interventions.is_empty());
    }

    #[test]
    fn only_unowned_shadow_registers_admit_whole_value_trials() {
        for unowned in [false, true] {
            let mut d = Discovery::default();
            d.fallback_owner(0xfff12300, 1, unowned);
            for i in 1..=100_000 {
                d.observe_read(0, 0x2000, 0xfff12300, 1, 3, i * 3);
            }
            assert_eq!(d.interventions.len(), if unowned { 26 } else { 8 });
            assert_eq!(d.interventions.iter().any(|r| r.value == 0), unowned);
            assert_eq!(d.interventions.iter().any(|r| r.value == 255), unowned);
            assert!(d.interventions.iter().all(|r| r.value <= 255));
            if unowned {
                d.fallback_owner(0xfff12300, 1, false);
                assert_eq!(d.trial_limit(0xfff12300, 1), 8);
            }
        }
    }

    #[test]
    fn mixed_set_clear_status_trials_are_bounded_and_only_for_unowned_words() {
        for (mask, expected, kind) in [
            (0x81, 1, "unowned-fallback-one-hot-probe"),
            (0xff, 0x7f, "unowned-fallback-one-clear-probe"),
        ] {
            for unowned in [false, true] {
                let mut d = Discovery::default();
                d.fallback_owner(0xfff12300, 1, unowned);
                let mut reached = false;
                for n in 1..=100_000 {
                    let reply = d.observe_read(0, 0x2000, 0xfff12300, 1, 0x80, n * 3);
                    if reply & mask == expected {
                        reached = true;
                        d.settle_trials(0, n * 3 + 4096);
                        break;
                    }
                }
                assert_eq!(reached, unowned);
                assert!(d.interventions.len() <= if unowned { 26 } else { 8 });
                assert!(d.interventions.iter().all(|row| row.value <= 255));
                if unowned {
                    let last = d.interventions.last().unwrap();
                    assert_eq!(last.kind, kind);
                    assert_eq!(last.outcome, "kept");
                }
            }
        }
    }

    #[test]
    fn kept_probe_preserves_live_bits_subword_reads_and_guest_writes() {
        let mut d = Discovery::default();
        for i in 1..=1024 {
            d.observe_read(0, 0x2000, 0xfff12300, 4, 0, i * 3);
        }
        d.settle_trials(0, 10_000);
        assert_eq!(d.interventions[0].outcome, "kept");
        assert_eq!(d.observe_read(1, 0x3000, 0xfff12300, 4, 0x10, 1), 0x11);
        assert_eq!(d.observe_read(1, 0x3004, 0xfff12300, 1, 0xa0, 2), 0xa1);
        d.observe_write(0xfff12300, 1);
        assert_eq!(d.observe_read(1, 0x3000, 0xfff12300, 4, 0x10, 3), 0x10);
    }
    #[test]
    fn resumed_poll_retries_after_interrupt_or_task_switch_gap() {
        let mut d = Discovery::default();
        d.fallback_owner(0xfff12300, 1, true);
        for n in 1..=1024 {
            d.observe_read(0, 0x2000, 0xfff12300, 1, 0, n * 3);
        }
        d.settle_trials(0, 10_000);
        assert_eq!(d.interventions[0].outcome, "kept");
        assert!(
            d.interventions[0]
                .evidence
                .contains("6928 CPU instructions")
        );
        for n in 1..1024 {
            assert_eq!(
                d.observe_read(0, 0x2000, 0xfff12300, 1, 0, 10_000 + n * 3),
                1
            );
        }
        assert_eq!(d.observe_read(0, 0x2000, 0xfff12300, 1, 0, 13_072), 0);
        assert_eq!(d.interventions[0].outcome, "reverted");
        assert!(d.interventions[0].evidence.contains("resumed"));
        for n in 1..=1023 {
            d.observe_read(0, 0x2000, 0xfff12300, 1, 0, 13_072 + n * 3);
        }
        assert_eq!(d.interventions.len(), 2);
        assert_eq!(d.interventions[1].value, 2);
        assert_eq!(d.interventions[1].outcome, "trial");
    }
    #[test]
    fn aggregation_and_both_rankings_preserve_access_identity() {
        let mut d = Discovery::default();
        assert!(!d.enabled);
        d.record(0, 0xfff12300, 0x1000, 4, false, 0);
        d.record(1, 0xfff12304, 0x2000, 2, true, 7);
        d.record(1, 0xfff12304, 0x2000, 2, true, 7);
        d.record(1, 0xfff12304, 0x2000, 2, true, 8);
        assert_eq!(
            d.frontier(false)
                .iter()
                .map(|r| (r.first, r.count))
                .collect::<Vec<_>>(),
            vec![(0, 1), (1, 2), (3, 1)]
        );
        assert_eq!(
            d.frontier(true)[0],
            FrontierEntry {
                first: 1,
                count: 2,
                cpu: 1,
                address: 0xfff12304,
                pc: 0x2000,
                size: 2,
                write: true,
                value: 7,
                kind: "unsupported".into(),
            }
        );
    }
}
