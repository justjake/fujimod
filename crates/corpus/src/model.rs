use crate::{Entry, Lookup};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Architecture {
    Arm32,
    Aarch64,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FunctionKey {
    pub firmware: String,
    pub unit: String,
    pub address: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Region {
    pub address: u64,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Annotation {
    pub kind: String,
    pub text: String,
    pub source: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Boundary {
    pub address: u64,
    /// Half-open byte ranges, in execution/listing order.
    pub ranges: Vec<(u64, u64)>,
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub annotations: Vec<Annotation>,
    #[serde(default)]
    pub references: Vec<Reference>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reference {
    pub site: u64,
    pub target: u64,
    pub kind: String,
    #[serde(default)]
    pub text: Option<String>,
}
/// Identity and address policy of one mapped unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitLayout {
    pub firmware: String,
    pub unit: String,
    pub architecture: Architecture,
    /// Pointer windows, including aliases, supplied by the caller. No camera map is built in.
    pub address_ranges: Vec<(u64, u64)>,
    /// (alias start, exclusive end, mapped start), explicit one-hop mappings.
    #[serde(default)]
    pub address_aliases: Vec<(u64, u64, u64)>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnitInput {
    #[serde(flatten)]
    pub layout: UnitLayout,
    pub functions: Vec<Boundary>,
}
/// Boundaries are consumed by decoding; a unit keeps only its layout and bytes.
#[derive(Clone, Debug)]
pub struct Unit {
    pub layout: UnitLayout,
    pub regions: Vec<Region>,
}
impl Unit {
    pub fn canonical_address(&self, address: u64) -> u64 {
        self.layout
            .address_aliases
            .iter()
            .find_map(|&(lo, hi, target)| {
                (lo <= address && address < hi).then(|| target + (address - lo))
            })
            .unwrap_or(address)
    }
    pub fn read(&self, address: u64, size: usize) -> Option<&[u8]> {
        if self.layout.address_aliases.iter().any(|&(lo, hi, _)| {
            lo <= address
                && address < hi
                && address.checked_add(size as u64).is_none_or(|end| end > hi)
        }) {
            return None;
        }
        let address = self.canonical_address(address);
        self.regions.iter().find_map(|r| {
            let offset = usize::try_from(address.checked_sub(r.address)?).ok()?;
            r.bytes.get(offset..offset.checked_add(size)?)
        })
    }
    pub fn pointer(&self, value: u64) -> bool {
        self.layout
            .address_ranges
            .iter()
            .any(|&(lo, hi)| lo <= value && value < hi)
    }
}
/// Identity, geometry, normalized hash and calls: everything family queries
/// read without the matching features.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionHeader {
    pub key: FunctionKey,
    pub architecture: Architecture,
    pub names: Vec<String>,
    pub annotations: Vec<Annotation>,
    pub ranges: Vec<(u64, u64)>,
    pub signature: [u8; 32],
    pub instructions: usize,
    /// Token offset and absolute target. Resolution always keeps the unit identity.
    pub calls: Vec<(usize, u64)>,
}
/// A function's header plus the features used to match it. Header fields are
/// reachable directly through `Deref`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Function {
    #[serde(flatten)]
    pub header: FunctionHeader,
    pub tokens: Vec<u32>,
    pub references: Vec<Reference>,
    pub constants: BTreeSet<u64>,
    pub strings: BTreeSet<String>,
    pub ngrams: BTreeSet<u64>,
}
impl std::ops::Deref for Function {
    type Target = FunctionHeader;
    fn deref(&self) -> &FunctionHeader {
        &self.header
    }
}
impl std::ops::DerefMut for Function {
    fn deref_mut(&mut self) -> &mut FunctionHeader {
        &mut self.header
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Grade {
    Identical,
    SameCode,
    CallSite,
    Weak,
}
impl Grade {
    pub fn normalized(self) -> bool {
        matches!(self, Self::Identical | Self::SameCode)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NameDonor {
    #[serde(flatten)]
    pub key: FunctionKey,
    pub grade: Grade,
    pub sources: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pair {
    pub reference: FunctionKey,
    pub other: FunctionKey,
    pub grade: Grade,
    pub similarity: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Family {
    pub id: usize,
    pub members: Vec<usize>,
    pub annotations: Vec<Annotation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionView {
    pub id: usize,
    pub family: usize,
    pub key: FunctionKey,
    pub names: Vec<String>,
    pub instructions: usize,
    pub signature: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub name_donors: Vec<NameDonor>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataSymbol {
    pub firmware: String,
    pub unit: String,
    pub address: u64,
    pub length: usize,
    pub name: String,
    pub kind: String,
    pub source: String,
}
/// The complete in-memory index. Decoding, family building and graph-wide
/// analysis run here; `Store` persists it and answers lookups without loading.
#[derive(Clone, Debug, Default)]
pub struct Index {
    pub(crate) inspection: Arc<crate::quality::InspectionCache>,
    pub(crate) units: Vec<Arc<Unit>>,
    pub(crate) data_symbols: Vec<DataSymbol>,
    pub(crate) functions: Vec<Function>,
    pub(crate) tokens: Vec<String>,
    pub(crate) families: Vec<Family>,
    /// Family of each function; empty until `build`.
    pub(crate) membership: Vec<usize>,
    pub(crate) weak: Vec<(usize, usize)>,
    pub(crate) weak_families: HashMap<usize, BTreeSet<usize>>,
    pub(crate) intern: HashMap<String, u32>,
    pub(crate) keys: HashMap<FunctionKey, usize>,
    pub(crate) targets: HashMap<(String, u64), Vec<usize>>,
}
pub(crate) fn infallible<T>(result: std::result::Result<T, std::convert::Infallible>) -> T {
    let Ok(value) = result;
    value
}
impl Index {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn functions(&self) -> &[Function] {
        &self.functions
    }
    pub fn families(&self) -> &[Family] {
        &self.families
    }
    /// Unbuilt index over the given parts; callers build or attach families.
    pub(crate) fn from_parts(
        units: Vec<Arc<Unit>>,
        functions: Vec<Function>,
        data_symbols: Vec<DataSymbol>,
        tokens: Vec<String>,
    ) -> Self {
        Self {
            units,
            functions,
            data_symbols,
            tokens,
            ..Self::new()
        }
    }
    pub(crate) fn entry_at(&self, id: usize) -> Entry<'_> {
        Entry {
            id,
            family: self.membership.get(id).copied(),
            function: std::borrow::Cow::Borrowed(&self.functions[id].header),
        }
    }
    pub(crate) fn shown(&self, id: usize) -> FunctionView {
        self.entry_at(id).view()
    }
    pub(crate) fn grade_ids(&self, a: usize, b: usize) -> Grade {
        infallible(self.grade(&self.entry_at(a), &self.entry_at(b)))
    }
    pub(crate) fn unit(&self, firmware: &str, unit: &str) -> Option<&Unit> {
        self.units
            .iter()
            .find(|u| u.layout.firmware == firmware && u.layout.unit == unit)
            .map(|u| u.as_ref())
    }
    pub(crate) fn reindex(&mut self) {
        self.inspection =
            std::sync::Arc::new(crate::quality::InspectionCache::new(&self.functions));
        self.intern = self
            .tokens
            .iter()
            .enumerate()
            .map(|(i, s)| (s.clone(), i as u32))
            .collect();
        self.keys = self
            .functions
            .iter()
            .enumerate()
            .map(|(i, f)| (f.key.clone(), i))
            .collect();
        self.reindex_weak();
        self.targets.clear();
        for (i, f) in self.functions.iter().enumerate() {
            self.targets
                .entry((f.key.firmware.clone(), f.key.address))
                .or_default()
                .push(i);
        }
    }
    pub(crate) fn reindex_weak(&mut self) {
        self.weak_families.clear();
        for &(a, b) in &self.weak {
            if let (Some(&x), Some(&y)) = (self.membership.get(a), self.membership.get(b))
                && x != y
            {
                self.weak_families.entry(x).or_default().insert(y);
                self.weak_families.entry(y).or_default().insert(x);
            }
        }
    }
    pub fn add_unit(&mut self, input: UnitInput, mut regions: Vec<Region>) -> Result<()> {
        let UnitInput { layout, functions } = input;
        if layout.firmware.is_empty() || layout.unit.is_empty() {
            return Err(Error::Invalid("empty firmware/unit".into()));
        }
        if self.unit(&layout.firmware, &layout.unit).is_some() {
            return Err(Error::Invalid("duplicate firmware unit".into()));
        }
        regions.sort_by_key(|r| r.address);
        for r in &regions {
            r.address
                .checked_add(r.bytes.len() as u64)
                .ok_or_else(|| Error::Invalid("region overflow".into()))?;
        }
        if regions
            .windows(2)
            .any(|r| r[0].address + r[0].bytes.len() as u64 > r[1].address)
        {
            return Err(Error::Invalid("overlapping regions in one unit".into()));
        }
        let unit = Unit { layout, regions };
        let mut alias_ranges = unit.layout.address_aliases.clone();
        alias_ranges.sort_unstable();
        if alias_ranges.windows(2).any(|w| w[0].1 > w[1].0)
            || alias_ranges.iter().any(|&(lo, hi, target)| {
                hi <= lo
                    || lo == target
                    || target.checked_add(hi - lo).is_none()
                    || !unit.regions.iter().any(|r| {
                        r.address <= target
                            && target + (hi - lo) <= r.address + r.bytes.len() as u64
                    })
            })
        {
            return Err(Error::Invalid(
                "invalid or overlapping address aliases".into(),
            ));
        }
        let mut seen = BTreeSet::new();
        for b in &functions {
            if !seen.insert(b.address)
                || b.ranges.is_empty()
                || b.ranges
                    .iter()
                    .any(|&(lo, hi)| hi <= lo || unit.read(lo, (hi - lo) as usize).is_none())
            {
                return Err(Error::Invalid(format!(
                    "invalid/unmapped boundary {:#x}",
                    b.address
                )));
            }
        }
        let (decoded, local) = crate::decode::decode(&unit, &functions)?;
        let start = self.functions.len();
        self.functions.extend(decoded);
        let remap: Vec<u32> = local
            .into_iter()
            .map(|s| {
                let n = self.tokens.len() as u32;
                *self.intern.entry(s.clone()).or_insert_with(|| {
                    self.tokens.push(s);
                    n
                })
            })
            .collect();
        for f in &mut self.functions[start..] {
            for t in &mut f.tokens {
                *t = remap[*t as usize];
            }
        }
        self.units.push(Arc::new(unit));
        self.families.clear();
        self.membership.clear();
        self.weak.clear();
        self.reindex();
        Ok(())
    }
    /// Merge normalized indexes without decoding either corpus again.
    pub fn merge(&mut self, other: Self) -> Result<()> {
        if other
            .units
            .iter()
            .any(|u| self.unit(&u.layout.firmware, &u.layout.unit).is_some())
        {
            return Err(Error::Invalid("duplicate firmware unit in merge".into()));
        }
        if other
            .functions
            .iter()
            .any(|f| self.keys.contains_key(&f.key))
        {
            return Err(Error::Invalid("duplicate function in merge".into()));
        }
        let remap: Vec<u32> = other
            .tokens
            .into_iter()
            .map(|s| {
                let n = self.tokens.len() as u32;
                *self.intern.entry(s.clone()).or_insert_with(|| {
                    self.tokens.push(s);
                    n
                })
            })
            .collect();
        for mut f in other.functions {
            for t in &mut f.tokens {
                *t = remap[*t as usize];
            }
            self.functions.push(f);
        }
        self.units.extend(other.units);
        self.data_symbols.extend(other.data_symbols);
        self.build();
        Ok(())
    }
    /// Recompute families using only selected donor firmware, retaining original features.
    pub fn subset(&self, firmwares: &BTreeSet<String>) -> Self {
        let mut out = Self::from_parts(
            self.units
                .iter()
                .filter(|u| firmwares.contains(&u.layout.firmware))
                .cloned()
                .collect(),
            self.functions
                .iter()
                .filter(|f| firmwares.contains(&f.key.firmware))
                .cloned()
                .collect(),
            self.data_symbols
                .iter()
                .filter(|s| firmwares.contains(&s.firmware))
                .cloned()
                .collect(),
            self.tokens.clone(),
        );
        out.build();
        out
    }
    /// Calls resolve first in the caller's unit, then at a unique address in its firmware.
    pub(crate) fn resolve_key(&self, caller: &FunctionKey, address: u64) -> Option<usize> {
        self.keys
            .get(&FunctionKey {
                firmware: caller.firmware.clone(),
                unit: caller.unit.clone(),
                address,
            })
            .copied()
            .or_else(|| {
                let v = self.targets.get(&(caller.firmware.clone(), address))?;
                (v.len() == 1).then_some(v[0])
            })
    }
    pub(crate) fn callee(&self, caller: usize, address: u64) -> Option<usize> {
        self.resolve_key(&self.functions[caller].key, address)
    }
    pub fn add_symbol(&mut self, symbol: DataSymbol) -> Result<()> {
        if symbol.name.is_empty()
            || symbol.length == 0
            || self
                .unit(&symbol.firmware, &symbol.unit)
                .and_then(|u| u.read(symbol.address, symbol.length))
                .is_none()
        {
            return Err(Error::Invalid("invalid or unmapped data symbol".into()));
        }
        if !self.data_symbols.contains(&symbol) {
            self.data_symbols.push(symbol);
        }
        Ok(())
    }
    pub fn annotate(&mut self, family: usize, annotation: Annotation) -> Result<()> {
        let f = self
            .families
            .get_mut(family)
            .ok_or_else(|| Error::Invalid("unknown family".into()))?;
        let representative = f.members[0];
        f.annotations.push(annotation.clone());
        self.functions[representative].annotations.push(annotation);
        Ok(())
    }
    /// Write a SQLite store that `Store::open` reads without loading the index.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        crate::store::write(self, path.as_ref())
    }
}
