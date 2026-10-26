//! XP4's indexed USB PHY programming port, separate from DWC3 and bus traffic.
//! 1d89148: index[12:8], data[7:0], write-strobe rising edge at bit16;
//! selected register readback is at base+0x400. No analog/link-ready effects.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};

const WRITE: u32 = 1 << 16;
const CONTROL: u32 = WRITE | 0x1fff;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub base: u64,
    pub initial: [u8; 32],
    pub parallel: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    control: u32,
    parallel: u32,
    registers: [u8; 32],
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    config: Config,
    state: State,
}
pub struct Phy {
    config: Config,
    state: State,
}
impl Phy {
    pub fn new(config: Config) -> Result<Self, String> {
        if config.base & 3 != 0
            || config.base.checked_add(0x404).is_none()
            || config.parallel & !0x70 != 0
        {
            return Err("Invalid XP4 USB PHY wiring or parallel configuration".into());
        }
        Ok(Self {
            config,
            state: State {
                control: 0,
                parallel: config.parallel,
                registers: config.initial,
            },
        })
    }
    fn offset(&self, address: u64, size: u8) -> Option<u64> {
        let offset = address.checked_sub(self.config.base)?;
        (size == 4 && matches!(offset, 0 | 4 | 0x400)).then_some(offset)
    }
}
impl Device for Phy {
    fn name(&self) -> &'static str {
        "xp4-usb-phy"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match self.offset(address, size) {
            Some(0) => u64::from(self.state.control),
            Some(4) => u64::from(self.state.parallel),
            Some(0x400) => u64::from(self.state.registers[(self.state.control >> 8 & 31) as usize]),
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return;
        };
        let Ok(value) = u32::try_from(value) else {
            cx.fail("XP4 USB PHY word overflow");
            return;
        };
        match offset {
            0 => {
                if value & !CONTROL != 0
                    || self.state.control & value & WRITE != 0
                        && (self.state.control ^ value) & 0x1fff != 0
                {
                    cx.fail("Unsupported USB PHY control bits or data change while write strobe is high");
                    return;
                }
                if self.state.control & WRITE == 0 && value & WRITE != 0 {
                    self.state.registers[(value >> 8 & 31) as usize] = value as u8;
                }
                self.state.control = value;
            }
            4 => {
                if value & !0x70 != 0 {
                    cx.fail("Unsupported USB PHY parallel bits");
                    return;
                }
                self.state.parallel = value;
            }
            _ => cx.unmodeled(),
        }
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(Saved {
            config: self.config,
            state: self.state.clone(),
        })
        .unwrap()
    }
    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let saved: Saved = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        let s = &saved.state;
        if saved.config != self.config
            || s.control & !CONTROL != 0
            || s.parallel & !0x70 != 0
            || s.control & WRITE != 0
                && s.registers[(s.control >> 8 & 31) as usize] != s.control as u8
        {
            return Err("Invalid USB PHY checkpoint or changed physical input".into());
        }
        self.state = saved.state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{Host, Record};
    struct Board;
    impl Host for Board {
        fn now(&self) -> u64 {
            100
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            panic!("PHY has no autonomous clock")
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            panic!("PHY has no IRQ output")
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            unreachable!()
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            panic!("PHY never inspects RAM")
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            panic!("PHY never writes RAM")
        }
        fn record(&mut self, _: Record) {}
    }
    fn config() -> Config {
        let mut initial = [0; 32];
        initial[7] = 0xa0;
        Config {
            base: 0xffb23800,
            initial,
            parallel: 0,
        }
    }
    fn write(d: &mut Phy, cx: &mut Cx<'_>, value: u64) {
        d.write(cx, config().base, 4, value);
        assert_eq!(cx.refusal(), None);
    }
    #[test]
    fn register_pulses_observation_and_mid_strobe_restore_do_not_depend_on_programs() {
        fn run(pc: u64) -> serde_json::Value {
            let mut d = Phy::new(config()).unwrap();
            let mut b = Board;
            let mut cx = Cx::new(&mut b, 0, pc, 0);
            write(&mut d, &mut cx, 0x701);
            assert_eq!(d.read(&mut cx, config().base + 0x400, 4), 0xa0);
            write(&mut d, &mut cx, 0x7a5);
            assert_eq!(d.read(&mut cx, config().base + 0x400, 4), 0xa0);
            write(&mut d, &mut cx, 0x107a5);
            let high = d.save();
            write(&mut d, &mut cx, 0x107a5);
            assert_eq!(d.save(), high);
            let mut restored = Phy::new(config()).unwrap();
            restored.restore(&high).unwrap();
            assert_eq!(restored.next_deadline(100), None);
            write(&mut restored, &mut cx, 0x7a5);
            assert_eq!(restored.read(&mut cx, config().base + 0x400, 4), 0xa5);
            let saved = restored.save();
            for _ in 0..3 {
                assert_eq!(restored.read(&mut cx, config().base + 0x400, 4), 0xa5);
            }
            assert_eq!(restored.save(), saved);
            saved
        }
        assert_eq!(run(0x1110), run(0x8880));
    }
    #[test]
    fn invalid_state_and_unmodeled_apertures_leave_the_phy_unchanged() {
        let mut d = Phy::new(config()).unwrap();
        let before = d.save();
        for (key, value) in [
            ("/state/control", 0x10055u64),
            ("/state/parallel", 0x80),
            ("/config/base", 0xffb23804),
        ] {
            let mut damaged = before.clone();
            *damaged.pointer_mut(key).unwrap() = serde_json::json!(value);
            assert!(d.restore(&damaged).is_err());
            assert_eq!(d.save(), before);
        }
        let mut b = Board;
        let mut cx = Cx::new(&mut b, 0, 0, 0);
        d.write(&mut cx, config().base + 0x400, 4, 1);
        assert!(cx.is_unmodeled());
        assert_eq!(d.save(), before);

        let mut cx = Cx::new(&mut b, 0, 0, 0);
        write(&mut d, &mut cx, 0x107a5);
        let strobed = d.save();
        for value in [0x107a6, 0x108a5, 0x20000] {
            let mut cx = Cx::new(&mut b, 0, 0, 0);
            d.write(&mut cx, config().base, 4, value);
            assert!(cx.refusal().is_some());
            assert_eq!(d.save(), strobed);
        }
    }
}
