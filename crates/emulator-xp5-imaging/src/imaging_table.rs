//! XP5 imaging table window at 0xff96c000: write-only 32-bit words and one status word.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const BASE: u64 = 0xff96c000;
/// The status word; stock code reads it and keeps only bit 0.
pub const STATUS: u64 = 0xb30;

/// Word runs the stock firmware writes (inclusive start, inclusive end), one 32-bit store each.
const TABLE_RUNS: [(u64, u64); 9] = [
    (0x000, 0x00c),
    (0x108, 0x118),
    (0x200, 0x26c),
    (0x300, 0x314),
    (0x400, 0x4d0),
    (0x600, 0x61c),
    (0x634, 0x668),
    (0x700, 0x76c),
    (0xc00, 0xca0),
];

fn table(offset: u64) -> bool {
    offset % 4 == 0 && TABLE_RUNS.iter().any(|&(a, b)| (a..=b).contains(&offset))
}

/// Words latch what was written. The status word reads zero at reset and is not changed by
/// anything: the meaning of its bit and its silicon reset value are unverified. The only observed
/// reader discards the bit, so a constant zero satisfies it.
#[derive(Default, Serialize, Deserialize)]
pub struct Table {
    words: BTreeMap<u64, u32>,
}
impl Device for Table {
    fn name(&self) -> &'static str {
        "imaging-table"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(BASE);
        if size != 4 || !(offset == STATUS || table(offset)) {
            cx.unmodeled();
            return 0;
        }
        u64::from(self.words.get(&offset).copied().unwrap_or(0))
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(BASE);
        if size != 4 || !(offset == STATUS || table(offset)) {
            cx.unmodeled();
            return;
        }
        self.words.insert(offset, value as u32);
    }
    fujimod_emulator_devices::checkpoint_fields!(words: BTreeMap<u64, u32>);
    fn save(&self) -> Value {
        serde_json::to_value(self).unwrap()
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        *self = serde_json::from_value(value.clone())
            .map_err(|e| format!("imaging table checkpoint: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    #[test]
    fn the_status_word_reads_zero_and_the_read_gets_past() {
        let mut t = Table::default();
        let mut host = RecordingHost::default();
        assert_eq!(t.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + STATUS, 4), 0);
    }
    #[test]
    fn table_words_read_back() {
        let mut t = Table::default();
        let mut host = RecordingHost::default();
        for (o, v) in [(0x0, 0x10302), (0x108, 0x1000100), (0xca0, 5), (0x66c - 4, 9)] {
            t.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + o, 4, v);
            assert_eq!(t.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + o, 4), v);
        }
    }
    #[test]
    fn an_unlisted_word_is_unmodeled() {
        assert!(!table(0x10) && !table(0xb30) && !table(0x2) && !table(0x670) && !table(0xca4));
        assert!(table(0x668) && table(0xc00));
    }
}
