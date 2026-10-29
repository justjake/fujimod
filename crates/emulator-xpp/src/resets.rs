//! X-Processor Pro reset and gate controls at 0xfff0a000.
//!
//! Words read back their last write. The X-T20 2.11 SD start-up 0xc65b18
//! drives +0x1c bit 18, 19 or 20 low and then high for SD channel 0, 1 or 2
//! and then sets bit 0. A falling edge of a channel bit is a hardware reset
//! pulse for that SD host: the UHS-II fallback at 0xc65dc4 runs it before it
//! rewrites power and clock words by read-modify-write, which relies on the
//! cleared UHS-II settings. 0xc65a04/0xc65e58 set and clear +0x14 bit 22
//! around the UHS-II attempt; 0xc621d0/0xc620b8 set and clear pad bits in
//! +0x70 and +0x88. What those bits gate is not modeled.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Register window size in bytes.
pub const WINDOW: u64 = 0x1000;
const WORDS: [u64; 8] = [0x08, 0x14, 0x1c, 0x20, 0x6c, 0x70, 0x88, 0x9c];
const SD_RESETS: u64 = 0x1c;
const SD_CHANNELS: usize = 3;
const SD_FIRST_BIT: u32 = 18;

/// Count of reset pulses on one line; a receiver resets once per new pulse.
#[derive(Clone, Default)]
pub struct ResetPulses(Arc<AtomicU64>);
impl ResetPulses {
    pub fn count(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
    fn set(&self, count: u64) {
        self.0.store(count, Ordering::Relaxed);
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    pulses: [u64; SD_CHANNELS],
}
pub struct Gates {
    base: u64,
    words: BTreeMap<u64, u32>,
    sd: [ResetPulses; SD_CHANNELS],
}
impl Gates {
    pub fn new(base: u64) -> Result<Self, String> {
        if base & 0xfff != 0 {
            return Err("Invalid X-Processor Pro reset/gate base".into());
        }
        Ok(Self {
            base,
            words: BTreeMap::new(),
            sd: Default::default(),
        })
    }
    /// The reset line of SD channel `channel`.
    pub fn sd_reset(&self, channel: usize) -> Result<ResetPulses, String> {
        self.sd
            .get(channel)
            .cloned()
            .ok_or_else(|| "Invalid SD reset channel".into())
    }
    fn offset(&self, address: u64, size: u8) -> Option<u64> {
        let offset = address.checked_sub(self.base)?;
        (size == 4 && WORDS.contains(&offset)).then_some(offset)
    }
}
impl Device for Gates {
    fn name(&self) -> &'static str {
        "xpp-reset-gates"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match self.offset(address, size) {
            Some(offset) => self.words.get(&offset).copied().unwrap_or(0).into(),
            None => {
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
        let value = value as u32;
        if offset == SD_RESETS {
            let old = self.words.get(&offset).copied().unwrap_or(0);
            for (channel, line) in self.sd.iter().enumerate() {
                let bit = 1 << (SD_FIRST_BIT + channel as u32);
                if old & bit != 0 && value & bit == 0 {
                    line.set(line.count() + 1);
                }
            }
        }
        self.words.insert(offset, value);
    }
    fn save(&self) -> Value {
        serde_json::to_value(State {
            words: self.words.clone(),
            pulses: std::array::from_fn(|channel| self.sd[channel].count()),
        })
        .unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state.words.keys().any(|offset| !WORDS.contains(offset)) {
            return Err("Invalid reset/gate checkpoint".into());
        }
        self.words = state.words;
        for (line, count) in self.sd.iter().zip(state.pulses) {
            line.set(count);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, read, write};

    const BASE: u64 = 0xfff0a000;

    #[test]
    fn falling_sd_bits_pulse_their_channel_and_words_read_back() {
        let mut gates = Gates::new(BASE).unwrap();
        let mut h = TestHost::new();
        let line = gates.sd_reset(0).unwrap();
        // 0xc65b50: clear bit 18 (already low), set it, then set bit 0.
        for value in [0, 0x40000, 0x40001] {
            write(&mut gates, &mut h, BASE + 0x1c, value);
        }
        assert_eq!(line.count(), 0);
        assert_eq!(read(&mut gates, &mut h, BASE + 0x1c), 0x40001);
        for value in [0x1, 0x40001] {
            write(&mut gates, &mut h, BASE + 0x1c, value);
        }
        assert_eq!(line.count(), 1);
        assert_eq!(gates.sd_reset(1).unwrap().count(), 0);
        write(&mut gates, &mut h, BASE + 0x14, 0x400000);
        let saved = gates.save();
        write(&mut gates, &mut h, BASE + 0x1c, 0);
        assert_eq!(line.count(), 2);
        gates.restore(&saved).unwrap();
        assert_eq!(line.count(), 1);
        assert_eq!(read(&mut gates, &mut h, BASE + 0x14), 0x400000);
        assert!(gates.sd_reset(3).is_err());
    }
}
