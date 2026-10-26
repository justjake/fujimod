//! Observational corroboration, independent of name propagation policy.
use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::sync::RwLock;

#[derive(Debug, Default)]
pub(crate) struct InspectionCache {
    pub signatures: HashMap<(String, Architecture, [u8; 32]), usize>,
    pub strings: HashMap<(String, String), usize>,
    pub code: HashMap<(String, String), Vec<(u64, u64)>>,
    pub tables: RwLock<HashMap<usize, Vec<DataWitness>>>,
    pub anchors: RwLock<HashMap<usize, BTreeSet<[u8; 32]>>>,
    pub candidate_shapes: RwLock<HashMap<String, HashMap<(Architecture, [u8; 32]), Vec<usize>>>>,
}
impl InspectionCache {
    pub fn new(functions: &[Function]) -> Self {
        let mut out = Self::default();
        for f in functions {
            *out.signatures
                .entry((f.key.firmware.clone(), f.architecture, f.signature))
                .or_default() += 1;
            for s in &f.strings {
                *out.strings
                    .entry((f.key.firmware.clone(), s.clone()))
                    .or_default() += 1;
            }
            out.code
                .entry((f.key.firmware.clone(), f.key.unit.clone()))
                .or_default()
                .extend(&f.ranges);
        }
        for ranges in out.code.values_mut() {
            ranges.sort_unstable();
            let mut merged: Vec<(u64, u64)> = Vec::new();
            for &(lo, hi) in ranges.iter() {
                if let Some(last) = merged.last_mut()
                    && lo <= last.1
                {
                    last.1 = last.1.max(hi);
                } else {
                    merged.push((lo, hi));
                }
            }
            *ranges = merged;
        }
        out
    }
    pub fn is_code(&self, key: &FunctionKey, a: u64) -> bool {
        [&key.unit, "main"].iter().any(|unit| {
            let Some(ranges) = self.code.get(&(key.firmware.clone(), unit.to_string())) else {
                return false;
            };
            let at = ranges.partition_point(|&(lo, _)| lo <= a);
            at > 0 && a < ranges[at - 1].1
        })
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NameSupport {
    pub corroborated: bool,
    pub exact_body_bytes: bool,
    pub shared_rare_strings: Vec<String>,
    pub independent_callee_pairs: usize,
    pub constant_tables: Vec<String>,
}
impl Index {
    pub(crate) fn anchor_signatures(&self, id: usize) -> BTreeSet<[u8; 32]> {
        if let Some(found) = self.inspection.anchors.read().unwrap().get(&id) {
            return found.clone();
        }
        let mut found = BTreeSet::new();
        if let Some(f) = self.functions.get(id) {
            for &(_, target) in &f.calls {
                if let Some(i) = self.callee(id, target) {
                    let c = &self.functions[i];
                    if c.instructions >= 4
                        && self.inspection.signatures.get(&(
                            c.key.firmware.clone(),
                            c.architecture,
                            c.signature,
                        )) == Some(&1)
                    {
                        found.insert(c.signature);
                    }
                }
            }
        }
        self.inspection
            .anchors
            .write()
            .unwrap()
            .insert(id, found.clone());
        found
    }
    /// Report a corroborating signal separate from equal-code or incoming-call
    /// nomination. This observation never blocks analysis names.
    pub fn name_support(&self, a: usize, b: usize) -> Option<NameSupport> {
        let (x, y) = (self.functions.get(a)?, self.functions.get(b)?);
        let mut out = NameSupport::default();
        if a == b {
            out.corroborated = true;
            return Some(out);
        }
        if self.membership.get(a).is_none() || self.membership.get(a) != self.membership.get(b) {
            return Some(out);
        }
        let same_arch = x.architecture == y.architecture;
        let normalized = same_arch && x.signature == y.signature;
        if normalized {
            out.exact_body_bytes = infallible(self.body_sha256(a))
                .is_some_and(|h| infallible(self.body_sha256(b)) == Some(h));
            out.shared_rare_strings = x
                .strings
                .intersection(&y.strings)
                .filter(|s| {
                    s.len() >= 8
                        && s.chars().filter(|c| c.is_alphabetic()).count() >= 4
                        && self
                            .inspection
                            .strings
                            .get(&(x.key.firmware.clone(), s.to_string()))
                            == Some(&1)
                        && self
                            .inspection
                            .strings
                            .get(&(y.key.firmware.clone(), s.to_string()))
                            == Some(&1)
                })
                .cloned()
                .collect();
        }
        if same_arch {
            out.independent_callee_pairs = self
                .anchor_signatures(a)
                .intersection(&self.anchor_signatures(b))
                .count();
        }
        let sufficient = if normalized {
            (out.exact_body_bytes
                && x.instructions >= 4
                && self.inspection.signatures.get(&(
                    x.key.firmware.clone(),
                    x.architecture,
                    x.signature,
                )) == Some(&1)
                && self.inspection.signatures.get(&(
                    y.key.firmware.clone(),
                    y.architecture,
                    y.signature,
                )) == Some(&1))
                || !out.shared_rare_strings.is_empty()
                || out.independent_callee_pairs > 0
        } else {
            same_arch && out.independent_callee_pairs >= 2
        };
        if !sufficient {
            let left: BTreeSet<_> = self
                .data_witnesses(a, 64)
                .into_iter()
                .map(|w| w.sha256)
                .collect();
            let right: BTreeSet<_> = self
                .data_witnesses(b, 64)
                .into_iter()
                .map(|w| w.sha256)
                .collect();
            out.constant_tables = left.intersection(&right).cloned().collect();
        }
        out.corroborated = sufficient || !out.constant_tables.is_empty();
        Some(out)
    }
}
