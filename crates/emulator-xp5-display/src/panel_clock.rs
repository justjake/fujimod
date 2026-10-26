//! Panel pixel-clock registers and frequency selection.
//! Only the four display PLL banks are owned here. Request/lock latency
//! is an explicit50us scheduling assumption; frequencies use guest N/D.
use fujimod_emulator_devices::{Cx, Device};
use serde_json::Value;
use fujimod_emulator_devices::xp5::panel_clocks::Clocks;
pub struct PanelPll {
    clocks: Clocks,
    index: usize,
}
impl PanelPll {
    pub fn new(base: u64, clocks: Clocks) -> Result<Self, String> {
        let index = match base {
            0xffa11000 => 0,
            0xffa12000 => 1,
            0xffa13000 => 2,
            0xffa14000 => 3,
            _ => return Err("unsupported panel PLL base".into()),
        };
        Ok(Self { clocks, index })
    }
}
impl Device for PanelPll {
    fn name(&self) -> &'static str {
        "panel-pll"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        let o = address.wrapping_sub([0xffa11000, 0xffa12000, 0xffa13000, 0xffa14000][self.index]);
        if n != 4 {
            cx.unmodeled();
            return 0;
        }
        let p = self.clocks.0.lock().unwrap().plls[self.index];
        match o {
            0 => p.n as u64,
            8 => p.fraction as u64,
            0x40 => p.divider as u64,
            0x80 => p.request as u64,
            0xf0 => u64::from(p.request != 0),
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, v: u64) {
        let o = address.wrapping_sub([0xffa11000, 0xffa12000, 0xffa13000, 0xffa14000][self.index]);
        if n != 4 {
            cx.unmodeled();
            return;
        }
        let mut s = self.clocks.0.lock().unwrap();
        let p = &mut s.plls[self.index];
        match o {
            0 if p.request == 0 => p.n = v as u32,
            8 if p.request == 0 => p.fraction = v as u32,
            0x40 if p.request == 0 && v <= 0x1ff => p.divider = v as u32,
            0x80 if p.request == 0 && matches!(v, 2 | 3 | 5 | 6) => {
                p.request = v as u32;
                p.generation += 1;
                cx.schedule(cx.now() + 50_000, p.generation);
            }
            0 | 8 | 0x40 | 0x80 => cx.fail("invalid/busy panel PLL programming"),
            _ => cx.unmodeled(),
        }
    }
    fn event(&mut self, _: &mut Cx<'_>, token: u64) {
        let mut s = self.clocks.0.lock().unwrap();
        let p = &mut s.plls[self.index];
        if token == p.generation {
            p.request = 0;
        }
    }
    fn save(&self) -> Value {
        let s = self.clocks.0.lock().unwrap();
        serde_json::json!({"pll":s.plls[self.index],"selector":s.selector})
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        let pll = serde_json::from_value(v["pll"].clone())
            .map_err(|e| format!("invalid PLL checkpoint: {e}"))?;
        let selector = v["selector"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or("invalid PLL selector checkpoint")?;
        let mut s = self.clocks.0.lock().unwrap();
        s.plls[self.index] = pll;
        s.selector = selector;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;

    #[test]
    fn programming_checkpoint_and_guest_deadline() {
        let clocks = Clocks::new();
        let mut pll = PanelPll::new(0xffa11000, clocks.clone()).unwrap();
        let mut host = RecordingHost {
            now: 700,
            ..RecordingHost::default()
        };
        let mut cx = Cx::new(&mut host, 0, 0, 2);
        pll.write(&mut cx, 0xffa11000, 4, 100);
        pll.write(&mut cx, 0xffa11040, 4, 1);
        pll.write(&mut cx, 0xffa11080, 4, 3);
        let saved = pll.save();
        for _ in 0..32 {
            assert_eq!(pll.read(&mut cx, 0xffa110f0, 4), 1);
        }
        assert_eq!(saved, pll.save());
        assert_eq!(cx.refusal(), None);
        assert_eq!(host.events, [(2, 50_700, 1)]);
        host.now = 50_700;
        let mut restored = PanelPll::new(0xffa11000, clocks.clone()).unwrap();
        restored.restore(&saved).unwrap();
        restored.event(&mut Cx::new(&mut host, 0, 0, 2), 1);
        assert_eq!(
            restored.read(&mut Cx::new(&mut host, 0, 0, 2), 0xffa110f0, 4),
            0
        );
        assert_eq!(clocks.pixel_hz(1).unwrap(), 600_000_000.0);
    }
}
