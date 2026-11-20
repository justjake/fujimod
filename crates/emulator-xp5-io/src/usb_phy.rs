//! XP5 indexed USB2 PHY programming port, separate from the DWC3 core and bus traffic.
//!
//! X-T5 4.31 helper 0x2bfae38: control word = index[12:8] | data[7:0]; the
//! selected register reads back at base+0x400; a write is the rising edge of
//! bit 16. This is the XP4 protocol (X-T3 0xffb23800). The parallel word at
//! base+4 is read-modify-written by 0x2bfaee8 with masks 0x70 (0x2bfc2f8) and
//! 0x1 (0x2bfc310); XP4 firmware never writes bit 0. No analog or link-ready
//! effects are modeled: the ff80 driver only read-modify-writes these values.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};

const WRITE: u32 = 1 << 16;
const CONTROL: u32 = WRITE | 0x1fff;
const PARALLEL: u32 = 0x71;

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
            || config.parallel & !PARALLEL != 0
        {
            return Err("Invalid XP5 USB PHY wiring or parallel configuration".into());
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
    fn validate(&self, saved: &Saved) -> Result<(), String> {
        let s = &saved.state;
        if saved.config != self.config
            || s.control & !CONTROL != 0
            || s.parallel & !PARALLEL != 0
            || s.control & WRITE != 0
                && s.registers[(s.control >> 8 & 31) as usize] != s.control as u8
        {
            return Err("Invalid XP5 USB PHY checkpoint or changed physical input".into());
        }
        Ok(())
    }
}

impl Device for Phy {
    fn name(&self) -> &'static str {
        "xp5-usb-phy"
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
            cx.fail("XP5 USB PHY word overflow");
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
                if value & !PARALLEL != 0 {
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
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &Saved {
                config: self.config,
                state: self.state.clone(),
            },
        )
    }
    fn validate_checkpoint(
        &self,
        data: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate(&data.decode(self.name())?)
    }
    fn restore_checkpoint(
        &mut self,
        data: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let saved: Saved = data.decode(self.name())?;
        self.validate(&saved)?;
        self.state = saved.state;
        Ok(())
    }
    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let saved: Saved = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.validate(&saved)?;
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
    const BASE: u64 = 0xffc23800;
    fn phy() -> Phy {
        Phy::new(Config {
            base: BASE,
            initial: [0; 32],
            parallel: 0,
        })
        .unwrap()
    }
    fn write(d: &mut Phy, address: u64, value: u64) {
        let mut b = Board;
        let mut cx = Cx::new(&mut b, 0, 0, 0);
        d.write(&mut cx, address, 4, value);
        assert_eq!(cx.refusal(), None);
    }
    fn read(d: &mut Phy, address: u64) -> u64 {
        let mut b = Board;
        let mut cx = Cx::new(&mut b, 0, 0, 0);
        d.read(&mut cx, address, 4)
    }

    #[test]
    fn the_stock_indexed_write_sequence_updates_the_selected_register() {
        let mut d = phy();
        // 0x2bfae38(index 6, data 0x35, mask 0xff): select, read back, data, strobe, release.
        write(&mut d, BASE, 6 << 8 | 1);
        assert_eq!(read(&mut d, BASE + 0x400), 0);
        write(&mut d, BASE, 6 << 8 | 0x35);
        write(&mut d, BASE, 0x10000 | 6 << 8 | 0x35);
        write(&mut d, BASE, 6 << 8 | 0x35);
        write(&mut d, BASE, 6 << 8 | 1);
        assert_eq!(read(&mut d, BASE + 0x400), 0x35);
    }

    #[test]
    fn parallel_bit0_and_bits_6_4_are_accepted_and_others_refused() {
        let mut d = phy();
        write(&mut d, BASE + 4, 0x31);
        assert_eq!(read(&mut d, BASE + 4), 0x31);
        let before = d.save();
        let mut b = Board;
        let mut cx = Cx::new(&mut b, 0, 0, 0);
        d.write(&mut cx, BASE + 4, 4, 0x80);
        assert!(cx.refusal().is_some());
        assert_eq!(d.save(), before);
        let mut restored = phy();
        restored
            .restore_checkpoint(&d.checkpoint().unwrap())
            .unwrap();
        assert_eq!(restored.save(), before);
    }
}
