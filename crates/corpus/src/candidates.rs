//! Analysis nominations independent of family merging and mod-symbol policy.
use crate::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionCandidate {
    pub function: FunctionView,
    pub grade: Grade,
    pub origins: Vec<String>,
}

fn wide(mnemonic: &str, prefix: &str) -> bool {
    mnemonic.strip_prefix(prefix).is_some_and(|suffix| {
        matches!(
            suffix,
            "" | "eq"
                | "ne"
                | "hs"
                | "lo"
                | "mi"
                | "pl"
                | "vs"
                | "vc"
                | "hi"
                | "ls"
                | "ge"
                | "lt"
                | "gt"
                | "le"
                | "cs"
                | "cc"
                | "al"
        )
    })
}

impl Index {
    fn candidate_shape(&self, id: usize) -> [u8; 32] {
        let f = &self.functions[id];
        let mut hash = Sha256::new();
        for (offset, &token) in f.tokens.iter().enumerate() {
            let text = &self.tokens[token as usize];
            let (mnemonic, operands) = text.split_once(' ').unwrap_or((text, ""));
            let replacement = if f.architecture == Architecture::Arm32
                && (wide(mnemonic, "movw") || wide(mnemonic, "movt"))
                && operands.contains(',')
            {
                Some(format!(
                    "{mnemonic} {}, K",
                    operands.split(',').next().unwrap()
                ))
            } else if f.architecture == Architecture::Arm32
                && wide(mnemonic, "mov")
                && operands.contains(", #")
                && f.tokens.get(offset + 1).is_some_and(|&next| {
                    self.tokens[next as usize].starts_with(&format!("movt{} ", &mnemonic[3..]))
                })
            {
                // MOV-immediate and MOVW can both form the low half before
                // MOVT. Register/condition/data-use checks belong to the
                // verifier; this hash only nominates every matching function.
                Some(format!(
                    "movw{} {}, K",
                    &mnemonic[3..],
                    operands.split(',').next().unwrap()
                ))
            } else {
                None
            };
            hash.update(replacement.as_deref().unwrap_or(text).as_bytes());
            hash.update(b"\n");
        }
        hash.finalize().into()
    }

    /// Return all family, one-hop weak and relocation-shape nominations.
    ///
    /// Shape-only hits retain weak grade, do not merge families, and do not
    /// transfer names into the index. A caller must independently verify the
    /// instructions, references and ambiguity before using an address.
    pub fn candidates(&self, reference: usize, firmware: &str) -> Vec<FunctionCandidate> {
        let Some(source) = self.functions.get(reference) else {
            return vec![];
        };
        let mut found = BTreeMap::<usize, FunctionCandidate>::new();
        if let Some(&family) = self.membership.get(reference) {
            for &id in &self.families[family].members {
                if self.functions[id].key.firmware == firmware {
                    found.insert(
                        id,
                        FunctionCandidate {
                            function: self.shown(id),
                            grade: self.grade_ids(reference, id),
                            origins: vec!["family".into()],
                        },
                    );
                }
            }
            if let Some(adjacent) = self.weak_families.get(&family) {
                for &other in adjacent {
                    for &id in &self.families[other].members {
                        if self.functions[id].key.firmware == firmware {
                            found
                                .entry(id)
                                .or_insert_with(|| FunctionCandidate {
                                    function: self.shown(id),
                                    grade: Grade::Weak,
                                    origins: vec![],
                                })
                                .origins
                                .push("weak-link".into());
                        }
                    }
                }
            }
        }
        if source.instructions >= 4 {
            if !self
                .inspection
                .candidate_shapes
                .read()
                .unwrap()
                .contains_key(firmware)
            {
                let mut shapes = HashMap::<(Architecture, [u8; 32]), Vec<usize>>::new();
                for (id, f) in self
                    .functions
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.key.firmware == firmware && f.instructions >= 4)
                {
                    shapes
                        .entry((f.architecture, self.candidate_shape(id)))
                        .or_default()
                        .push(id);
                }
                self.inspection
                    .candidate_shapes
                    .write()
                    .unwrap()
                    .insert(firmware.into(), shapes);
            }
            let shapes = self.inspection.candidate_shapes.read().unwrap();
            if let Some(ids) =
                shapes[firmware].get(&(source.architecture, self.candidate_shape(reference)))
            {
                for &id in ids {
                    found
                        .entry(id)
                        .or_insert_with(|| FunctionCandidate {
                            function: self.shown(id),
                            grade: Grade::Weak,
                            origins: vec![],
                        })
                        .origins
                        .push("relocation-shape".into());
                }
            }
        }
        found.into_values().collect()
    }
}
