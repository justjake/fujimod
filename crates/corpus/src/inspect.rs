//! Reproducible samples and independent data witnesses for index inspection.
use crate::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BinaryHeap};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinkSample {
    pub left: FunctionView,
    pub right: FunctionView,
    pub pair: Pair,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SampleGroup {
    pub population: usize,
    pub links: Vec<LinkSample>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DataWitness {
    pub address: u64,
    pub length: usize,
    pub sha256: String,
    pub distinct_words: usize,
}
impl Index {
    /// Bottom-k hash sampling is stable across iteration order and index rebuilds.
    pub fn sample_links(&self, count: usize, seed: u64) -> BTreeMap<String, SampleGroup> {
        let mut groups = BTreeMap::<String, (usize, BinaryHeap<([u8; 32], usize, usize)>)>::new();
        let mut sample = |a: usize, b: usize, grade: &str| {
            let (x, y) = (&self.functions[a], &self.functions[b]);
            let group = groups.entry(grade.into()).or_default();
            group.0 += 1;
            let mut hash = Sha256::new();
            hash.update(seed.to_le_bytes());
            for key in [&x.key, &y.key] {
                hash.update(key.firmware.as_bytes());
                hash.update([0]);
                hash.update(key.unit.as_bytes());
                hash.update(key.address.to_le_bytes());
            }
            let rank: [u8; 32] = hash.finalize().into();
            if count > 0
                && (group.1.len() < count
                    || group.1.peek().is_some_and(|&(worst, _, _)| rank < worst))
            {
                group.1.push((rank, a, b));
                if group.1.len() > count {
                    group.1.pop();
                }
            }
        };
        for family in &self.families {
            for (p, &a) in family.members.iter().enumerate() {
                for &b in &family.members[p + 1..] {
                    let grade = if self.functions[a].architecture != self.functions[b].architecture
                    {
                        "cross-isa"
                    } else {
                        match self.grade_ids(a, b) {
                            Grade::Identical => "identical",
                            Grade::SameCode => "same-code",
                            Grade::CallSite => "call-site",
                            Grade::Weak => unreachable!(),
                        }
                    };
                    sample(a, b, grade);
                }
            }
        }
        for &(a, b) in &self.weak {
            if self.membership[a] != self.membership[b] {
                sample(a, b, "weak");
            }
        }
        groups
            .into_iter()
            .map(|(grade, (population, heap))| {
                let links = heap
                    .into_sorted_vec()
                    .into_iter()
                    .map(|(_, a, b)| LinkSample {
                        left: self.shown(a),
                        right: self.shown(b),
                        pair: if grade == "weak" {
                            self.weak_pair(a, b)
                        } else {
                            self.pair(a, b)
                        },
                    })
                    .collect();
                (grade, SampleGroup { population, links })
            })
            .collect()
    }
    pub fn add_name(&mut self, id: usize, name: String, source: String) -> Result<()> {
        if name.is_empty() {
            return Err(Error::Invalid("empty name".into()));
        }
        let f = self
            .functions
            .get_mut(id)
            .ok_or_else(|| Error::Invalid("unknown function".into()))?;
        let annotation = Annotation {
            kind: "name-source".into(),
            text: name.clone(),
            source,
        };
        if !f.names.contains(&name) {
            f.names.push(name.clone());
        }
        if !f.annotations.contains(&annotation) {
            f.annotations.push(annotation.clone());
        }
        if let Some(&family) = self.membership.get(id)
            && !self.families[family].annotations.contains(&annotation)
        {
            self.families[family].annotations.push(annotation);
        }
        Ok(())
    }
    /// Fixed-size non-pointer tables are a witness separate from instruction normalization.
    pub fn data_witnesses(&self, id: usize, length: usize) -> Vec<DataWitness> {
        if length == 64
            && let Some(found) = self.inspection.tables.read().unwrap().get(&id)
        {
            return found.clone();
        }
        let Some(f) = self.functions.get(id) else {
            return vec![];
        };
        let Some(u) = self.unit(&f.key.firmware, &f.key.unit) else {
            return vec![];
        };
        if length < 16 || !length.is_multiple_of(4) || length > 4096 {
            return vec![];
        }
        let mut found = BTreeMap::new();
        for reference in &f.references {
            let a = reference.target;
            if self.inspection.is_code(&f.key, a) {
                continue;
            }
            let Some(bytes) = u.read(a, length) else {
                continue;
            };
            if bytes
                .iter()
                .filter(|&&b| b == 0 || (32..127).contains(&b))
                .count()
                > bytes.len() * 3 / 4
            {
                continue;
            }
            let words: Vec<_> = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&b| u32::from_le_bytes(b) as u64)
                .collect();
            let distinct = words
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            if distinct < 4
                || words.iter().filter(|&&v| u.pointer(v)).count() > words.len() / 4
                || words
                    .iter()
                    .filter(|&&v| v > 255 && v != u32::MAX as u64)
                    .count()
                    < 4
            {
                continue;
            }
            let sha256 = Sha256::digest(bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            found.insert(
                a,
                DataWitness {
                    address: a,
                    length,
                    sha256,
                    distinct_words: distinct,
                },
            );
        }
        let result: Vec<_> = found.into_values().collect();
        if length == 64 {
            self.inspection
                .tables
                .write()
                .unwrap()
                .insert(id, result.clone());
        }
        result
    }
}
