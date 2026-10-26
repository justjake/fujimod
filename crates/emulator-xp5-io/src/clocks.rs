//! PLL/divider request banks with bounded divider selections.
//! Completion/readback only; rates and initial PLL words are not measured.
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
#[derive(Default, Serialize, Deserialize)]
struct State {
    config: [u32; 2],
    dividers: [u32; 4],
}
pub struct Bank {
    base: u64,
    state: State,
    count: usize,
    index: usize,
}
impl Bank {
    pub fn new(base: u64) -> Result<Self, String> {
        let (index, &(_, count)) = BANKS
            .iter()
            .enumerate()
            .find(|(_, (b, _))| *b == base)
            .ok_or("unknown XP5 PLL bank")?;
        Ok(Self {
            base,
            state: State::default(),
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
            0x80 | 0xf0 => 0,
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
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        self.state = serde_json::from_value(v.clone())
            .map_err(|e| format!("invalid clock bank checkpoint: {e}"))?;
        Ok(())
    }
}
