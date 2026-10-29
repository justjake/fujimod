//! X-Processor Pro power control at 0xfff10000: supply hold and power domains.
//!
//! X-T20 2.11 stage 2 (0xc098..0xc4ec) and section 5 (0xc4cbec..0xc4d284)
//! switch five domains, numbered 2..6, with masks 0x1, 0x20, 0x40, 0x80 and
//! 0x100. Turning a domain on (0xc4d108) sets its mask in +0x0c, waits for it
//! in the status word +0x2c, sets it in +0x08 and waits for the mask shifted
//! left by 16. Turning it off (0xc4cbec) clears the mask in +0x14 and +0x08,
//! waits for the shifted mask to clear, clears it in +0x10 and +0x0c and waits
//! until the status masked with the domain and bits 30..31 reads 0xc0000000.
//! The status therefore carries +0x0c bits 0..15 and +0x08 bits 0..13 in bits
//! 16..29, with bits 30 and 31 set: switching completes at once. 0xc4ce30
//! sets the mask in +0x14 and waits for it to read back, 0xc25c sets it in
//! +0x10 and clears it in +0x14, and 0xc4cf20/0xc4cfbc read-modify-write
//! bits 0, 1 and 4 of +0x38. These words read back their last write and
//! start at 0; what the stage before the kernel leaves in them is not known.
//!
//! +0x00 reads 6 and +0x04 reads 1 on the running camera (service-mode reads).
//! Stage 1 tests +0x00 bit 0. 0xc4cb94 sets or clears +0x04 bit 0, and its
//! only caller 0xc4c9a4 clears it on the shutdown and fatal-stop paths, which
//! end in endless loops: clearing it releases the supply hold and ends the run.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Register window size in bytes.
pub const WINDOW: u64 = 0x1000;
const IDENTITY: u64 = 0x00;
const IDENTITY_VALUE: u32 = 6;
const HOLD: u64 = 0x04;
const STATUS: u64 = 0x2c;
const WORDS: [u64; 5] = [0x08, 0x0c, 0x10, 0x14, 0x38];
const ENABLE: usize = 0;
const REQUEST: usize = 1;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: [u32; WORDS.len()],
}
pub struct Control {
    base: u64,
    state: State,
}
impl Control {
    pub fn new(base: u64) -> Result<Self, String> {
        if base & 0xfff != 0 {
            return Err("Invalid X-Processor Pro power control base".into());
        }
        Ok(Self {
            base,
            state: State::default(),
        })
    }
    fn status(&self) -> u32 {
        self.state.words[REQUEST] & 0xffff | (self.state.words[ENABLE] & 0x3fff) << 16 | 0xc000_0000
    }
}
impl Device for Control {
    fn name(&self) -> &'static str {
        "xpp-power-control"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(self.base);
        if size != 4 {
            cx.unmodeled();
            return 0;
        }
        u64::from(match offset {
            IDENTITY => IDENTITY_VALUE,
            HOLD => 1,
            STATUS => self.status(),
            o => match WORDS.iter().position(|w| *w == o) {
                Some(n) => self.state.words[n],
                None => {
                    cx.unmodeled();
                    0
                }
            },
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(self.base);
        if size != 4 || value > u32::MAX.into() {
            cx.unmodeled();
            return;
        }
        if offset == HOLD {
            if value & 1 == 0 {
                cx.fail("supply off: power hold FFF10004 bit 0 released");
            }
            return;
        }
        match WORDS.iter().position(|w| *w == offset) {
            Some(n) => self.state.words[n] = value as u32,
            None => cx.unmodeled(),
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

    const BASE: u64 = 0xfff10000;

    #[test]
    fn domains_switch_at_once_and_the_status_follows_their_words() {
        let mut power = Control::new(BASE).unwrap();
        let mut h = TestHost::new();
        assert_eq!(read(&mut power, &mut h, BASE), 6);
        assert_eq!(read(&mut power, &mut h, BASE + 4), 1);
        // 0xc4d108, domain 6 (mask 0x100): request, then enable.
        write(&mut power, &mut h, BASE + 0x0c, 0x100);
        assert_eq!(read(&mut power, &mut h, BASE + 0x2c) & 0x100, 0x100);
        write(&mut power, &mut h, BASE + 0x08, 0x100);
        assert_eq!(read(&mut power, &mut h, BASE + 0x2c), 0xc100_0100);
        let saved = power.save();
        // 0xc4cbec: enable off and wait, then request off and wait.
        write(&mut power, &mut h, BASE + 0x08, 0);
        assert_eq!(read(&mut power, &mut h, BASE + 0x2c) & 0x100_0000, 0);
        write(&mut power, &mut h, BASE + 0x0c, 0);
        assert_eq!(
            read(&mut power, &mut h, BASE + 0x2c) & 0xc000_0100,
            0xc000_0000
        );
        power.restore(&saved).unwrap();
        assert_eq!(read(&mut power, &mut h, BASE + 0x08), 0x100);
        write(&mut power, &mut h, BASE + 0x38, 0x13);
        assert_eq!(read(&mut power, &mut h, BASE + 0x38), 0x13);
        let mut cx = h.cx();
        power.write(&mut cx, BASE + 0x2c, 4, 0);
        assert!(cx.is_unmodeled());
        assert!(Control::new(BASE + 4).is_err());
    }

    #[test]
    fn releasing_the_supply_hold_ends_the_run() {
        let mut power = Control::new(BASE).unwrap();
        let mut h = TestHost::new();
        write(&mut power, &mut h, BASE + 4, 1);
        let mut cx = h.cx();
        power.write(&mut cx, BASE + 4, 4, 0);
        assert!(cx.refusal().is_some_and(|f| f.contains("supply off")));
        assert!(!cx.is_unmodeled());
    }
}
