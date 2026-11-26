//! XP5 imaging table windows (0xff96c000 and kin): write-only 32-bit words and one status word.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Windows of this kind and the word runs the stock firmware writes in each (inclusive start and
/// end offsets), one 32-bit store each.
pub const BASES: [u64; 5] = [0xff96c000, 0xff96e000, 0xff959000, 0xff95a000, 0xff95b000];
/// The status word of the first window; stock code reads it and keeps only bit 0.
pub const STATUS: u64 = 0xb30;

const RUNS_C: [(u64, u64); 9] = [
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
const RUNS_E: [(u64, u64); 6] = [
    (0x000, 0x000),
    (0x100, 0x120),
    (0x200, 0x264),
    (0x400, 0x468),
    (0x500, 0x5ec),
    (0x700, 0x700),
];

// Three 0x400-byte tables back to back in each of the last two windows, nine 0x100-byte tables in
// the first; loops copy byte-packed words with one 32-bit store each and never read them back.
const RUNS_9: [(u64, u64); 1] = [(0x000, 0x8fc)];
const RUNS_A: [(u64, u64); 1] = [(0x000, 0xbfc)];

fn runs(base: u64) -> &'static [(u64, u64)] {
    match base {
        b if b == BASES[0] => &RUNS_C,
        b if b == BASES[1] => &RUNS_E,
        b if b == BASES[2] => &RUNS_9,
        _ => &RUNS_A,
    }
}
fn table(base: u64, offset: u64) -> bool {
    offset.is_multiple_of(4) && runs(base).iter().any(|&(a, b)| (a..=b).contains(&offset))
}

/// Words latch what was written. The status word reads zero at reset and is not changed by
/// anything: the meaning of its bit and its silicon reset value are unverified. The only observed
/// reader discards the bit, so a constant zero satisfies it.
#[derive(Default, Serialize, Deserialize)]
pub struct Table {
    #[serde(default)]
    base: u64,
    words: BTreeMap<u64, u32>,
}
impl Table {
    pub fn reset(base: u64) -> Option<Self> {
        BASES.contains(&base).then(|| Self {
            base,
            words: BTreeMap::new(),
        })
    }
    fn known(&self, offset: u64) -> bool {
        table(self.base, offset) || (self.base == BASES[0] && offset == STATUS)
    }
}
impl Device for Table {
    fn name(&self) -> &'static str {
        "imaging-table"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(self.base);
        if size != 4 || !self.known(offset) {
            cx.unmodeled();
            return 0;
        }
        u64::from(self.words.get(&offset).copied().unwrap_or(0))
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(self.base);
        if size != 4 || !self.known(offset) {
            cx.unmodeled();
            return;
        }
        self.words.insert(offset, value as u32);
    }
    fujimod_emulator_devices::checkpoint_fields!(base: u64, words: BTreeMap<u64, u32>);
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
    const C: u64 = BASES[0];
    const E: u64 = BASES[1];
    #[test]
    fn the_status_word_reads_zero_and_the_read_gets_past() {
        let mut t = Table::reset(C).unwrap();
        let mut host = RecordingHost::default();
        assert_eq!(t.read(&mut Cx::new(&mut host, 0, 0, 0), C + STATUS, 4), 0);
    }
    #[test]
    fn table_words_read_back() {
        for (base, o, v) in [
            (C, 0x0, 0x10302),
            (C, 0xca0, 5),
            (E, 0x0, 0x111121),
            (E, 0x5ec, 9),
        ] {
            let mut t = Table::reset(base).unwrap();
            let mut host = RecordingHost::default();
            t.write(&mut Cx::new(&mut host, 0, 0, 0), base + o, 4, v);
            assert_eq!(t.read(&mut Cx::new(&mut host, 0, 0, 0), base + o, 4), v);
        }
    }
    #[test]
    fn the_coefficient_windows_latch_their_runs_and_refuse_the_rest() {
        for (base, last, past) in [
            (BASES[2], 0x8fc, 0x900),
            (BASES[3], 0xbfc, 0xc00),
            (BASES[4], 0xbfc, 0xc00),
        ] {
            let mut t = Table::reset(base).unwrap();
            let mut host = RecordingHost::default();
            for o in [0, 0x100, last] {
                t.write(&mut Cx::new(&mut host, 0, 0, 0), base + o, 4, 0xa28065 + o);
                assert_eq!(
                    t.read(&mut Cx::new(&mut host, 0, 0, 0), base + o, 4),
                    0xa28065 + o
                );
            }
            assert!(table(base, last) && !table(base, past) && !table(base, 2));
        }
    }
    #[test]
    fn an_unlisted_word_is_unmodeled() {
        assert!(
            !table(C, 0x10)
                && !table(C, 0xb30)
                && !table(C, 0x2)
                && !table(C, 0x670)
                && !table(C, 0xca4)
        );
        assert!(table(C, 0x668) && table(C, 0xc00));
        assert!(!table(E, 0x4) && !table(E, 0x124) && table(E, 0x700));
        assert!(Table::reset(0xff96d000).is_none());
    }
}
