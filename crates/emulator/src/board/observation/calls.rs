//! Bounded scalar call observations shared by host CPU instrumentation.
//! No memory reads, guest substitutions or device behavior enter these records.
use std::collections::{HashMap, HashSet};

pub struct Recorder {
    entries: HashMap<u64, u64>,
    watches: HashSet<u64>,
    returns: HashSet<u64>,
    edges: HashMap<(u64, u64), u64>,
    states: Vec<Call>,
    pending: HashMap<(usize, u64, u32), Vec<usize>>,
    quota: u64,
    per_edge: bool,
    edge_limit: usize,
    state_limit: usize,
    dropped_edges: u64,
    dropped_states: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub callee: u64,
    pub core: usize,
    pub instructions: u64,
    pub registers: [u32; 16],
    pub returned: Option<(u64, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub edges: Vec<(u64, u64, u64)>,
    pub hits: Vec<(u64, u64)>,
    pub states: Vec<Call>,
    pub dropped_edges: u64,
    pub dropped_states: u64,
}

impl Recorder {
    pub fn new(
        entries: Vec<u64>,
        returns: Vec<u64>,
        quota: u64,
        per_edge: bool,
        edge_limit: usize,
        state_limit: usize,
    ) -> Result<Self, String> {
        if entries.is_empty()
            || entries.len() > 1 << 18
            || returns.len() > 1 << 20
            || entries
                .iter()
                .chain(&returns)
                .any(|a| a & 1 != 0 || *a > u32::MAX as u64)
            || edge_limit > 1 << 24
            || state_limit > 1 << 22
        {
            return Err("Invalid scalar recorder addresses or limits".into());
        }
        Ok(Self {
            watches: entries.iter().chain(&returns).copied().collect(),
            entries: entries.into_iter().map(|a| (a, 0)).collect(),
            returns: returns.into_iter().collect(),
            edges: HashMap::new(),
            states: Vec::new(),
            pending: HashMap::new(),
            quota,
            per_edge,
            edge_limit,
            state_limit,
            dropped_edges: 0,
            dropped_states: 0,
        })
    }
    pub fn watches(&self, pc: u64) -> bool {
        self.watches.contains(&pc)
    }
    pub fn observe(
        &mut self,
        core: usize,
        pc: u64,
        instructions: u64,
        registers: [u32; 16],
        resume_pc: u64,
    ) {
        let sp = registers[13];
        if self.returns.contains(&pc)
            && let Some(frames) = self.pending.remove(&(core, pc, sp))
        {
            for frame in frames {
                self.states[frame].returned = Some((instructions, registers[0]));
            }
        }
        let Some(hits) = self.entries.get_mut(&pc) else {
            return;
        };
        *hits += 1;
        let lr = u64::from(registers[14]);
        let count = if let Some(count) = self.edges.get_mut(&(pc, lr)) {
            *count += 1;
            *count
        } else if self.edges.len() < self.edge_limit {
            self.edges.insert((pc, lr), 1);
            1
        } else {
            self.dropped_edges += 1;
            u64::MAX
        };
        if (if self.per_edge { count } else { *hits }) > self.quota {
            return;
        }
        if self.states.len() >= self.state_limit {
            self.dropped_states += 1;
            return;
        }
        if self.returns.contains(&resume_pc) {
            self.pending
                .entry((core, resume_pc, sp))
                .or_default()
                .push(self.states.len());
        }
        self.states.push(Call {
            callee: pc,
            core,
            instructions,
            registers,
            returned: None,
        });
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut edges: Vec<_> = self
            .edges
            .iter()
            .map(|(&(pc, lr), &n)| (pc, lr, n))
            .collect();
        edges.sort_unstable();
        let mut hits: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, n)| **n != 0)
            .map(|(&pc, &n)| (pc, n))
            .collect();
        hits.sort_unstable();
        Snapshot {
            edges,
            hits,
            states: self.states.clone(),
            dropped_edges: self.dropped_edges,
            dropped_states: self.dropped_states,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observations_keep_raw_addresses_and_match_core_resume_and_stack() {
        let mut r = Recorder::new(vec![0x1000, 0x40001000], vec![0x2000], 4, true, 16, 16).unwrap();
        let mut regs = [0; 16];
        regs[13] = 0x3000;
        regs[14] = 0x2000;
        r.observe(0, 0x1000, 1, regs, u64::from(regs[14]) & !1);
        r.observe(1, 0x40001000, 2, regs, u64::from(regs[14]) & !1);
        regs[0] = 7;
        regs[13] += 4;
        r.observe(0, 0x2000, 3, regs, u64::from(regs[14]) & !1);
        assert!(r.states.iter().all(|s| s.returned.is_none()));
        regs[13] -= 4;
        r.observe(1, 0x2000, 4, regs, u64::from(regs[14]) & !1);
        assert!(r.states[0].returned.is_none());
        assert_eq!(r.states[1].returned, Some((4, 7)));
        regs[0] = 9;
        r.observe(0, 0x2000, 5, regs, u64::from(regs[14]) & !1);
        assert_eq!(r.states[0].returned, Some((5, 9)));
        assert_eq!(r.edges.len(), 2);
        assert!(!r.watches(0x40002000));
        let saved = r.snapshot();
        assert_eq!(saved, r.snapshot());
        assert_eq!(saved.hits, vec![(0x1000, 1), (0x40001000, 1)]);
        assert_eq!(
            saved.edges,
            vec![(0x1000, 0x2000, 1), (0x40001000, 0x2000, 1)]
        );
        assert_eq!(saved.states[0].returned, Some((5, 9)));
        assert_eq!(saved.states[1].returned, Some((4, 7)));
    }
}
