//! Passive byte counts and medium-offset ranges for storage transfers.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageReads {
    pub bytes: u64,
    /// Sorted, disjoint half-open ranges; adjacent ranges are merged.
    pub ranges: Vec<(u64, u64)>,
}

impl StorageReads {
    pub fn record(&mut self, offset: u64, size: u64, capacity: u64) -> Result<(), String> {
        let mut end = offset
            .checked_add(size)
            .filter(|&end| end <= capacity)
            .ok_or("Storage read exceeds medium")?;
        let bytes = self
            .bytes
            .checked_add(size)
            .ok_or("Storage read count overflow")?;
        if size == 0 {
            return Ok(());
        }
        let mut start = offset;
        let first = self.ranges.partition_point(|&(_, hi)| hi < start);
        let mut last = first;
        while last < self.ranges.len() && self.ranges[last].0 <= end {
            start = start.min(self.ranges[last].0);
            end = end.max(self.ranges[last].1);
            last += 1;
        }
        self.ranges.splice(first..last, [(start, end)]);
        self.bytes = bytes;
        Ok(())
    }

    pub fn validate(&self, capacity: u64) -> Result<(), String> {
        let mut previous = None;
        let mut unique = 0u64;
        for &(start, end) in &self.ranges {
            if start >= end || end > capacity || previous.is_some_and(|p| p >= start) {
                return Err("Invalid storage read ranges".into());
            }
            unique = unique
                .checked_add(end - start)
                .ok_or("Storage read range count overflow")?;
            previous = Some(end);
        }
        if unique > self.bytes || (self.ranges.is_empty() && self.bytes != 0) {
            return Err("Invalid storage read count".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_adjacent_repeated_and_disjoint_transfers_keep_exact_counts() {
        let mut reads = StorageReads::default();
        for (start, size) in [(20, 4), (0, 4), (16, 4), (4, 12), (0, 24), (80, 2), (82, 0)] {
            reads.record(start, size, 128).unwrap();
        }
        assert_eq!(reads.ranges, [(0, 24), (80, 82)]);
        assert_eq!(reads.bytes, 50);
        reads.validate(128).unwrap();
        let saved = serde_json::to_value(&reads).unwrap();
        let restored: StorageReads = serde_json::from_value(saved).unwrap();
        assert_eq!(restored, reads);
        let before = reads.clone();
        assert!(reads.record(127, 2, 128).is_err());
        assert!(reads.record(u64::MAX, 1, u64::MAX).is_err());
        assert_eq!(reads, before);
    }

    #[test]
    fn invalid_checkpoint_ranges_or_counts_refuse() {
        for (bytes, ranges) in [
            (1, vec![]),
            (0, vec![(0, 1)]),
            (1, vec![(4, 4)]),
            (8, vec![(0, 4), (4, 8)]),
            (9, vec![(0, 5), (4, 8)]),
            (10, vec![(0, 129)]),
        ] {
            assert!(StorageReads { bytes, ranges }.validate(128).is_err());
        }
    }
}
