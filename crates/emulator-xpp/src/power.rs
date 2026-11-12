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
    due: Option<u64>,
    settled: bool,
    control: u32,
    hold: u32,
}
pub struct Control {
    base: u64,
    state: State,
    settle_ticks: Option<u64>,
}
impl Control {
    pub fn new(base: u64) -> Result<Self, String> {
        if base & 0xfff != 0 {
            return Err("Invalid X-Processor Pro power control base".into());
        }
        Ok(Self {
            base,
            state: State {
                control: IDENTITY_VALUE,
                hold: 1,
                ..State::default()
            },
            settle_ticks: None,
        })
    }
    /// Cold-start domain settling, in declared guest ticks.
    pub fn with_startup(mut self, ticks: u64) -> Result<Self, String> {
        if ticks == 0 {
            return Err("Power startup clock must be positive".into());
        }
        self.settle_ticks = Some(ticks);
        self.state.control = 0;
        self.state.hold = 0;
        Ok(self)
    }
    fn status(&self) -> u32 {
        let ready = if self.settle_ticks.is_none() || self.state.settled {
            0xc000_0000
        } else {
            0
        };
        self.state.words[REQUEST] & 0xffff | (self.state.words[ENABLE] & 0x3fff) << 16 | ready
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
            IDENTITY => self.state.control,
            HOLD => self.state.hold,
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
        if offset == IDENTITY && self.settle_ticks.is_some() {
            self.state.control = value as u32;
            return;
        }
        if offset == HOLD {
            if value & 1 == 0 && self.state.hold & 1 != 0 {
                cx.fail("supply off: power hold FFF10004 bit 0 released");
            }
            self.state.hold = value as u32;
            return;
        }
        match WORDS.iter().position(|w| *w == offset) {
            Some(n) => {
                self.state.words[n] = value as u32;
                if offset == 0x0c
                    && let Some(ticks) = self.settle_ticks
                {
                    self.state.settled = false;
                    self.state.due = cx.now().checked_add(ticks);
                    if self.state.due.is_none() {
                        cx.fail("Power startup deadline overflow");
                    }
                }
            }
            None => cx.unmodeled(),
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.state.due.is_some_and(|at| at <= cx.now()) {
            self.state.due = None;
            self.state.settled = true;
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state.due
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        self.state = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        Ok(())
    }
    fujimod_emulator_devices::checkpoint_state!(state: State, Self::check);
}
impl Control {
    fn check(&self, state: &State) -> Result<(), String> {
        if state.due.is_some() && self.settle_ticks.is_none() {
            return Err("Invalid X-Processor Pro power control checkpoint".into());
        }
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
    fn cold_start_settles_domains_and_preserves_supply_hold_edges() {
        use fujimod_emulator_devices::test_host::advance;
        let mut power = Control::new(BASE).unwrap().with_startup(2).unwrap();
        let mut host = TestHost::new();
        assert_eq!(read(&mut power, &mut host, BASE + 4), 0);
        write(&mut power, &mut host, BASE + 4, 0);
        write(&mut power, &mut host, BASE, 6);
        write(&mut power, &mut host, BASE + 4, 1);
        write(&mut power, &mut host, BASE + 0xc, 0x100);
        assert_eq!(power.next_deadline(0), Some(2));
        assert_eq!(read(&mut power, &mut host, BASE + 0x2c) & 0xc0000000, 0);
        advance(&mut power, &mut host, 2);
        assert_eq!(read(&mut power, &mut host, BASE + 0x2c), 0xc0000100);
        let mut cx = host.cx();
        power.write(&mut cx, BASE + 4, 4, 0);
        assert!(cx.refusal().is_some_and(|f| f.contains("supply off")));
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
