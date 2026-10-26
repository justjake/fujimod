//! XP4 on-chip temperature conversion: 1090818..1090d9b.
//! A signed twelve-bit sample is 125 millicelsius; timing is explicit.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Serialize, Deserialize)]
struct State {
    regs: [u32; 17],
    common: [u32; 2],
    deadline: Option<u64>,
}
pub struct Temperature {
    raw: i16,
    ticks: u64,
    state: State,
}
impl Temperature {
    pub fn new(millicelsius: i32, ticks: u64) -> Result<Self, String> {
        if millicelsius % 125 != 0
            || !(-256000..=255875).contains(&millicelsius)
            || ticks == 0
            || ticks > 54_000_000
        {
            return Err("Invalid XP4 temperature or conversion clock".into());
        }
        Ok(Self {
            raw: (millicelsius / 125) as i16,
            ticks,
            state: State {
                regs: {
                    let mut r = [0; 17];
                    r[6] = 15;
                    r
                },
                common: [0; 2],
                deadline: None,
            },
        })
    }
    fn index(address: u64) -> Option<usize> {
        if (0xfff47810..=0xfff47850).contains(&address) && address & 3 == 0 {
            Some((address as usize - 0xfff47810) / 4)
        } else {
            None
        }
    }
    fn arm(&mut self, cx: &mut Cx<'_>) {
        self.state.deadline = None;
        self.state.regs[8] = 0;
        if self.state.regs[0] & 1 != 0 && self.state.regs[1] & 1 != 0 {
            self.state.deadline = cx.now().checked_add(self.ticks << self.state.regs[2]);
            if self.state.deadline.is_none() {
                cx.fail("XP4 temperature deadline overflow");
            }
        }
    }
}
impl Device for Temperature {
    fn name(&self) -> &'static str {
        "xp4-temperature"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.advance(cx);
        if size != 4 {
            cx.unmodeled();
            return 0;
        }
        if address == 0xfff4700c {
            return u64::from(self.state.common[0]);
        }
        if address == 0xfff47028 {
            return u64::from(self.state.common[1]);
        }
        match Self::index(address) {
            Some(i) if matches!(i,0..=2|4..=6|8|9|12..=16) => u64::from(self.state.regs[i]),
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.advance(cx);
        if size != 4 || value > u64::from(u32::MAX) {
            cx.unmodeled();
            return;
        }
        let v = value as u32;
        if address == 0xfff4700c && v <= 1 {
            self.state.common[0] = v;
            return;
        }
        if address == 0xfff47028 && v <= 1 {
            self.state.common[1] = v;
            return;
        }
        match Self::index(address) {
            Some(i @ 0..=1) if v <= 1 => {
                self.state.regs[i] = v;
                self.arm(cx);
            }
            Some(2) if v <= 3 => self.state.regs[2] = v,
            Some(5) if v <= 15 => {
                self.state.regs[4] &= !v;
            }
            Some(6) if v == 15 => {
                self.state.regs[6] = v;
            }
            Some(i @ 12..=15) if v <= 0xfff => self.state.regs[i] = v,
            Some(16) if v <= 255 => self.state.regs[16] = v,
            _ => cx.unmodeled(),
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let Some(_at) = self.state.deadline.filter(|at| *at <= cx.now()) else {
            return;
        };
        self.state.deadline = None;
        self.state.regs[8] = 1;
        self.state.regs[9] = self.raw as u16 as u32 & 0xfff;
        // Comparison interrupts are outside this bounded converter. The mask
        // stays all-masked; attempting to unmask one refuses the operation.
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state.deadline
    }
    fn save(&self) -> Value {
        serde_json::json!({"raw":self.raw,"millicelsius":i32::from(self.raw)*125,"ticks":self.ticks,"sensor":self.state})
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        let s: State = serde_json::from_value(v["sensor"].clone()).map_err(|e| e.to_string())?;
        if v["raw"] != self.raw
            || v["ticks"] != self.ticks
            || s.regs[0] > 1
            || s.regs[1] > 1
            || s.regs[2] > 3
            || s.regs[4] > 15
            || s.regs[6] != 15
            || s.regs[8] > 1
            || s.regs[9] > 0xfff
            || s.regs[12..16].iter().any(|v| *v > 0xfff)
            || s.regs[16] > 255
            || (s.deadline.is_some() && (s.regs[0] == 0 || s.regs[1] == 0))
        {
            return Err("Invalid XP4 temperature checkpoint or changed physical input".into());
        }
        self.state = s;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{Host, Record};
    #[derive(Default)]
    struct Clock(u64);
    impl Host for Clock {
        fn now(&self) -> u64 {
            self.0
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            unreachable!()
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            unreachable!()
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            unreachable!()
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            unreachable!()
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            unreachable!()
        }
        fn record(&mut self, _: Record) {}
    }
    #[test]
    fn signed_physical_temperature_has_exact_deadline_and_observational_snapshot() {
        for (value, expected) in [(25000, 200), (-125, 4095)] {
            let mut m = Temperature::new(value, 5000).unwrap();
            let mut h = Clock::default();
            for (a, v) in [(0xfff47810, 1), (0xfff47818, 1), (0xfff47814, 1)] {
                m.write(&mut Cx::new(&mut h, 3, 0x123400, 0), a, 4, v);
            }
            assert_eq!(m.next_deadline(0), Some(10000));
            let saved = m.save();
            h.0 = 9999;
            m.advance(&mut Cx::new(&mut h, 0, 0, 0));
            assert_eq!(m.save(), saved);
            let mut restored = Temperature::new(value, 5000).unwrap();
            restored.restore(&saved).unwrap();
            h.0 = 10000;
            let mut cx = Cx::new(&mut h, 1, 0xdead, 0);
            restored.advance(&mut cx);
            assert_eq!(restored.read(&mut cx, 0xfff47830, 4), 1);
            assert_eq!(restored.read(&mut cx, 0xfff47834, 4), expected);
            assert!(cx.refusal().is_none());
            let saved = restored.save();
            assert_eq!(restored.save(), saved);
            restored.write(&mut cx, 0xfff47828, 4, 0);
            assert!(cx.is_unmodeled());
        }
    }
}
