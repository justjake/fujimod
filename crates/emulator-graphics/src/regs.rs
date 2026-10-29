//! Register banks with a Python dict's insertion order.

use std::collections::{HashMap, hash_map::Entry};

/// Register index -> value, iterating in first-insertion order.
/// Device state and fingerprints preserve that order.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Registers {
    order: Vec<u32>,
    values: HashMap<u32, u64>,
}

impl Registers {
    pub fn new() -> Self {
        Self::default()
    }
    #[inline]
    pub fn get(&self, key: u32) -> Option<u64> {
        self.values.get(&key).copied()
    }
    pub fn contains(&self, key: u32) -> bool {
        self.values.contains_key(&key)
    }
    #[inline]
    pub fn set(&mut self, key: u32, value: u64) {
        match self.values.entry(key) {
            Entry::Occupied(mut entry) => {
                if *entry.get() != value {
                    entry.insert(value);
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(value);
                self.order.push(key);
            }
        }
    }
    /// Apply selected byte lanes using one hash-table lookup.
    #[inline]
    pub fn write_masked(&mut self, key: u32, mask: u64, value: u64) -> (Option<u64>, bool) {
        match self.values.entry(key) {
            Entry::Occupied(mut entry) => {
                let previous = *entry.get();
                let next = previous & !mask | value & mask;
                if next == previous {
                    (Some(previous), false)
                } else {
                    entry.insert(next);
                    (Some(previous), true)
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(value & mask);
                self.order.push(key);
                (None, true)
            }
        }
    }
    /// `dict.update(other)`.
    pub fn update(&mut self, other: &Registers) {
        for (key, value) in other.iter() {
            self.set(key, value);
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = (u32, u64)> + '_ {
        self.order.iter().map(|k| (*k, self.values[k]))
    }
    pub fn len(&self) -> usize {
        self.order.len()
    }
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
    /// The value at `key`, which the caller has already required.
    pub fn at(&self, key: u32) -> u64 {
        self.values[&key]
    }
}
