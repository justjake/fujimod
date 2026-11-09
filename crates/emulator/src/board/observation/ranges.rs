//! Instruction counts for half-open raw PC ranges.
pub struct RangeCounts {
    ranges: Vec<(u64, u64)>,
    sorted: Vec<usize>,
    max_end: Vec<u64>,
    counts: Vec<Vec<u64>>,
}

impl RangeCounts {
    pub fn new(ranges: Vec<(u64, u64)>, cores: usize) -> Result<Self, String> {
        if ranges
            .iter()
            .any(|&(start, end)| start >= end || end > (1u64 << 32))
        {
            return Err("Invalid instruction range".into());
        }
        let mut sorted: Vec<_> = (0..ranges.len()).collect();
        sorted.sort_unstable_by_key(|&i| (ranges[i].0, i));
        let mut result = Self {
            max_end: vec![0; ranges.len().saturating_mul(4)],
            counts: vec![vec![0; cores]; ranges.len()],
            ranges,
            sorted,
        };
        if !result.ranges.is_empty() {
            result.build(1, 0, result.ranges.len());
        }
        Ok(result)
    }
    fn build(&mut self, node: usize, lo: usize, hi: usize) -> u64 {
        let end = if hi - lo == 1 {
            self.ranges[self.sorted[lo]].1
        } else {
            let mid = (lo + hi) / 2;
            self.build(node * 2, lo, mid)
                .max(self.build(node * 2 + 1, mid, hi))
        };
        self.max_end[node] = end;
        end
    }
    pub fn observe(&mut self, core: usize, pc: u64) {
        if !self.ranges.is_empty() {
            self.visit(1, 0, self.ranges.len(), core, pc);
        }
    }
    fn visit(&mut self, node: usize, lo: usize, hi: usize, core: usize, pc: u64) {
        if self.max_end[node] <= pc || self.ranges[self.sorted[lo]].0 > pc {
            return;
        }
        if hi - lo == 1 {
            self.counts[self.sorted[lo]][core] += 1;
        } else {
            let mid = (lo + hi) / 2;
            self.visit(node * 2, lo, mid, core, pc);
            self.visit(node * 2 + 1, mid, hi, core, pc);
        }
    }
    pub fn snapshot(&self) -> Vec<(u64, u64, Vec<u64>)> {
        self.ranges
            .iter()
            .copied()
            .zip(&self.counts)
            .map(|((start, end), counts)| (start, end, counts.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlapping_large_ranges_and_many_ranges_count_per_core() {
        let mut ranges = vec![(0, 0x100000), (4, 8), (4, 8)];
        ranges.extend((0..10_000).map(|i| (0x200000 + i * 4, 0x200004 + i * 4)));
        let mut counts = RangeCounts::new(ranges, 2).unwrap();
        counts.observe(0, 4);
        counts.observe(1, 4);
        counts.observe(1, 8);
        counts.observe(0, 0x200000 + 9999 * 4);
        let rows = counts.snapshot();
        assert_eq!(rows[0].2, [1, 2]);
        assert_eq!(rows[1].2, [1, 1]);
        assert_eq!(rows[2].2, [1, 1]);
        assert_eq!(rows[10002].2, [1, 0]);
        assert!(RangeCounts::new(vec![(5, 5)], 2).is_err());
    }
}
