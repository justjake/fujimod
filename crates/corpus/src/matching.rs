use crate::*;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

// SequenceMatcher's no-junk longest-block alignment. Tie order is earliest A, then B.
pub(crate) fn blocks(a: &[u32], b: &[u32]) -> Vec<(usize, usize, usize)> {
    let mut positions = HashMap::<u32, Vec<usize>>::new();
    for (j, &t) in b.iter().enumerate() {
        positions.entry(t).or_default().push(j);
    }
    let mut pending = vec![(0, a.len(), 0, b.len())];
    let mut out = Vec::new();
    while let Some((al, ah, bl, bh)) = pending.pop() {
        let (mut bi, mut bj, mut bn) = (al, bl, 0);
        let mut previous = vec![0; b.len() + 1];
        let mut current = vec![0; b.len() + 1];
        for (i, &t) in a.iter().enumerate().take(ah).skip(al) {
            current.fill(0);
            if let Some(js) = positions.get(&t) {
                for &j in js.iter().skip_while(|&&j| j < bl).take_while(|&&j| j < bh) {
                    let n = 1 + previous[j];
                    current[j + 1] = n;
                    if n > bn {
                        bi = i + 1 - n;
                        bj = j + 1 - n;
                        bn = n;
                    }
                }
            }
            std::mem::swap(&mut previous, &mut current);
        }
        if bn > 0 {
            out.push((bi, bj, bn));
            if al < bi && bl < bj {
                pending.push((al, bi, bl, bj));
            }
            if bi + bn < ah && bj + bn < bh {
                pending.push((bi + bn, ah, bj + bn, bh));
            }
        }
    }
    out.sort_unstable();
    out
}
fn similarity(a: &Function, b: &Function) -> f64 {
    if a.tokens.is_empty() && b.tokens.is_empty() {
        return 1.;
    }
    let mut counts = HashMap::<u32, usize>::new();
    for t in &a.tokens {
        *counts.entry(*t).or_default() += 1;
    }
    let mut n = 0;
    for t in &b.tokens {
        if let Some(c) = counts.get_mut(t)
            && *c > 0
        {
            *c -= 1;
            n += 1;
        }
    }
    2. * n as f64 / (a.tokens.len() + b.tokens.len()) as f64
}
fn aligned(a: &Function, b: &Function) -> Vec<(u64, u64)> {
    // Instruction-token positions have no shared meaning across instruction sets.
    if a.architecture != b.architecture || a.calls.is_empty() || b.calls.is_empty() {
        return Vec::new();
    }
    if a.signature == b.signature {
        return a
            .calls
            .iter()
            .zip(&b.calls)
            .map(|(&(_, x), &(_, y))| (x, y))
            .collect();
    }
    if a.tokens.len().max(b.tokens.len()) > 6000 {
        return Vec::new();
    }
    let b_calls: HashMap<usize, u64> = b.calls.iter().copied().collect();
    let mut out = Vec::new();
    for (i, j, n) in blocks(&a.tokens, &b.tokens) {
        for &(p, x) in &a.calls {
            if p >= i
                && p < i + n
                && let Some(&y) = b_calls.get(&(j + p - i))
            {
                out.push((x, y));
            }
        }
    }
    out
}
struct Groups {
    parent: Vec<usize>,
    members: Vec<Vec<usize>>,
    firmwares: Vec<BTreeSet<String>>,
}
impl Groups {
    fn new(f: &[Function]) -> Self {
        Self {
            parent: (0..f.len()).collect(),
            members: (0..f.len()).map(|i| vec![i]).collect(),
            firmwares: f
                .iter()
                .map(|f| BTreeSet::from([f.key.firmware.clone()]))
                .collect(),
        }
    }
    fn root(&self, mut i: usize) -> usize {
        while self.parent[i] != i {
            i = self.parent[i];
        }
        i
    }
    fn join(&mut self, a: usize, b: usize) -> Vec<(usize, usize)> {
        let (mut a, mut b) = (self.root(a), self.root(b));
        if a == b || !self.firmwares[a].is_disjoint(&self.firmwares[b]) {
            return vec![];
        }
        if self.members[a].len() < self.members[b].len() {
            std::mem::swap(&mut a, &mut b);
        }
        let pairs = self.members[a]
            .iter()
            .flat_map(|&x| self.members[b].iter().map(move |&y| (x, y)))
            .collect();
        self.parent[b] = a;
        let members = std::mem::take(&mut self.members[b]);
        self.members[a].extend(members);
        let fws = std::mem::take(&mut self.firmwares[b]);
        self.firmwares[a].extend(fws);
        pairs
    }
}
impl Index {
    /// Rebuild from all supplied firmwares simultaneously, independent of input order.
    pub fn build(&mut self) {
        self.functions.sort_by(|a, b| a.key.cmp(&b.key));
        self.reindex();
        let mut groups = Groups::new(&self.functions);
        let mut signatures =
            BTreeMap::<(Architecture, [u8; 32]), BTreeMap<&str, Vec<usize>>>::new();
        for (i, f) in self.functions.iter().enumerate() {
            if f.instructions >= 4 {
                signatures
                    .entry((f.architecture, f.signature))
                    .or_default()
                    .entry(&f.key.firmware)
                    .or_default()
                    .push(i);
            }
        }
        let mut frontier = Vec::new();
        for by_fw in signatures.values() {
            let unique: Vec<_> = by_fw
                .values()
                .filter(|v| v.len() == 1)
                .map(|v| v[0])
                .collect();
            if let Some((&first, rest)) = unique.split_first() {
                for &i in rest {
                    frontier.extend(groups.join(first, i));
                }
            }
        }
        // Feature seeds require both a rare referenced string and high n-gram agreement.
        // Cross-architecture seeds require two strings and two nontrivial constants.
        let mut strings = BTreeMap::<&str, Vec<usize>>::new();
        for (i, f) in self.functions.iter().enumerate() {
            for s in &f.strings {
                strings.entry(s).or_default().push(i);
            }
        }
        let mut candidates = BTreeSet::new();
        for ids in strings.values().filter(|v| v.len() <= 32) {
            for (p, &a) in ids.iter().enumerate() {
                for &b in &ids[p + 1..] {
                    if self.functions[a].key.firmware != self.functions[b].key.firmware {
                        candidates.insert((a, b));
                    }
                }
            }
        }
        let mut feature_votes = BTreeMap::<(usize, String), BTreeSet<usize>>::new();
        for (a, b) in candidates {
            let (x, y) = (&self.functions[a], &self.functions[b]);
            let strings = x.strings.intersection(&y.strings).count();
            let constants = x
                .constants
                .intersection(&y.constants)
                .filter(|&&v| v > 255)
                .count();
            let union = x.ngrams.union(&y.ngrams).count();
            let gram = x.ngrams.intersection(&y.ngrams).count() as f64 / union.max(1) as f64;
            let qualifies = if x.architecture == y.architecture {
                x.instructions >= 8 && y.instructions >= 8 && gram >= 0.85 && constants >= 2
            } else {
                strings >= 2
                    && constants >= 2
                    && strings * 4 >= x.strings.len()
                    && strings * 4 >= y.strings.len()
            };
            if qualifies {
                feature_votes
                    .entry((a, y.key.firmware.clone()))
                    .or_default()
                    .insert(b);
                feature_votes
                    .entry((b, x.key.firmware.clone()))
                    .or_default()
                    .insert(a);
            }
        }
        for ((a, _), bs) in &feature_votes {
            if bs.len() != 1 {
                continue;
            }
            let b = *bs.first().unwrap();
            if feature_votes
                .get(&(b, self.functions[*a].key.firmware.clone()))
                .is_some_and(|v| v.len() == 1 && v.contains(a))
            {
                frontier.extend(groups.join(*a, b));
            }
        }
        let mut seen = HashSet::new();
        let mut weak = BTreeMap::<(usize, usize), BTreeSet<usize>>::new();
        let mut all_votes = BTreeMap::<(usize, String), BTreeSet<usize>>::new();
        while !frontier.is_empty() {
            let mut votes = BTreeMap::<(usize, String), BTreeSet<usize>>::new();
            let mut sources = BTreeMap::<(usize, usize), BTreeSet<usize>>::new();
            for (a, b) in std::mem::take(&mut frontier) {
                if !seen.insert((a.min(b), a.max(b))) {
                    continue;
                }
                for (x, y) in aligned(&self.functions[a], &self.functions[b]) {
                    let (Some(x), Some(y)) = (self.callee(a, x), self.callee(b, y)) else {
                        continue;
                    };
                    if groups.root(x) == groups.root(y) {
                        continue;
                    }
                    sources.entry((x.min(y), x.max(y))).or_default().insert(a);
                    all_votes
                        .entry((x, self.functions[y].key.firmware.clone()))
                        .or_default()
                        .insert(y);
                    all_votes
                        .entry((y, self.functions[x].key.firmware.clone()))
                        .or_default()
                        .insert(x);
                    votes
                        .entry((x, self.functions[y].key.firmware.clone()))
                        .or_default()
                        .insert(y);
                    votes
                        .entry((y, self.functions[x].key.firmware.clone()))
                        .or_default()
                        .insert(x);
                }
            }
            for ((a, _), bs) in &votes {
                if bs.len() != 1 {
                    continue;
                }
                let b = *bs.first().unwrap();
                if !votes
                    .get(&(b, self.functions[*a].key.firmware.clone()))
                    .is_some_and(|v| v.len() == 1 && v.contains(a))
                {
                    continue;
                }
                let (x, y) = (&self.functions[*a], &self.functions[b]);
                if similarity(x, y) < 0.3
                    || x.instructions.max(y.instructions)
                        > 4 * x.instructions.min(y.instructions).max(1)
                {
                    let key = ((*a).min(b), (*a).max(b));
                    weak.entry(key).or_default().extend(&sources[&key]);
                    continue;
                }
                frontier.extend(groups.join(*a, b));
            }
        }
        // Dissimilar callees need two separate caller families, with no conflicting
        // nomination in any discovery round. Repeating a call in one body is one vote.
        self.weak = weak
            .into_iter()
            .filter_map(|((a, b), callers)| {
                let independent: BTreeSet<_> =
                    callers.into_iter().map(|i| groups.root(i)).collect();
                (independent.len() >= 2
                    && all_votes[&(a, self.functions[b].key.firmware.clone())].len() == 1
                    && all_votes[&(b, self.functions[a].key.firmware.clone())].len() == 1)
                    .then_some((a, b))
            })
            .collect();
        self.families.clear();
        self.membership = vec![0; self.functions.len()];
        let mut roots = BTreeMap::new();
        for i in 0..self.functions.len() {
            let next = self.families.len();
            let family = *roots.entry(groups.root(i)).or_insert_with(|| {
                self.families.push(Family {
                    id: next,
                    members: vec![],
                    annotations: vec![],
                });
                next
            });
            self.membership[i] = family;
            self.families[family].members.push(i);
            self.families[family]
                .annotations
                .extend(self.functions[i].annotations.clone());
        }
        self.reindex_weak();
    }
    pub fn pairs(&self, reference: &str, other: &str) -> Vec<Pair> {
        let mut out = Vec::new();
        for family in &self.families {
            let left = family
                .members
                .iter()
                .copied()
                .find(|&i| self.functions[i].key.firmware == reference);
            let right = family
                .members
                .iter()
                .copied()
                .find(|&i| self.functions[i].key.firmware == other);
            if let (Some(a), Some(b)) = (left, right) {
                out.push(self.pair(a, b));
            }
        }
        // Weak links remain separate from family membership and call propagation.
        for &(a, b) in &self.weak {
            let (a, b) = if self.functions[a].key.firmware == reference
                && self.functions[b].key.firmware == other
            {
                (a, b)
            } else if self.functions[b].key.firmware == reference
                && self.functions[a].key.firmware == other
            {
                (b, a)
            } else {
                continue;
            };
            if !out
                .iter()
                .any(|p| p.reference == self.functions[a].key || p.other == self.functions[b].key)
            {
                out.push(Pair {
                    reference: self.functions[a].key.clone(),
                    other: self.functions[b].key.clone(),
                    grade: Grade::Weak,
                    similarity: similarity(&self.functions[a], &self.functions[b]),
                });
            }
        }
        out.sort_by(|a, b| a.reference.cmp(&b.reference));
        out
    }
    pub(crate) fn weak_pair(&self, a: usize, b: usize) -> Pair {
        Pair {
            reference: self.functions[a].key.clone(),
            other: self.functions[b].key.clone(),
            grade: Grade::Weak,
            similarity: similarity(&self.functions[a], &self.functions[b]),
        }
    }
    pub(crate) fn pair(&self, a: usize, b: usize) -> Pair {
        let (x, y) = (&self.functions[a], &self.functions[b]);
        Pair {
            reference: x.key.clone(),
            other: y.key.clone(),
            grade: self.grade_ids(a, b),
            similarity: similarity(x, y),
        }
    }
}
