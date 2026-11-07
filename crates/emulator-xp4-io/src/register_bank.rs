//! X-Processor 4 peripheral register bank: the card-controller words the stock
//! driver reads and writes around its clock gates. Reads return the last write;
//! the clock words are modeled separately (`sdhci::Clocks`), so each window here
//! excludes them. No card protocol, DMA or interrupt is modeled.
use fujimod_emulator_devices::{Cx, Device, Record, Value as Field};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterBank {
    windows: Vec<(u64, u64)>,
    registers: BTreeMap<u64, u32>,
}
impl RegisterBank {
    pub fn new(windows: &[(u64, u64)]) -> Result<Self, String> {
        let mut sorted = windows.to_vec();
        sorted.sort_unstable();
        if sorted.is_empty()
            || sorted.iter().any(|&(lo, hi)| {
                lo & 1 != 0 || hi <= lo || hi > 1 << 32
            })
            || sorted.windows(2).any(|pair| pair[0].1 > pair[1].0)
        {
            return Err("Invalid or overlapping XP4 SD host window".into());
        }
        Ok(Self {
            windows: sorted,
            registers: BTreeMap::new(),
        })
    }
    fn owns(&self, address: u64, size: u64) -> bool {
        let end = address + size;
        self.windows
            .iter()
            .any(|&(lo, hi)| lo <= address && end <= hi)
    }
}
impl Device for RegisterBank {
    fn name(&self) -> &'static str {
        "xp4-register-bank"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if !(1..=4).contains(&size) || !self.owns(address, u64::from(size)) {
            cx.unmodeled();
            return 0;
        }
        let word = self.registers.get(&(address & !3)).copied().unwrap_or(0);
        if size == 4 {
            return u64::from(word);
        }
        let shift = (address & 3) * 8;
        (u64::from(word) >> shift) & ((1u64 << (u32::from(size) * 8)) - 1)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if !(1..=4).contains(&size) || !self.owns(address, u64::from(size)) {
            cx.unmodeled();
            return;
        }
        let base = address & !3;
        let merged = if size == 4 {
            value as u32
        } else {
            let shift = (address & 3) * 8;
            let mask = ((1u32 << (u32::from(size) * 8)) - 1) << shift;
            (self.registers.get(&base).copied().unwrap_or(0) & !mask)
                | (((value as u32) << shift) & mask)
        };
        if merged == 0 {
            self.registers.remove(&base);
        } else {
            self.registers.insert(base, merged);
        }
        cx.record(record("XP4 SD host write", address, merged));
    }
    fn save(&self) -> Value {
        json!(self)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: Self = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state.windows != self.windows
            || state
                .registers
                .keys()
                .any(|&address| !state.owns(address, 4))
            || state.registers.values().any(|&value| value == 0)
        {
            return Err("Invalid XP4 SD host checkpoint".into());
        }
        *self = state;
        Ok(())
    }
}
fn record(message: &'static str, address: u64, value: u32) -> Record {
    Record::new(
        message,
        [("address", Field::int(address)), ("value", Field::int(value))],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::Host;

    #[derive(Default)]
    struct Clock {
        records: Vec<Record>,
    }
    impl Host for Clock {
        fn now(&self) -> u64 {
            0
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {}
        fn set_irq(&mut self, _: u32, _: bool) {}
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            Err("no ram".into())
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            Err("no ram".into())
        }
        fn record(&mut self, record: Record) {
            self.records.push(record);
        }
    }

    #[test]
    fn words_read_back_their_last_write_and_bytes_merge() {
        let mut host = RegisterBank::new(&[
            (0xfffc4000, 0xfffc402c),
            (0xfffc402e, 0xfffc5000),
            (0xfffc6000, 0xfffc622c),
        ])
        .unwrap();
        let mut clock = Clock::default();
        host.write(&mut Cx::new(&mut clock, 0, 0, 0), 0xfffc6080, 4, 0xf00);
        assert_eq!(host.read(&mut Cx::new(&mut clock, 0, 0, 0), 0xfffc6080, 4), 0xf00);
        // An address outside every window is unmodeled (the clock halfword is
        // owned by the clock device, not the bank).
        let mut cx = Cx::new(&mut clock, 0, 0, 0);
        host.read(&mut cx, 0xfffc402c, 2);
        assert!(cx.refusal().is_some());
    }

    #[test]
    fn bytes_merge_and_read_back_their_lane() {
        let mut host = RegisterBank::new(&[(0xfffc6000, 0xfffc622c)]).unwrap();
        let mut clock = Clock::default();
        host.write(&mut Cx::new(&mut clock, 0, 0, 0), 0xfffc6078, 1, 0x1);
        host.write(&mut Cx::new(&mut clock, 0, 0, 0), 0xfffc6079, 1, 0x2);
        assert_eq!(host.read(&mut Cx::new(&mut clock, 0, 0, 0), 0xfffc6078, 4), 0x201);
        let mut cx = Cx::new(&mut clock, 0, 0, 0);
        assert_eq!(host.read(&mut cx, 0xfffc6079, 1), 0x2);
    }

    #[test]
    fn overlapping_or_misaligned_windows_refuse() {
        assert!(RegisterBank::new(&[(0xfffc4000, 0xfffc5000), (0xfffc4800, 0xfffc6000)]).is_err());
        assert!(RegisterBank::new(&[(0xfffc4001, 0xfffc5000)]).is_err());
        assert!(RegisterBank::new(&[]).is_err());
    }
}
