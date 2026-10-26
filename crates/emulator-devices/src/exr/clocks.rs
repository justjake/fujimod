//! EXR clock mux/PLL latches. PLL +34 enable/lock use virtual settling time.
//! Frequencies and analog lock timing are unmeasured declared inputs.
use crate::{Cx, Device};
use serde::{Deserialize, Serialize};

const BASE: u64 = 0xfff88000;
const PLL: usize = 0x34 / 4;
const ENABLE: u32 = 1 << 28;
const LOCKED: u32 = 1 << 24;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub settle_ticks: u64,
    pub reset_ready: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    registers: Vec<u32>,
    due: Option<u64>,
}
pub struct Clocks {
    options: Options,
    state: State,
}
impl Clocks {
    pub fn new(options: Options) -> Result<Self, String> {
        if options.settle_ticks == 0 {
            return Err("EXR PLL needs positive settling time".into());
        }
        let mut registers = vec![0; 0x88 / 4];
        registers[0x80 / 4] = u32::from(options.reset_ready) << 16;
        Ok(Self {
            options,
            state: State {
                registers,
                due: None,
            },
        })
    }
    fn index(address: u64, size: u8) -> Option<usize> {
        let offset = address.checked_sub(BASE)?;
        (size == 4 && offset.is_multiple_of(4) && matches!(offset, 0..=0x38 | 0x80 | 0x84))
            .then_some((offset / 4) as usize)
    }
}
impl Device for Clocks {
    crate::checkpoint_fields!(state: State);
    fn name(&self) -> &'static str {
        "exr-clock-control"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match Self::index(address, size) {
            Some(index) => self.state.registers[index].into(),
            None => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(index) = Self::index(address, size) else {
            cx.unmodeled();
            return;
        };
        let Ok(word) = u32::try_from(value) else {
            cx.fail("EXR clock word exceeds 32 bits");
            return;
        };
        if index == 0x80 / 4 {
            cx.fail("EXR clock readiness is read-only");
            return;
        }
        if index == PLL {
            let due = if word & ENABLE != 0 {
                let Some(due) = cx.now().checked_add(self.options.settle_ticks) else {
                    cx.fail("EXR PLL deadline overflow");
                    return;
                };
                Some(due)
            } else {
                None
            };
            self.state.registers[index] = word & !LOCKED;
            self.state.due = due;
        } else {
            self.state.registers[index] = word;
        }
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.due
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.state.due.is_some_and(|due| due <= cx.now()) {
            self.state.registers[PLL] |= LOCKED;
            self.state.due = None;
        }
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        Self::index(address, size).is_some()
    }
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(self.save())
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state.registers.len() != 0x88 / 4
            || state.registers[0x80 / 4] != u32::from(self.options.reset_ready) << 16
            || state.due.is_some() && state.registers[PLL] & (ENABLE | LOCKED) != ENABLE
        {
            return Err("invalid EXR clock checkpoint".into());
        }
        self.state = state;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_host::{TestHost, advance, read, write};
    #[test]
    fn pll_locks_only_at_the_enabled_register_deadline_and_inspection_is_passive() {
        let mut model = Clocks::new(Options {
            settle_ticks: 17,
            reset_ready: true,
        })
        .unwrap();
        let mut host = TestHost::new();
        host.now = 100;
        write(&mut model, &mut host, BASE + 0x34, ENABLE | LOCKED | 7);
        let saved = model.save();
        for _ in 0..5 {
            assert_eq!(
                read(&mut model, &mut host, BASE + 0x34),
                u64::from(ENABLE | 7)
            );
            assert_eq!(model.save(), saved);
            assert_eq!(model.next_deadline(100), Some(117));
        }
        advance(&mut model, &mut host, 116);
        assert_eq!(model.save(), saved);
        advance(&mut model, &mut host, 117);
        assert_eq!(
            read(&mut model, &mut host, BASE + 0x34),
            u64::from(ENABLE | LOCKED | 7)
        );
        model.restore(&saved).unwrap();
        advance(&mut model, &mut host, 117);
        assert_eq!(
            read(&mut model, &mut host, BASE + 0x34),
            u64::from(ENABLE | LOCKED | 7)
        );
        write(&mut model, &mut host, BASE + 0x34, 7);
        assert_eq!(model.next_deadline(117), None);
        advance(&mut model, &mut host, 1000);
        assert_eq!(read(&mut model, &mut host, BASE + 0x34), 7);
        assert!(host.effects.is_empty());
    }
}
