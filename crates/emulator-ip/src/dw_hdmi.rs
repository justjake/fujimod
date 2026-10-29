//! DesignWare HDMI TX controller with no sink attached.
//!
//! An idle law, not a transmitter: byte registers sit at four-byte strides.
//! PHY_STAT0 (0x3004) reads 0: no PHY lock, hot-plug detect low, no
//! receiver sense. The interrupt status registers IH_FC_STAT0..
//! IH_AHBDMAAUD_STAT0 (0x100..0x109) read 0 and accept clearing writes, as
//! no event is pending. The interrupt mute registers (0x180..0x189 and
//! IH_MUTE 0x1ff), the frame composer (0x1000..0x12ff) and the PHY
//! configuration, interrupt, mask and polarity registers (0x3000..0x3003,
//! 0x3005..0x3007) read back their last write and start at 0. Video,
//! audio, PHY configuration over I2C, HDCP and CEC are not modeled.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::ops::RangeInclusive;

/// Register window size in bytes.
pub const WINDOW: u64 = 0x2_0000;
const STRIDE: u64 = 4;
const PHY_STAT0: u64 = 0x3004;
const STATUS: RangeInclusive<u64> = 0x100..=0x109;
const STORED: [RangeInclusive<u64>; 5] = [
    0x180..=0x189,
    0x1ff..=0x1ff,
    0x1000..=0x12ff,
    0x3000..=0x3003,
    0x3005..=0x3007,
];

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    registers: BTreeMap<u64, u8>,
}
pub struct Transmitter {
    base: u64,
    state: State,
}
impl Transmitter {
    pub fn new(base: u64) -> Result<Self, String> {
        if base & (WINDOW - 1) != 0 {
            return Err("Invalid DesignWare HDMI TX base".into());
        }
        Ok(Self {
            base,
            state: State::default(),
        })
    }
    fn register(&self, address: u64, size: u8) -> Option<u64> {
        let offset = address.checked_sub(self.base)?;
        (size == 4 && offset % STRIDE == 0 && offset < WINDOW).then_some(offset / STRIDE)
    }
    fn stored(register: u64) -> bool {
        STORED.iter().any(|range| range.contains(&register))
    }
}
impl Device for Transmitter {
    fn name(&self) -> &'static str {
        "designware-hdmi-tx"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match self.register(address, size) {
            Some(PHY_STAT0) => 0,
            Some(r) if STATUS.contains(&r) => 0,
            Some(r) if Self::stored(r) => self.state.registers.get(&r).copied().unwrap_or(0).into(),
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        match self.register(address, size) {
            Some(r) if STATUS.contains(&r) && value <= 0xff => {}
            Some(r) if Self::stored(r) && value <= 0xff => {
                self.state.registers.insert(r, value as u8);
            }
            _ => cx.unmodeled(),
        }
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state.registers.keys().any(|r| !Self::stored(*r)) {
            return Err("Invalid DesignWare HDMI TX checkpoint".into());
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, read, write};

    const BASE: u64 = 0xf8000000;

    #[test]
    fn no_sink_reads_no_hot_plug_and_no_events_while_settings_read_back() {
        let mut hdmi = Transmitter::new(BASE).unwrap();
        let mut h = TestHost::new();
        // X-T20 2.11 0xcb7df8 reads PHY_STAT0; 0xcbe578 takes bit 1 as hot plug.
        assert_eq!(read(&mut hdmi, &mut h, BASE + 0xc010), 0);
        write(&mut hdmi, &mut h, BASE + 0x400, 0xff);
        assert_eq!(read(&mut hdmi, &mut h, BASE + 0x400), 0);
        write(&mut hdmi, &mut h, BASE + 0x7fc, 3);
        write(&mut hdmi, &mut h, BASE + 0xc018, 0xf2);
        let saved = hdmi.save();
        write(&mut hdmi, &mut h, BASE + 0x7fc, 0);
        hdmi.restore(&saved).unwrap();
        assert_eq!(read(&mut hdmi, &mut h, BASE + 0x7fc), 3);
        assert_eq!(read(&mut hdmi, &mut h, BASE + 0xc018), 0xf2);
        let mut cx = h.cx();
        hdmi.write(&mut cx, BASE + 0xc010, 4, 1);
        assert!(cx.is_unmodeled());
        let mut cx = h.cx();
        hdmi.read(&mut cx, BASE, 4);
        assert!(cx.is_unmodeled());
    }
}
