//! PLL/divider request banks with bounded divider selections.
//! Completion/readback only; rates are not measured. Initial PLL words are
//! zero unless a board declares them with a `Reset`.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const BANKS: [(u64, usize); 17] = [
    (0xffa01000, 4),
    (0xffa02000, 3),
    (0xffa03000, 3),
    (0xffa04000, 4),
    (0xffa05000, 3),
    (0xffa10000, 2),
    (0xffa11000, 3),
    (0xffa12000, 1),
    (0xffa13000, 1),
    (0xffa14000, 1),
    (0xffa15000, 1),
    (0xffa16000, 2),
    (0xffa17000, 2),
    (0xffa18000, 3),
    (0xffa19000, 4),
    (0xffa1a000, 2),
    (0xffa1b000, 1),
];
/// Register state left programmed before the guest starts, and the status
/// word read back while the PLL is idle.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reset {
    pub base: u64,
    pub n: u32,
    pub fraction: u32,
    pub dividers: Vec<u32>,
    pub idle_status: u32,
}
#[derive(Default, Serialize, Deserialize)]
struct State {
    config: [u32; 2],
    dividers: [u32; 4],
    #[serde(default)]
    idle_status: u32,
}
pub struct Bank {
    base: u64,
    state: State,
    count: usize,
    index: usize,
}
impl Bank {
    pub fn new(base: u64) -> Result<Self, String> {
        Self::with_reset(base, None)
    }
    pub fn with_reset(base: u64, reset: Option<&Reset>) -> Result<Self, String> {
        let (index, &(_, count)) = BANKS
            .iter()
            .enumerate()
            .find(|(_, (b, _))| *b == base)
            .ok_or("unknown XP5 PLL bank")?;
        let mut state = State::default();
        if let Some(reset) = reset {
            if reset.base != base {
                return Err(format!(
                    "XP5 PLL reset for {:#x} given to bank {base:#x}",
                    reset.base
                ));
            }
            if reset.dividers.len() != count {
                return Err(format!(
                    "XP5 PLL bank {base:#x} reset needs {count} dividers, got {}",
                    reset.dividers.len()
                ));
            }
            if reset.dividers.iter().any(|&d| d > 0x1ff) {
                return Err(format!(
                    "XP5 PLL bank {base:#x} reset divider exceeds 0x1ff"
                ));
            }
            if reset.idle_status & 1 != 0 {
                return Err(format!(
                    "XP5 PLL bank {base:#x} idle status must not set the busy bit"
                ));
            }
            state.config = [reset.n, reset.fraction];
            state.dividers[..count].copy_from_slice(&reset.dividers);
            state.idle_status = reset.idle_status;
        }
        Ok(Self {
            base,
            state,
            count,
            index,
        })
    }
    fn divider(&self, o: u64) -> Option<usize> {
        if o >= 0x40 && o & 3 == 0 && (o - 0x40) / 4 < self.count as u64 {
            Some(((o - 0x40) / 4) as usize)
        } else {
            None
        }
    }
}
impl Device for Bank {
    fn name(&self) -> &'static str {
        "clock-pll-bank"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        let o = address.wrapping_sub(self.base);
        if n != 4 {
            cx.unmodeled();
            return 0;
        }
        if let Some(i) = self.divider(o) {
            return self.state.dividers[i] as u64;
        }
        match o {
            0 => self.state.config[0] as u64,
            8 => self.state.config[1] as u64,
            // 1496660 waits for request bits0..2 clear; 1496a68 waits for busy bit0 clear.
            0x80 => 0,
            0xf0 => self.state.idle_status as u64,
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, v: u64) {
        let o = address.wrapping_sub(self.base);
        if n != 4 {
            cx.unmodeled();
            return;
        }
        if let Some(i) = self.divider(o) {
            if v > 0x1ff {
                cx.fail("clock divider exceeds stock table range");
                return;
            }
            self.state.dividers[i] = v as u32;
            return;
        }
        match o {
            0 => self.state.config[0] = v as u32,
            8 => self.state.config[1] = v as u32,
            // Immediate functional completion; no PLL lock time is claimed.
            0x80 if matches!(v, 2 | 3)
                || ((v == 5 && matches!(self.index, 7 | 8 | 9 | 12))
                    || (v == 6 && matches!(self.index, 7 | 9 | 12))) => {}
            _ => cx.unmodeled(),
        }
    }
    fujimod_emulator_devices::checkpoint_fields!(state: State);
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        self.state = serde_json::from_value(v.clone())
            .map_err(|e| format!("invalid clock bank checkpoint: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;

    const BASE: u64 = 0xffa01000;

    fn reset() -> Reset {
        Reset {
            base: BASE,
            n: 0x1234,
            fraction: 0x56,
            dividers: vec![1, 2, 3, 0x1ff],
            idle_status: 0x10,
        }
    }
    fn read(bank: &mut Bank, o: u64) -> u64 {
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        let v = bank.read(&mut cx, BASE + o, 4);
        assert!(!cx.is_unmodeled());
        v
    }

    #[test]
    fn declared_bank_reads_back_state_and_completes_requests() {
        let mut bank = Bank::with_reset(BASE, Some(&reset())).unwrap();
        assert_eq!(read(&mut bank, 0), 0x1234);
        assert_eq!(read(&mut bank, 8), 0x56);
        for (i, d) in [1, 2, 3, 0x1ff].into_iter().enumerate() {
            assert_eq!(read(&mut bank, 0x40 + 4 * i as u64), d);
        }
        assert_eq!(read(&mut bank, 0xf0), 0x10);
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        bank.write(&mut cx, BASE + 0x80, 4, 2);
        assert!(!cx.is_unmodeled());
        assert_eq!(read(&mut bank, 0x80), 0);
        assert_eq!(read(&mut bank, 0xf0), 0x10);
        let mut plain = Bank::new(BASE).unwrap();
        assert_eq!(read(&mut plain, 0xf0), 0);
        assert_eq!(read(&mut plain, 0), 0);
    }

    #[test]
    fn invalid_resets_refuse() {
        let mut r = reset();
        r.base = 0xffa02000;
        assert!(Bank::with_reset(BASE, Some(&r)).is_err());
        let mut r = reset();
        r.dividers.pop();
        assert!(Bank::with_reset(BASE, Some(&r)).is_err());
        let mut r = reset();
        r.dividers[2] = 0x200;
        assert!(Bank::with_reset(BASE, Some(&r)).is_err());
        let mut r = reset();
        r.idle_status = 0x11;
        assert!(Bank::with_reset(BASE, Some(&r)).is_err());
    }

    #[test]
    fn checkpoint_round_trip_keeps_idle_status() {
        let bank = Bank::with_reset(BASE, Some(&reset())).unwrap();
        let saved = bank.save();
        let mut other = Bank::new(BASE).unwrap();
        other.restore(&saved).unwrap();
        assert_eq!(read(&mut other, 0xf0), 0x10);
        assert_eq!(read(&mut other, 0), 0x1234);
        assert_eq!(saved, other.save());
        assert!(other.restore(&serde_json::json!({"config": "x"})).is_err());
    }
}
