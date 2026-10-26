//! Queries shared by the in-memory `Index` and the SQLite `Store`. Each query
//! is written once over a small set of data-access primitives.
use crate::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

/// One function's header and family, borrowed in memory or owned from a store.
#[derive(Clone, Debug)]
pub struct Entry<'a> {
    pub id: usize,
    /// `None` until the index is built.
    pub family: Option<usize>,
    pub function: Cow<'a, FunctionHeader>,
}
impl Entry<'_> {
    pub fn view(&self) -> FunctionView {
        let f = &self.function;
        FunctionView {
            id: self.id,
            family: self.family.unwrap_or(self.id),
            key: f.key.clone(),
            names: f.names.clone(),
            instructions: f.instructions,
            signature: f.signature.iter().map(|b| format!("{b:02x}")).collect(),
            name_donors: vec![],
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub functions: usize,
    pub families: usize,
    /// Function count of every firmware with at least one function.
    pub firmwares: BTreeMap<String, usize>,
}

/// Non-empty bytes with an optional equal-length bit mask.
#[derive(Clone, Debug)]
pub struct BytePattern {
    bytes: Vec<u8>,
    mask: Option<Vec<u8>>,
}
impl BytePattern {
    pub fn new(bytes: Vec<u8>, mask: Option<Vec<u8>>) -> Result<Self> {
        if bytes.is_empty() || mask.as_ref().is_some_and(|m| m.len() != bytes.len()) {
            return Err(Error::Invalid(
                "empty pattern or mask length mismatch".into(),
            ));
        }
        Ok(Self { bytes, mask })
    }
    fn matches(&self, window: &[u8]) -> bool {
        window
            .iter()
            .zip(&self.bytes)
            .enumerate()
            .all(|(j, (&a, &b))| (a ^ b) & self.mask.as_ref().map_or(255, |m| m[j]) == 0)
    }
}

pub trait Lookup {
    type Error: std::fmt::Display;

    fn stats(&self) -> std::result::Result<Stats, Self::Error>;
    fn entry(&self, id: usize) -> std::result::Result<Option<Entry<'_>>, Self::Error>;
    /// Functions matching every given key part, in id order.
    fn select(
        &self,
        firmware: Option<&str>,
        unit: Option<&str>,
        address: Option<u64>,
    ) -> std::result::Result<Vec<Entry<'_>>, Self::Error>;
    /// Functions carrying `name`, in id order.
    fn named(&self, name: &str) -> std::result::Result<Vec<usize>, Self::Error>;
    fn family(&self, family: usize) -> std::result::Result<Option<Cow<'_, Family>>, Self::Error>;
    /// Families joined to `family` by a weak edge, in id order.
    fn weak_families(&self, family: usize) -> std::result::Result<Vec<usize>, Self::Error>;
    /// Calls resolve first in the caller's unit, then at a unique address in its firmware.
    fn resolve(
        &self,
        caller: &FunctionKey,
        address: u64,
    ) -> std::result::Result<Option<usize>, Self::Error>;
    fn function(&self, id: usize) -> std::result::Result<Option<Cow<'_, Function>>, Self::Error>;
    fn token(&self, token: u32) -> std::result::Result<String, Self::Error>;
    /// Units of `firmware` (optionally one unit) in input order.
    fn units(
        &self,
        firmware: &str,
        unit: Option<&str>,
    ) -> std::result::Result<Vec<Arc<Unit>>, Self::Error>;
    /// `(string, function id)` for every function referencing one of `strings`.
    fn string_holders(
        &self,
        strings: &BTreeSet<String>,
    ) -> std::result::Result<Vec<(String, usize)>, Self::Error>;
    /// References to `address` from functions of `firmware` (optionally one unit), in id order.
    fn referrers(
        &self,
        firmware: &str,
        unit: Option<&str>,
        address: u64,
    ) -> std::result::Result<Vec<(usize, Reference)>, Self::Error>;
    fn symbols(
        &self,
        name: Option<&str>,
        firmware: Option<&str>,
        unit: Option<&str>,
    ) -> std::result::Result<Vec<DataSymbol>, Self::Error>;

    fn view(&self, id: usize) -> std::result::Result<Option<FunctionView>, Self::Error> {
        Ok(self.entry(id)?.map(|e| e.view()))
    }
    /// Without a name, select by key parts. A name selects its carriers, their
    /// family members and other-firmware members of weak-linked families, each
    /// with the donors of that name.
    fn find(
        &self,
        name: Option<&str>,
        address: Option<u64>,
        firmware: Option<&str>,
        unit: Option<&str>,
    ) -> std::result::Result<Vec<FunctionView>, Self::Error> {
        let Some(name) = name else {
            return Ok(self
                .select(firmware, unit, address)?
                .iter()
                .map(Entry::view)
                .collect());
        };
        let mut found = BTreeSet::new();
        for donor in self.named(name)? {
            found.insert(donor);
            let Some(entry) = self.entry(donor)? else {
                continue;
            };
            let Some(family) = entry.family else {
                continue;
            };
            if let Some(members) = self.family(family)? {
                found.extend(&members.members);
            }
            for other in self.weak_families(family)? {
                let Some(members) = self.family(other)? else {
                    continue;
                };
                for &id in &members.members {
                    if self
                        .entry(id)?
                        .is_some_and(|e| e.function.key.firmware != entry.function.key.firmware)
                    {
                        found.insert(id);
                    }
                }
            }
        }
        let mut out = Vec::new();
        for id in found {
            let Some(entry) = self.entry(id)? else {
                continue;
            };
            let key = &entry.function.key;
            if firmware.is_none_or(|x| key.firmware == x)
                && unit.is_none_or(|x| key.unit == x)
                && address.is_none_or(|x| key.address == x)
            {
                let mut view = entry.view();
                view.name_donors = self
                    .inherited_names(id, None, false)?
                    .remove(name)
                    .unwrap_or_default();
                out.push(view);
            }
        }
        Ok(out)
    }
    fn members(
        &self,
        family: usize,
        firmware: Option<&str>,
    ) -> std::result::Result<Vec<FunctionView>, Self::Error> {
        let mut out = Vec::new();
        if let Some(family) = self.family(family)? {
            for &id in &family.members {
                if let Some(entry) = self.entry(id)?
                    && firmware.is_none_or(|x| entry.function.key.firmware == x)
                {
                    out.push(entry.view());
                }
            }
        }
        Ok(out)
    }
    /// `identical` and `same-code` need equal normalized code; `identical`
    /// also needs every call to resolve into the same family on both sides.
    fn grade(&self, a: &Entry, b: &Entry) -> std::result::Result<Grade, Self::Error> {
        let (x, y) = (&a.function, &b.function);
        if x.architecture != y.architecture || x.signature != y.signature {
            return Ok(Grade::CallSite);
        }
        let calls: HashMap<usize, u64> = y.calls.iter().copied().collect();
        for &(p, target) in &x.calls {
            let i = self.resolve(&x.key, target)?;
            let j = match calls.get(&p) {
                Some(&t) => self.resolve(&y.key, t)?,
                None => None,
            };
            let (Some(i), Some(j)) = (i, j) else {
                return Ok(Grade::SameCode);
            };
            let family = |id| -> std::result::Result<Option<usize>, Self::Error> {
                Ok(self.entry(id)?.and_then(|e| e.family))
            };
            let left = family(i)?;
            if left.is_none() || left != family(j)? {
                return Ok(Grade::SameCode);
            }
        }
        Ok(Grade::Identical)
    }
    /// Analysis names carry their donor and grade. A weak edge exposes names
    /// from the adjacent family; weak edges never merge families or chain.
    fn inherited_names(
        &self,
        id: usize,
        donors: Option<&BTreeSet<String>>,
        normalized_only: bool,
    ) -> std::result::Result<BTreeMap<String, Vec<NameDonor>>, Self::Error> {
        let mut names = BTreeMap::<String, Vec<NameDonor>>::new();
        let Some(target) = self.entry(id)? else {
            return Ok(names);
        };
        let Some(family) = target.family else {
            return Ok(names);
        };
        let mut candidates = Vec::new();
        if let Some(members) = self.family(family)? {
            for &i in &members.members {
                let Some(member) = self.entry(i)? else {
                    continue;
                };
                let grade = if i == id {
                    Grade::Identical
                } else {
                    self.grade(&member, &target)?
                };
                candidates.push((member, grade));
            }
        }
        if !normalized_only {
            for other in self.weak_families(family)? {
                let Some(members) = self.family(other)? else {
                    continue;
                };
                for &i in &members.members {
                    if let Some(member) = self.entry(i)?
                        && member.function.key.firmware != target.function.key.firmware
                    {
                        candidates.push((member, Grade::Weak));
                    }
                }
            }
        }
        candidates.sort_by_key(|(e, _)| e.id);
        candidates.dedup_by_key(|(e, _)| e.id);
        for (member, grade) in candidates {
            let f = &member.function;
            if (normalized_only && !grade.normalized())
                || !donors.is_none_or(|d| d.contains(&f.key.firmware))
            {
                continue;
            }
            for name in &f.names {
                names.entry(name.clone()).or_default().push(NameDonor {
                    key: f.key.clone(),
                    grade,
                    sources: f
                        .annotations
                        .iter()
                        .filter(|a| a.kind == "name-source" && a.text == *name)
                        .map(|a| a.source.clone())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect(),
                });
            }
        }
        Ok(names)
    }
    fn calls(
        &self,
        id: usize,
    ) -> std::result::Result<Vec<(u64, Option<FunctionView>)>, Self::Error> {
        let Some(entry) = self.entry(id)? else {
            return Ok(vec![]);
        };
        let mut out = Vec::new();
        for &(_, target) in &entry.function.calls {
            let callee = match self.resolve(&entry.function.key, target)? {
                Some(i) => self.view(i)?,
                None => None,
            };
            out.push((target, callee));
        }
        Ok(out)
    }
    fn data_references(&self, id: usize) -> std::result::Result<Vec<Reference>, Self::Error> {
        Ok(self
            .function(id)?
            .map(|f| f.references.clone())
            .unwrap_or_default())
    }
    fn references_to(
        &self,
        firmware: &str,
        unit: Option<&str>,
        address: u64,
    ) -> std::result::Result<Vec<(FunctionView, Reference)>, Self::Error> {
        let mut out = Vec::new();
        for (id, reference) in self.referrers(firmware, unit, address)? {
            if let Some(view) = self.view(id)? {
                out.push((view, reference));
            }
        }
        Ok(out)
    }
    fn normalized_tokens(&self, id: usize) -> std::result::Result<Vec<String>, Self::Error> {
        let Some(f) = self.function(id)? else {
            return Ok(vec![]);
        };
        f.tokens.iter().map(|&t| self.token(t)).collect()
    }
    fn read(
        &self,
        firmware: &str,
        unit: &str,
        address: u64,
        size: usize,
    ) -> std::result::Result<Option<Vec<u8>>, Self::Error> {
        Ok(self
            .units(firmware, Some(unit))?
            .first()
            .and_then(|u| u.read(address, size))
            .map(<[u8]>::to_vec))
    }
    /// Every masked byte-pattern match in the mapped units of one firmware.
    fn search(
        &self,
        firmware: &str,
        unit: Option<&str>,
        pattern: &BytePattern,
    ) -> std::result::Result<Vec<FunctionKey>, Self::Error> {
        let mut hits = Vec::new();
        for u in self.units(firmware, unit)? {
            for r in &u.regions {
                for (i, w) in r.bytes.windows(pattern.bytes.len()).enumerate() {
                    if pattern.matches(w) {
                        hits.push(FunctionKey {
                            firmware: firmware.into(),
                            unit: u.layout.unit.clone(),
                            address: r.address + i as u64,
                        });
                    }
                }
            }
        }
        Ok(hits)
    }
    /// Length-prefixed SHA-256 of a function's body bytes, range by range.
    fn body_sha256(&self, id: usize) -> std::result::Result<Option<String>, Self::Error> {
        let Some(entry) = self.entry(id)? else {
            return Ok(None);
        };
        let f = &entry.function;
        let units = self.units(&f.key.firmware, Some(&f.key.unit))?;
        let Some(u) = units.first() else {
            return Ok(None);
        };
        let mut hash = Sha256::new();
        for &(lo, hi) in &f.ranges {
            let Some(bytes) = u.read(lo, (hi - lo) as usize) else {
                return Ok(None);
            };
            hash.update((hi - lo).to_le_bytes());
            hash.update(bytes);
        }
        Ok(Some(
            hash.finalize().iter().map(|b| format!("{b:02x}")).collect(),
        ))
    }
    fn find_strings(
        &self,
        strings: &BTreeSet<String>,
        firmware: Option<&str>,
    ) -> std::result::Result<Vec<FunctionView>, Self::Error> {
        let ids: BTreeSet<usize> = self
            .string_holders(strings)?
            .into_iter()
            .map(|(_, id)| id)
            .collect();
        let mut out = Vec::new();
        for id in ids {
            if let Some(entry) = self.entry(id)?
                && firmware.is_none_or(|x| entry.function.key.firmware == x)
            {
                out.push(entry.view());
            }
        }
        Ok(out)
    }
    /// Per string, the number of functions referencing it in each firmware.
    fn string_frequencies(
        &self,
        strings: &BTreeSet<String>,
    ) -> std::result::Result<BTreeMap<String, BTreeMap<String, usize>>, Self::Error> {
        let mut counts = BTreeMap::<String, BTreeMap<String, usize>>::new();
        for (text, id) in self.string_holders(strings)? {
            if let Some(entry) = self.entry(id)? {
                *counts
                    .entry(text)
                    .or_default()
                    .entry(entry.function.key.firmware.clone())
                    .or_default() += 1;
            }
        }
        Ok(counts)
    }
}

impl Lookup for Index {
    type Error = std::convert::Infallible;

    fn stats(&self) -> std::result::Result<Stats, Self::Error> {
        let mut firmwares = BTreeMap::<String, usize>::new();
        for f in &self.functions {
            *firmwares.entry(f.key.firmware.clone()).or_default() += 1;
        }
        Ok(Stats {
            functions: self.functions.len(),
            families: self.families.len(),
            firmwares,
        })
    }
    fn entry(&self, id: usize) -> std::result::Result<Option<Entry<'_>>, Self::Error> {
        Ok((id < self.functions.len()).then(|| self.entry_at(id)))
    }
    fn select(
        &self,
        firmware: Option<&str>,
        unit: Option<&str>,
        address: Option<u64>,
    ) -> std::result::Result<Vec<Entry<'_>>, Self::Error> {
        Ok(self
            .functions
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                firmware.is_none_or(|x| f.key.firmware == x)
                    && unit.is_none_or(|x| f.key.unit == x)
                    && address.is_none_or(|x| f.key.address == x)
            })
            .map(|(i, _)| self.entry_at(i))
            .collect())
    }
    fn named(&self, name: &str) -> std::result::Result<Vec<usize>, Self::Error> {
        Ok(self
            .functions
            .iter()
            .enumerate()
            .filter(|(_, f)| f.names.iter().any(|n| n == name))
            .map(|(i, _)| i)
            .collect())
    }
    fn family(&self, family: usize) -> std::result::Result<Option<Cow<'_, Family>>, Self::Error> {
        Ok(self.families.get(family).map(Cow::Borrowed))
    }
    fn weak_families(&self, family: usize) -> std::result::Result<Vec<usize>, Self::Error> {
        Ok(self
            .weak_families
            .get(&family)
            .map(|v| v.iter().copied().collect())
            .unwrap_or_default())
    }
    fn resolve(
        &self,
        caller: &FunctionKey,
        address: u64,
    ) -> std::result::Result<Option<usize>, Self::Error> {
        Ok(self.resolve_key(caller, address))
    }
    fn function(&self, id: usize) -> std::result::Result<Option<Cow<'_, Function>>, Self::Error> {
        Ok(self.functions.get(id).map(Cow::Borrowed))
    }
    fn token(&self, token: u32) -> std::result::Result<String, Self::Error> {
        Ok(self.tokens[token as usize].clone())
    }
    fn units(
        &self,
        firmware: &str,
        unit: Option<&str>,
    ) -> std::result::Result<Vec<Arc<Unit>>, Self::Error> {
        Ok(self
            .units
            .iter()
            .filter(|u| u.layout.firmware == firmware && unit.is_none_or(|x| u.layout.unit == x))
            .cloned()
            .collect())
    }
    fn string_holders(
        &self,
        strings: &BTreeSet<String>,
    ) -> std::result::Result<Vec<(String, usize)>, Self::Error> {
        Ok(self
            .functions
            .iter()
            .enumerate()
            .flat_map(|(i, f)| f.strings.intersection(strings).map(move |s| (s.clone(), i)))
            .collect())
    }
    fn referrers(
        &self,
        firmware: &str,
        unit: Option<&str>,
        address: u64,
    ) -> std::result::Result<Vec<(usize, Reference)>, Self::Error> {
        Ok(self
            .functions
            .iter()
            .enumerate()
            .filter(|(_, f)| f.key.firmware == firmware && unit.is_none_or(|u| f.key.unit == u))
            .flat_map(|(i, f)| {
                f.references
                    .iter()
                    .filter(move |r| r.target == address)
                    .map(move |r| (i, r.clone()))
            })
            .collect())
    }
    fn symbols(
        &self,
        name: Option<&str>,
        firmware: Option<&str>,
        unit: Option<&str>,
    ) -> std::result::Result<Vec<DataSymbol>, Self::Error> {
        Ok(self
            .data_symbols
            .iter()
            .filter(|s| {
                name.is_none_or(|n| n == s.name)
                    && firmware.is_none_or(|f| f == s.firmware)
                    && unit.is_none_or(|u| u == s.unit)
            })
            .cloned()
            .collect())
    }
}
