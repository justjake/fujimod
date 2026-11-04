//! EXR clock mux/PLL latches; the PLL offsets and the latch set are declared
//! inputs. Each PLL's enable bit 28 and lock bit 24 use virtual settling time.
//! Frequencies and analog lock timing are unmeasured declared inputs.
use crate::{Cx, Device};
use serde::{Deserialize, Serialize};

const BASE: u64 = 0xfff88000;
/// Read-only readiness word; bit 16 is `reset_ready`.
const READY: u32 = 0x80;
const WORDS: usize = 0x88 / 4;
const ENABLE: u32 = 1 << 28;
const LOCKED: u32 = 1 << 24;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub settle_ticks: u64,
    pub reset_ready: bool,
    /// Byte offsets of the PLL enable/lock registers; +34 unless declared.
    #[serde(default = "exr_plls")]
    pub plls: Vec<u32>,
    /// Byte offsets of the plain read/write latch registers; +00..+38 and
    /// +84 except the PLL unless declared.
    #[serde(default = "exr_latches")]
    pub latches: Vec<u32>,
}
fn exr_plls() -> Vec<u32> {
    vec![0x34]
}
fn exr_latches() -> Vec<u32> {
    (0..=0x38).step_by(4).filter(|&o| o != 0x34).chain([0x84]).collect()
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    registers: Vec<u32>,
    /// Lock deadline of each declared PLL, in declaration order.
    due: Vec<Option<u64>>,
}
pub struct Clocks {
    options: Options,
    /// Word indices of the declared PLL registers.
    plls: Vec<usize>,
    /// Word indices of the declared latch registers.
    latches: Vec<usize>,
    state: State,
}
impl Clocks {
    pub fn new(options: Options) -> Result<Self, String> {
        if options.settle_ticks == 0 {
            return Err("EXR PLL needs positive settling time".into());
        }
        fn word(offset: u32, role: &str) -> Result<usize, String> {
            if !offset.is_multiple_of(4) || offset >= 0x88 {
                return Err(format!(
                    "EXR clock {role} offset {offset:#x} is not an aligned word below 0x88"
                ));
            }
            if offset == READY {
                return Err(format!(
                    "EXR clock {role} offset {offset:#x} is the readiness word"
                ));
            }
            Ok(offset as usize / 4)
        }
        let mut plls = Vec::with_capacity(options.plls.len());
        for &offset in &options.plls {
            let pll = word(offset, "PLL")?;
            if plls.contains(&pll) {
                return Err(format!("EXR clock PLL offset {offset:#x} is repeated"));
            }
            plls.push(pll);
        }
        let mut latches = Vec::with_capacity(options.latches.len());
        for &offset in &options.latches {
            let latch = word(offset, "latch")?;
            if plls.contains(&latch) {
                return Err(format!(
                    "EXR clock latch offset {offset:#x} is a PLL register"
                ));
            }
            if latches.contains(&latch) {
                return Err(format!("EXR clock latch offset {offset:#x} is repeated"));
            }
            latches.push(latch);
        }
        let mut registers = vec![0; WORDS];
        registers[READY as usize / 4] = u32::from(options.reset_ready) << 16;
        let due = vec![None; plls.len()];
        Ok(Self {
            options,
            plls,
            latches,
            state: State { registers, due },
        })
    }
    fn index(&self, address: u64, size: u8) -> Option<usize> {
        let offset = address.checked_sub(BASE)?;
        if size != 4 || !offset.is_multiple_of(4) || offset >= 0x88 {
            return None;
        }
        let index = (offset / 4) as usize;
        (self.plls.contains(&index)
            || index == READY as usize / 4
            || self.latches.contains(&index))
            .then_some(index)
    }
}
impl Device for Clocks {
    crate::checkpoint_fields!(state: State);
    fn name(&self) -> &'static str {
        "exr-clock-control"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match self.index(address, size) {
            Some(index) => self.state.registers[index].into(),
            None => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(index) = self.index(address, size) else {
            cx.unmodeled();
            return;
        };
        let Ok(word) = u32::try_from(value) else {
            cx.fail("EXR clock word exceeds 32 bits");
            return;
        };
        if index == READY as usize / 4 {
            cx.fail("EXR clock readiness is read-only");
            return;
        }
        if let Some(pll) = self.plls.iter().position(|&p| p == index) {
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
            self.state.due[pll] = due;
        } else {
            self.state.registers[index] = word;
        }
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.due.iter().flatten().copied().min()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        for (pll, due) in self.plls.iter().zip(&mut self.state.due) {
            if due.is_some_and(|due| due <= cx.now()) {
                self.state.registers[*pll] |= LOCKED;
                *due = None;
            }
        }
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        self.index(address, size).is_some()
    }
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(self.save())
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state.registers.len() != WORDS
            || state.registers[READY as usize / 4] != u32::from(self.options.reset_ready) << 16
            || state.due.len() != self.plls.len()
            || self.plls.iter().zip(&state.due).any(|(&pll, due)| {
                due.is_some() && state.registers[pll] & (ENABLE | LOCKED) != ENABLE
            })
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
        // The default register layout.
        let mut model = Clocks::new(
            serde_json::from_value(serde_json::json!({"settle_ticks": 17, "reset_ready": true}))
                .unwrap(),
        )
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
    #[test]
    fn pll_offset_and_latches_are_declared() {
        let mut model = Clocks::new(Options {
            settle_ticks: 5,
            reset_ready: false,
            plls: vec![0x40, 0x48],
            latches: vec![0x0, 0x28, 0x58, 0x5c, 0x84],
        })
        .unwrap();
        let mut host = TestHost::new();
        write(&mut model, &mut host, BASE + 0x40, ENABLE | 3);
        assert_eq!(
            read(&mut model, &mut host, BASE + 0x40),
            u64::from(ENABLE | 3)
        );
        advance(&mut model, &mut host, 4);
        assert_eq!(
            read(&mut model, &mut host, BASE + 0x40),
            u64::from(ENABLE | 3)
        );
        advance(&mut model, &mut host, 5);
        assert_eq!(
            read(&mut model, &mut host, BASE + 0x40),
            u64::from(ENABLE | LOCKED | 3)
        );
        write(&mut model, &mut host, BASE + 0x48, ENABLE);
        assert_eq!(read(&mut model, &mut host, BASE + 0x48), u64::from(ENABLE));
        advance(&mut model, &mut host, 10);
        assert_eq!(
            read(&mut model, &mut host, BASE + 0x48),
            u64::from(ENABLE | LOCKED)
        );
        assert_eq!(model.next_deadline(10), None);
        write(&mut model, &mut host, BASE + 0x58, 0x1234_5678);
        assert_eq!(read(&mut model, &mut host, BASE + 0x58), 0x1234_5678);
        write(&mut model, &mut host, BASE + 0x5c, 0x9abc_def0);
        assert_eq!(read(&mut model, &mut host, BASE + 0x5c), 0x9abc_def0);
        assert!(!model.pollable(BASE + 0x34, 4));
        let mut cx = host.cx();
        assert_eq!(model.read(&mut cx, BASE + 0x34, 4), 0);
        assert!(cx.is_unmodeled());
    }
    #[test]
    fn invalid_pll_and_latches_are_rejected() {
        let options = |pll: u32, latches: Vec<u32>| Options {
            settle_ticks: 5,
            reset_ready: false,
            plls: vec![pll],
            latches,
        };
        assert!(
            Clocks::new(Options {
                settle_ticks: 5,
                reset_ready: false,
                plls: vec![0x40, 0x40],
                latches: vec![],
            })
            .is_err()
        );
        assert!(Clocks::new(options(0x80, vec![])).is_err());
        assert!(Clocks::new(options(0x34, vec![0x80])).is_err());
        assert!(Clocks::new(options(0x34, vec![0x34])).is_err());
        assert!(Clocks::new(options(0x34, vec![0x20, 0x20])).is_err());
        assert!(Clocks::new(options(0x36, vec![])).is_err());
        assert!(Clocks::new(options(0x34, vec![0x84, 0x86])).is_err());
        assert!(Clocks::new(options(0x34, vec![0x88])).is_err());
        assert!(Clocks::new(options(0x34, vec![0x84, 0x0])).is_ok());
    }
}
