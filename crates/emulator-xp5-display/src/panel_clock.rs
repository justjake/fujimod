//! Panel pixel-clock registers and frequency selection.
//! Only the four display PLL banks are owned here. Request/lock latency
//! is an explicit50us scheduling assumption; frequencies use guest N/D.
use fujimod_emulator_devices::xp5::panel_clocks::Clocks;
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_xp5_io::clocks::BANKS;
use serde_json::Value;

const BASES: [u64; 4] = [0xffa11000, 0xffa12000, 0xffa13000, 0xffa14000];

/// Declared state of one panel PLL when the guest starts.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(default)]
    pub reset: Option<Reset>,
}

/// Register state left programmed before the guest starts, and the status
/// word read back while the PLL is idle.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reset {
    pub n: u32,
    pub fraction: u32,
    pub dividers: Vec<u32>,
    pub idle_status: u32,
}

pub struct PanelPll {
    clocks: Clocks,
    index: usize,
    divider_count: usize,
    idle_status: u32,
}
impl PanelPll {
    pub fn new(base: u64, clocks: Clocks, options: Options) -> Result<Self, String> {
        let index = BASES
            .iter()
            .position(|&candidate| candidate == base)
            .ok_or("unsupported panel PLL base")?;
        let divider_count = BANKS
            .iter()
            .find_map(|&(bank_base, count)| (bank_base == base).then_some(count))
            .ok_or("panel PLL base has no XP5 clock bank")?;
        let mut idle_status = 0;
        if let Some(reset) = options.reset {
            if reset.dividers.len() != divider_count {
                return Err(format!(
                    "panel PLL reset needs {divider_count} dividers, got {}",
                    reset.dividers.len()
                ));
            }
            if reset.dividers.iter().any(|&d| d > 0x1ff) {
                return Err("panel PLL reset divider exceeds 0x1ff".into());
            }
            if reset.idle_status & 1 != 0 {
                return Err("panel PLL idle status must not set the busy bit".into());
            }
            let mut s = clocks.0.lock().unwrap();
            let pll = &mut s.plls[index];
            pll.n = reset.n;
            pll.fraction = reset.fraction;
            for (slot, &d) in pll.dividers.iter_mut().zip(&reset.dividers) {
                *slot = d;
            }
            idle_status = reset.idle_status;
        }
        Ok(Self {
            clocks,
            index,
            divider_count,
            idle_status,
        })
    }
    fn divider(&self, offset: u64) -> Option<usize> {
        if offset >= 0x40 && offset & 3 == 0 && (offset - 0x40) / 4 < self.divider_count as u64 {
            Some(((offset - 0x40) / 4) as usize)
        } else {
            None
        }
    }
}
impl Device for PanelPll {
    fn name(&self) -> &'static str {
        "panel-pll"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        let o = address.wrapping_sub(BASES[self.index]);
        if n != 4 {
            cx.unmodeled();
            return 0;
        }
        let p = self.clocks.0.lock().unwrap().plls[self.index];
        if let Some(i) = self.divider(o) {
            return p.dividers[i] as u64;
        }
        match o {
            0 => p.n as u64,
            8 => p.fraction as u64,
            0x80 => p.request as u64,
            0xf0 => u64::from(self.idle_status) | u64::from(p.request != 0),
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, v: u64) {
        let o = address.wrapping_sub(BASES[self.index]);
        if n != 4 {
            cx.unmodeled();
            return;
        }
        let mut s = self.clocks.0.lock().unwrap();
        let p = &mut s.plls[self.index];
        if let Some(i) = self.divider(o) {
            if p.request != 0 || v > 0x1ff {
                cx.fail("invalid/busy panel PLL programming");
                return;
            }
            p.dividers[i] = v as u32;
            return;
        }
        match o {
            0 if p.request == 0 => p.n = v as u32,
            8 if p.request == 0 => p.fraction = v as u32,
            0x80 if p.request == 0 && matches!(v, 2 | 3 | 5 | 6) => {
                p.request = v as u32;
                p.generation += 1;
                cx.schedule(cx.now() + 50_000, p.generation);
            }
            0 | 8 | 0x80 => cx.fail("invalid/busy panel PLL programming"),
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
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        let clocks = self.clocks.0.lock().unwrap();
        Checkpoint::encode(self.name(), &(clocks.plls[self.index], clocks.selector))
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        use fujimod_emulator_devices::xp5::panel_clocks::Pll;
        let (_, selector): (Pll, u32) = saved.decode(self.name())?;
        if selector > 1 {
            return Err("panel clock selector differs".into());
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (pll, selector) = saved.decode(self.name())?;
        let mut clocks = self.clocks.0.lock().unwrap();
        clocks.plls[self.index] = pll;
        clocks.selector = selector;
        Ok(())
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
        let mut pll = PanelPll::new(0xffa11000, clocks.clone(), Options::default()).unwrap();
        let mut host = RecordingHost {
            now: 700,
            ..RecordingHost::default()
        };
        let mut cx = Cx::new(&mut host, 0, 0, 2);
        pll.write(&mut cx, 0xffa11000, 4, 100);
        pll.write(&mut cx, 0xffa11040, 4, 1);
        pll.write(&mut cx, 0xffa11044, 4, 2);
        pll.write(&mut cx, 0xffa11048, 4, 3);
        pll.write(&mut cx, 0xffa11080, 4, 3);
        assert_eq!(pll.read(&mut cx, 0xffa11044, 4), 2);
        assert_eq!(pll.read(&mut cx, 0xffa11048, 4), 3);
        let saved = pll.save();
        for _ in 0..32 {
            assert_eq!(pll.read(&mut cx, 0xffa110f0, 4), 1);
        }
        assert_eq!(saved, pll.save());
        assert_eq!(cx.refusal(), None);
        assert_eq!(host.events, [(2, 50_700, 1)]);

        let mut bank1 = PanelPll::new(0xffa12000, clocks.clone(), Options::default()).unwrap();
        let mut unmodeled_cx = Cx::new(&mut host, 0, 0, 2);
        bank1.write(&mut unmodeled_cx, 0xffa12044, 4, 2);
        assert!(unmodeled_cx.is_unmodeled());
        let mut unmodeled_cx = Cx::new(&mut host, 0, 0, 2);
        assert_eq!(bank1.read(&mut unmodeled_cx, 0xffa12044, 4), 0);
        assert!(unmodeled_cx.is_unmodeled());

        host.now = 50_700;
        let mut restored = PanelPll::new(0xffa11000, clocks.clone(), Options::default()).unwrap();
        restored.restore(&saved).unwrap();
        restored.event(&mut Cx::new(&mut host, 0, 0, 2), 1);
        assert_eq!(
            restored.read(&mut Cx::new(&mut host, 0, 0, 2), 0xffa110f0, 4),
            0
        );
        assert_eq!(clocks.pixel_hz(1).unwrap(), 600_000_000.0);
    }

    fn declared(json: serde_json::Value) -> Result<Options, String> {
        serde_json::from_value(json).map_err(|e| e.to_string())
    }

    #[test]
    fn declared_reset_state_and_idle_status() {
        let clocks = Clocks::new();
        let options = declared(serde_json::json!({"reset": {
            "n": 99, "fraction": 34, "dividers": [0x115, 0x12b, 0], "idle_status": 8
        }}))
        .unwrap();
        let mut pll = PanelPll::new(0xffa11000, clocks.clone(), options).unwrap();
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 2);
        for (offset, want) in [
            (0, 99),
            (8, 34),
            (0x40, 0x115),
            (0x44, 0x12b),
            (0x48, 0),
            (0xf0, 8),
        ] {
            assert_eq!(
                pll.read(&mut cx, 0xffa11000 + offset, 4),
                want,
                "+{offset:#x}"
            );
        }
        assert_eq!(clocks.pixel_hz(1).unwrap(), 54_000_000.0);
        pll.write(&mut cx, 0xffa11080, 4, 3);
        assert_eq!(pll.read(&mut cx, 0xffa110f0, 4), 9);
        pll.event(&mut cx, 1);
        assert_eq!(pll.read(&mut cx, 0xffa110f0, 4), 8);
        assert_eq!(cx.refusal(), None);
    }

    #[test]
    fn invalid_declared_state_is_refused() {
        let reset = |dividers: serde_json::Value, idle_status: u32| {
            declared(serde_json::json!({"reset": {
                "n": 1, "fraction": 0, "dividers": dividers, "idle_status": idle_status
            }}))
            .unwrap()
        };
        let new = |options| PanelPll::new(0xffa11000, Clocks::new(), options).map(|_| ());
        assert!(new(reset(serde_json::json!([1, 2]), 0)).is_err());
        assert!(new(reset(serde_json::json!([1, 0x200, 0]), 0)).is_err());
        assert!(new(reset(serde_json::json!([1, 2, 3]), 1)).is_err());
        assert!(new(reset(serde_json::json!([1, 2, 3]), 8)).is_ok());
        assert!(declared(serde_json::json!({"unknown": 1})).is_err());
        assert!(
            declared(serde_json::json!({"reset": {
                "n": 1, "fraction": 0, "dividers": [], "idle_status": 0, "extra": 1
            }}))
            .is_err()
        );
    }
}
