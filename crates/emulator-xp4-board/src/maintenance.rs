//! Serialized delivery of accepted translation maintenance within one cluster.
use fujimod_emulator_cpu::{Cpu, Identity, arm};
use fujimod_emulator_cpu_unicorn::{Core, maintenance as native};
use std::{cell::RefCell, collections::VecDeque};

#[cfg(test)]
#[path = "maintenance_tests.rs"]
mod tests;

pub use native::{Event, Status};
pub const RECORD_LIMIT: usize = 64;
const SMP: u32 = 1 << 6;
const DDVM: u32 = 1 << 15;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    pub source_core: usize,
    pub source_identity: Identity,
    pub event: Event,
    pub peer_actlr: Vec<u32>,
    pub recipients: u8,
    pub applied: u8,
    pub excluded: u8,
    pub acknowledged: bool,
    pub error: Option<String>,
    pub raw_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub identities: Vec<Identity>,
    pub native: Vec<Status>,
    pub completed: u64,
    pub delivered_peers: u64,
    pub dropped: u64,
    pub records: Vec<Delivery>,
    pub pending: Option<Delivery>,
    pub failure: Option<String>,
    pub last_raw_error: Option<String>,
}

#[derive(Default)]
struct State {
    completed: u64,
    delivered_peers: u64,
    dropped: u64,
    sequences: Vec<u64>,
    records: VecDeque<Delivery>,
    pending: Option<Delivery>,
    failure: Option<String>,
    last_raw_error: Option<String>,
}

pub(super) struct Maintenance {
    owners: Vec<native::Owner>,
    tokens: Vec<u64>,
    identities: Vec<Identity>,
    state: RefCell<State>,
}

impl Maintenance {
    pub fn new(cpus: &[Core]) -> Result<Self, String> {
        let mut owners = Vec::with_capacity(cpus.len());
        let mut tokens = Vec::with_capacity(cpus.len());
        for cpu in cpus {
            // Cluster owns every Core privately, serializes execution and
            // settles its accepted events before another engine can run.
            let owner = unsafe { cpu.attach_translation_owner()? };
            let status = cpu.translation_maintenance_status()?;
            if status.owner == 0 || status.pending != 0 || status.poison != 0
                || status.next_sequence != 1
            {
                return Err("Translation owner did not attach to a fresh engine".into());
            }
            owners.push(owner);
            tokens.push(status.owner);
        }
        Ok(Self {
            owners,
            tokens,
            identities: cpus.iter().map(Cpu::identity).collect(),
            state: RefCell::new(State {
                sequences: vec![0; cpus.len()],
                ..State::default()
            }),
        })
    }

    fn statuses(&self, cpus: &[Core]) -> Result<Vec<Status>, String> {
        if cpus.len() != self.owners.len() {
            return Err("Translation domain membership changed".into());
        }
        cpus.iter().enumerate().map(|(index, cpu)| {
            if cpu.identity() != self.identities[index] {
                return Err("Translation processor identity changed".into());
            }
            let status = cpu.translation_maintenance_status()?;
            if status.owner != self.tokens[index] || status.pending > native::CAPACITY {
                return Err("Invalid translation owner status".into());
            }
            Ok(status)
        }).collect()
    }

    pub fn ready(&self, cpus: &[Core]) -> Result<(), String> {
        let state = self.state.borrow();
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        if state.pending.is_some() {
            return Err("Translation delivery remains incomplete".into());
        }
        for (core, status) in self.statuses(cpus)?.iter().enumerate() {
            if status.pending != 0 || status.poison != 0 {
                return Err(format!("Core {core} has unsettled translation maintenance: {} pending, poison {}",
                    status.pending, status.poison));
            }
        }
        Ok(())
    }

    pub fn snapshot(&self, cpus: &[Core]) -> Result<Snapshot, String> {
        let native = self.statuses(cpus)?;
        let state = self.state.borrow();
        Ok(Snapshot {
            identities: self.identities.clone(),
            native,
            completed: state.completed,
            delivered_peers: state.delivered_peers,
            dropped: state.dropped,
            records: state.records.iter().cloned().collect(),
            pending: state.pending.clone(),
            failure: state.failure.clone(),
            last_raw_error: state.last_raw_error.clone(),
        })
    }

    pub fn settle(&self, cpus: &[Core], source: usize, raw_error: Option<&str>) -> Result<(), String> {
        if let Some(error) = raw_error {
            self.state.borrow_mut().last_raw_error = Some(error.to_owned());
        }
        let result = self.deliver(cpus, source, raw_error);
        if let Err(error) = &result {
            let mut state = self.state.borrow_mut();
            state.failure.get_or_insert_with(|| error.clone());
            if let Some(pending) = &mut state.pending {
                pending.error.get_or_insert_with(|| error.clone());
            }
        }
        result
    }

    fn deliver(&self, cpus: &[Core], source: usize, raw_error: Option<&str>) -> Result<(), String> {
        {
            let state = self.state.borrow();
            if let Some(error) = &state.failure {
                return Err(error.clone());
            }
            if state.pending.is_some() {
                return Err("Translation delivery cannot be retried".into());
            }
        }
        let statuses = self.statuses(cpus)?;
        // Poison anywhere freezes the domain before any new peer effect.
        for (core, status) in statuses.iter().enumerate() {
            if status.poison != 0 {
                return Err(format!("Core {core} translation maintenance poison {}", status.poison));
            }
            if core != source && status.pending != 0 {
                return Err(format!("Paused core {core} has unexpected translation work"));
            }
        }
        let count = statuses.get(source).ok_or("Invalid translation source core")?.pending;
        for _ in 0..count {
            let status = cpus[source].translation_maintenance_status()?;
            if status.poison != 0 || status.pending == 0 || status.owner != self.tokens[source] {
                return Err("Translation source queue changed during delivery".into());
            }
            let event = status.front;
            self.state.borrow_mut().pending = Some(Delivery {
                source_core: source,
                source_identity: self.identities[source],
                event,
                peer_actlr: Vec::with_capacity(cpus.len()),
                recipients: 0,
                applied: 0,
                excluded: 0,
                acknowledged: false,
                error: None,
                raw_error: raw_error.map(str::to_owned),
            });
            let expected = self.state.borrow().sequences[source].checked_add(1)
                .ok_or("Translation sequence exhausted")?;
            // Native code emits only after instruction condition/access checks.
            // Security, EL, TTBR and VMID are evidence, never peer filters.
            if event.owner != self.tokens[source] || event.sequence != expected
                || event.selector != native::TLBIALLIS || event.secure > 1
                || event.exception_level > 3 || event.actlr & SMP == 0 || event.actlr & DDVM != 0
            {
                return Err("Unsupported or inconsistent accepted translation event".into());
            }
            let mut peer_actlr = Vec::with_capacity(cpus.len());
            let mut recipients = 0u8;
            let mut excluded = 0u8;
            for (core, cpu) in cpus.iter().enumerate() {
                // The source participation state belongs to the accepted
                // instruction; later source writes cannot rewrite that event.
                let actlr = if core == source {
                    event.actlr
                } else {
                    u32::try_from(cpu.coprocessor(arm::Coprocessor {
                        cp: 15, wide: false, secure: 0, crn: 1, crm: 0, opc1: 0, opc2: 1,
                    }, None)?).map_err(|_| "ACTLR exceeds 32 bits")?
                };
                peer_actlr.push(actlr);
                if core != source {
                    if actlr & SMP == 0 {
                        excluded |= 1 << core;
                    } else {
                        recipients |= 1 << core;
                    }
                }
            }
            {
                let mut state = self.state.borrow_mut();
                let pending = state.pending.as_mut().unwrap();
                pending.peer_actlr = peer_actlr;
                pending.recipients = recipients;
                pending.excluded = excluded;
                if pending.peer_actlr.iter().enumerate()
                    .any(|(core, actlr)| recipients & (1 << core) != 0 && actlr & DDVM != 0)
                {
                    return Err("Participating translation peer has unsupported ACTLR.DDVM".into());
                }
                state.completed.checked_add(1).ok_or("Translation completion count exhausted")?;
                state.delivered_peers.checked_add(u64::from(recipients.count_ones()))
                    .ok_or("Translation peer count exhausted")?;
                if state.records.len() == RECORD_LIMIT {
                    state.dropped.checked_add(1).ok_or("Translation diagnostic count exhausted")?;
                }
            }
            // The complete recipient set is frozen before the first flush.
            for (core, cpu) in cpus.iter().enumerate() {
                if recipients & (1 << core) != 0 {
                    cpu.flush_local_translations(&self.owners[core])
                        .map_err(|error| format!("Translation delivery to core {core}: {error}"))?;
                    let mut state = self.state.borrow_mut();
                    state.pending.as_mut().unwrap().applied |= 1 << core;
                    state.delivered_peers += 1;
                }
            }
            cpus[source].acknowledge_translation_maintenance(&self.owners[source], &event)?;
            let mut state = self.state.borrow_mut();
            let mut record = state.pending.take().unwrap();
            record.acknowledged = true;
            state.sequences[source] = event.sequence;
            state.completed += 1;
            if state.records.len() == RECORD_LIMIT {
                state.records.pop_front();
                state.dropped += 1;
            }
            state.records.push_back(record);
        }
        self.ready(cpus)
    }
}
