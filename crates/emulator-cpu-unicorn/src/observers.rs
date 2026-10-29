//! Passive callbacks indexed by kind and disjoint address spans. Mutations
//! happen only while stopped; execution looks up one span instead of scanning
//! every installed observer. Entries retain hook installation order.
use fujimod_emulator_cpu::{Hook, HookRange};
use std::collections::BTreeMap;
#[cfg(test)]
use std::ffi::c_void;

#[derive(Default)]
pub(crate) struct Ranges(BTreeMap<u64, Vec<(usize, Hook)>>);

impl Ranges {
    pub fn at(&self, address: u64) -> &[(usize, Hook)] {
        self.0
            .range(..=address)
            .next_back()
            .map_or(&[], |(_, hooks)| hooks.as_slice())
    }

    /// Whether any observer covers an address in `low..=high`. A branch to
    /// itself (`b .`, as the stock power-off loop ends) gives `low == high`.
    pub fn any_in(&self, low: u64, high: u64) -> bool {
        !self.at(low).is_empty()
            || low < high
                && self
                    .0
                    .range(low + 1..=high)
                    .any(|(_, hooks)| !hooks.is_empty())
    }

    /// Whether any observer is installed; removal coalescing leaves no spans
    /// once the last one is gone.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn split(&mut self, address: u64) {
        if !self.0.contains_key(&address) {
            let hooks = self.at(address).to_vec();
            self.0.insert(address, hooks);
        }
    }

    fn bounds(range: HookRange) -> Option<(u64, u64)> {
        match range {
            HookRange::All => Some((0, u64::MAX)),
            HookRange::Inclusive { begin, end } => (begin <= end).then_some((begin, end)),
        }
    }

    fn insert(&mut self, key: usize, hook: Hook, range: HookRange) {
        let Some((begin, end)) = Self::bounds(range) else {
            return;
        };
        self.split(begin);
        if let Some(after) = end.checked_add(1) {
            self.split(after);
        }
        for (_, hooks) in self.0.range_mut(begin..=end) {
            hooks.push((key, hook));
        }
    }

    fn remove(&mut self, key: usize, range: HookRange) {
        let Some((begin, end)) = Self::bounds(range) else {
            return;
        };
        for (_, hooks) in self.0.range_mut(begin..=end) {
            hooks.retain(|(id, _)| *id != key);
        }
        // Spans are coalesced before the removal, so only the changed spans and
        // their neighbors can now repeat the span before them (or, first in
        // the map, be empty). Coalescing the whole map on every removal made
        // each watch cost grow with every installed observer.
        let low = self.0.range(..begin).next_back().map_or(begin, |(&a, _)| a);
        let high = end.checked_add(1).unwrap_or(u64::MAX);
        let mut previous: &[(usize, Hook)] =
            self.0.range(..low).next_back().map_or(&[], |(_, h)| h);
        let mut redundant = vec![];
        for (&address, hooks) in self.0.range(low..=high) {
            if previous.len() == hooks.len()
                && previous.iter().zip(hooks).all(|((a, _), (b, _))| a == b)
            {
                redundant.push(address);
            } else {
                previous = hooks;
            }
        }
        for address in redundant {
            self.0.remove(&address);
        }
    }
}

#[derive(Default)]
pub(crate) struct Index {
    pub instructions: Ranges,
    pub blocks: Ranges,
    pub stores: Ranges,
    pub reads: Ranges,
    pub writes: Ranges,
    pub exceptions: Vec<(usize, Hook)>,
    pub synchronization: Vec<(usize, Hook)>,
}

impl Index {
    pub fn insert(&mut self, key: usize, hook: Hook) {
        match hook {
            Hook::Instruction { range, .. } => self.instructions.insert(key, hook, range),
            Hook::Block { range, .. } => self.blocks.insert(key, hook, range),
            Hook::CompletedStore { range, .. } => self.stores.insert(key, hook, range),
            Hook::Exception { .. } => self.exceptions.push((key, hook)),
            Hook::Synchronization { .. } => self.synchronization.push((key, hook)),
            Hook::Memory {
                read, write, range, ..
            } => {
                if read {
                    self.reads.insert(key, hook, range);
                }
                if write {
                    self.writes.insert(key, hook, range);
                }
            }
            Hook::Fault { .. } => (),
        }
    }

    pub fn remove(&mut self, key: usize, hook: Hook) {
        match hook {
            Hook::Instruction { range, .. } => self.instructions.remove(key, range),
            Hook::Block { range, .. } => self.blocks.remove(key, range),
            Hook::CompletedStore { range, .. } => self.stores.remove(key, range),
            Hook::Exception { .. } => self.exceptions.retain(|(id, _)| *id != key),
            Hook::Synchronization { .. } => self.synchronization.retain(|(id, _)| *id != key),
            Hook::Memory {
                read, write, range, ..
            } => {
                if read {
                    self.reads.remove(key, range);
                }
                if write {
                    self.writes.remove(key, range);
                }
            }
            Hook::Fault { .. } => (),
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod range_tests {
    use super::Ranges;

    #[test]
    fn a_self_branch_span_is_one_address() {
        let ranges = Ranges::default();
        assert!(!ranges.any_in(0x6d0000, 0x6d0000));
        assert!(!ranges.any_in(0x6d0004, 0x6d0000));
    }
}
