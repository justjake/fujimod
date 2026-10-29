//! X-Processor Pro DRAM controller words the running firmware uses.
//!
//! X-T20 2.11 0xc8f09c reads bits 8..10 of +0xec, the device's refresh-rate
//! level (taken from SDRAM mode register 4), and rewrites the low half of the
//! refresh words +0x9c and +0xa0: 0xc30 below level 4, 0x618 at level 4. The
//! level is a declared input; the other +0xec bits read 0. +0x9c and +0xa0
//! read back their last write and start at 0, since the boot code that
//! programs them before the kernel is not modeled.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Register window size in bytes.
pub const WINDOW: u64 = 0x1000;
const REFRESH: [u64; 2] = [0x9c, 0xa0];
const STATUS: u64 = 0xec;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    refresh: [u32; 2],
}
pub struct Controller {
    base: u64,
    refresh_level: u8,
    state: State,
}
impl Controller {
    pub fn new(base: u64, refresh_level: u8) -> Result<Self, String> {
        if base & 0xfff != 0 || refresh_level > 7 {
            return Err("Invalid X-Processor Pro DRAM controller base or refresh level".into());
        }
        Ok(Self {
            base,
            refresh_level,
            state: State::default(),
        })
    }
}
impl Device for Controller {
    fn name(&self) -> &'static str {
        "xpp-dram-controller"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(self.base);
        if size != 4 {
            cx.unmodeled();
            return 0;
        }
        u64::from(match offset {
            STATUS => u32::from(self.refresh_level) << 8,
            o => match REFRESH.iter().position(|r| *r == o) {
                Some(n) => self.state.refresh[n],
                None => {
                    cx.unmodeled();
                    0
                }
            },
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(self.base);
        match REFRESH.iter().position(|r| *r == offset) {
            Some(n) if size == 4 && value <= u32::MAX.into() => {
                self.state.refresh[n] = value as u32
            }
            _ => cx.unmodeled(),
        }
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        self.state = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, read, write};

    const BASE: u64 = 0xfff0e000;

    #[test]
    fn refresh_level_reads_in_bits_8_to_10_and_refresh_words_read_back() {
        let mut dram = Controller::new(BASE, 2).unwrap();
        let mut h = TestHost::new();
        assert_eq!(read(&mut dram, &mut h, BASE + 0xec) >> 8 & 7, 2);
        // 0xc8f190: keep the high half, refresh interval 0xc30 in the low half.
        write(&mut dram, &mut h, BASE + 0x9c, 0xc30);
        write(&mut dram, &mut h, BASE + 0xa0, 0x1_0c30);
        let saved = dram.save();
        write(&mut dram, &mut h, BASE + 0xa0, 0);
        dram.restore(&saved).unwrap();
        assert_eq!(read(&mut dram, &mut h, BASE + 0x9c), 0xc30);
        assert_eq!(read(&mut dram, &mut h, BASE + 0xa0), 0x1_0c30);
        let mut cx = h.cx();
        dram.read(&mut cx, BASE + 0x10, 4);
        assert!(cx.is_unmodeled());
        assert!(Controller::new(BASE, 8).is_err());
    }
}
