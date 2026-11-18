//! Sets of electrical levels a contact may present, ordered as integers.
use std::cmp::Ordering;

/// A set of levels, bit `n` meaning level `n` is allowed. Two masks compare
/// as the unsigned integers their bits spell, so ties between equally good
/// electrical realizations break the same way on every host.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Mask(Vec<u64>);

impl Mask {
    fn normalized(mut words: Vec<u64>) -> Self {
        while words.last() == Some(&0) {
            words.pop();
        }
        Self(words)
    }

    pub fn single(level: u32) -> Self {
        Self::range(level, level)
    }

    /// Levels `low..=high`.
    pub fn range(low: u32, high: u32) -> Self {
        if low > high {
            return Self::default();
        }
        let mut words = vec![0u64; (high / 64 + 1) as usize];
        for level in low..=high {
            words[(level / 64) as usize] |= 1 << (level % 64);
        }
        Self::normalized(words)
    }

    /// Every level of a domain of `size` values.
    pub fn full(size: u32) -> Self {
        if size == 0 {
            Self::default()
        } else {
            Self::range(0, size - 1)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn contains(&self, level: u64) -> bool {
        let word = (level / 64) as usize;
        word < self.0.len() && self.0[word] & (1 << (level % 64)) != 0
    }

    fn zip(&self, other: &Self, operation: impl Fn(u64, u64) -> u64) -> Self {
        let length = self.0.len().max(other.0.len());
        let word = |mask: &Self, index: usize| mask.0.get(index).copied().unwrap_or(0);
        Self::normalized(
            (0..length)
                .map(|index| operation(word(self, index), word(other, index)))
                .collect(),
        )
    }

    pub fn union(&self, other: &Self) -> Self {
        self.zip(other, |a, b| a | b)
    }

    pub fn intersection(&self, other: &Self) -> Self {
        self.zip(other, |a, b| a & b)
    }

    pub fn difference(&self, other: &Self) -> Self {
        self.zip(other, |a, b| a & !b)
    }

    /// Symmetric difference: with a full domain, the complement within it.
    pub fn xor(&self, other: &Self) -> Self {
        self.zip(other, |a, b| a ^ b)
    }

    /// The level to present: `current` when allowed, otherwise the middle of
    /// the first longest run of allowed levels below `limit`.
    pub fn level(&self, current: u64, limit: u64) -> Option<u64> {
        if current < limit && self.contains(current) {
            return Some(current);
        }
        let mut longest: Option<(u64, u64)> = None;
        let mut start = None;
        for value in 0..=limit {
            if value < limit && self.contains(value) {
                start.get_or_insert(value);
            } else if let Some(first) = start.take() {
                let size = value - first;
                if longest.is_none_or(|(best, _)| size > best) {
                    longest = Some((size, first));
                }
            }
        }
        longest.map(|(size, first)| first + (size - 1) / 2)
    }
}

impl Ord for Mask {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.iter().rev().cmp(other.0.iter().rev()))
    }
}

impl PartialOrd for Mask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::Mask;

    #[test]
    fn masks_compare_as_integers() {
        assert!(Mask::single(3) > Mask::range(0, 2));
        assert!(Mask::single(64) > Mask::full(64));
        assert!(Mask::range(1, 2) > Mask::range(0, 1));
        assert_eq!(
            Mask::range(0, 3).intersection(&Mask::range(2, 5)),
            Mask::range(2, 3)
        );
        assert_eq!(
            Mask::full(4).xor(&Mask::single(1)),
            Mask::single(0).union(&Mask::range(2, 3))
        );
        assert!(
            Mask::range(70, 80)
                .difference(&Mask::range(70, 80))
                .is_empty()
        );
    }

    #[test]
    fn the_level_keeps_an_allowed_current_value_or_takes_the_longest_run_middle() {
        let mask = Mask::range(2, 3).union(&Mask::range(10, 19));
        assert_eq!(mask.level(11, 1024), Some(11));
        assert_eq!(mask.level(0, 1024), Some(14));
        assert_eq!(
            Mask::range(0, 1).union(&Mask::range(4, 5)).level(3, 8),
            Some(0)
        );
        assert_eq!(Mask::default().level(0, 8), None);
    }
}
