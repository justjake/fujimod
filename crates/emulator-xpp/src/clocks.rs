//! Clock divisors, gates and five PLLs on the X-Processor Pro clock bank.
//! PLL lock follows power/enable controls and virtual time; reads are passive.
//! Divider updates complete at once: the busy bits at +0x68 read clear
//! (X-T20 2.11 0xc6a6e0 polls them). +0x58..+0x64 hold paired 12-bit
//! divider fields (0xc6ae68).
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
const CONTROL: [u64; 5] = [0x30, 0x3c, 0x48, 0x54, 0x80];
const LOCK: [u32; 5] = [2, 4, 16, 8, 1];
const DIVIDER_BUSY: u64 = 0x68;
const LOCK_STATUS: u64 = 0x6c;
const NAME: &str = "xpp-clock-control";
/// A PLL locks only while bit 26 is set and bit 27 is clear.
const CONTROL_POWERED: u32 = 1 << 26;
const CONTROL_LOCK_HELD: u32 = 1 << 27;
/// Upper control-word bits are rejected unless a row explicitly permits them.
const CONTROL_REJECTED: u32 = 0xf0000000;
/// Control-word configuration fields 28..29: kept as opaque values on the fifth
/// PLL row only, where the model has no behavior attached to them.
const CONTROL_CONFIG: u32 = 0x30000000;
fn word(offset: u64) -> bool {
    offset <= 0x64 || matches!(offset, 0x70 | 0x78 | 0x7c | 0x80)
}
/// Whether the fields of a control word are representable. Bits 30..31 are always
/// refused; the configuration fields 28..29 are valid on the fifth PLL row only.
/// Words outside the control offsets are not constrained.
fn control_valid(offset: u64, value: u32) -> bool {
    let allowed = match CONTROL.iter().position(|&control| control == offset) {
        Some(index) if index + 1 == CONTROL.len() => CONTROL_CONFIG,
        Some(_) => 0,
        None => return true,
    };
    value & (CONTROL_REJECTED & !allowed) == 0
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub base: u64,
    pub lock_ticks: u64,
    /// PLLs already powered, enabled and locked when the model starts, for a
    /// start after the boot stage that brings them up.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub running: Vec<usize>,
}
#[derive(Serialize, Deserialize)]
struct State {
    words: std::collections::BTreeMap<u64, u32>,
    locked: u32,
    due: [Option<u64>; 5],
}
pub struct ClockControl {
    options: Options,
    state: State,
}
impl ClockControl {
    pub fn new(options: Options) -> Result<Self, String> {
        if options.base & 0xfff != 0 || options.lock_ticks == 0 {
            return Err("Invalid clock register base/settle time".into());
        }
        let mut words: std::collections::BTreeMap<u64, u32> = CONTROL
            .into_iter()
            .map(|offset| (offset, CONTROL_LOCK_HELD))
            .collect();
        let mut locked = 0;
        for &index in &options.running {
            if index >= CONTROL.len() || locked & LOCK[index] != 0 {
                return Err("Invalid running PLL".into());
            }
            words.insert(CONTROL[index], CONTROL_POWERED);
            locked |= LOCK[index];
        }
        Ok(Self {
            options,
            state: State {
                words,
                locked,
                due: [None; 5],
            },
        })
    }
    fn transfer(
        &mut self,
        cx: &mut Cx<'_>,
        address: u64,
        size: u8,
        value: Option<u64>,
    ) -> Result<u64, String> {
        if size != 4 || address & 3 != 0 || value.is_some_and(|v| v > u32::MAX.into()) {
            return Err("Clock register width/alignment/value".into());
        }
        self.advance(cx);
        let offset = address
            .checked_sub(self.options.base)
            .ok_or("Clock register below bank")?;
        if offset == LOCK_STATUS {
            if value.is_some() {
                return Err("PLL lock status is read-only".into());
            }
            return Ok(self.state.locked.into());
        }
        if offset == DIVIDER_BUSY {
            if value.is_some() {
                return Err("Divider busy status is read-only".into());
            }
            return Ok(0);
        }
        if !word(offset) {
            return Err(format!("Unsupported clock register {offset:#x}"));
        }
        if let Some(value) = value {
            let value = value as u32;
            if !control_valid(offset, value) {
                return Err("Unsupported PLL control bits".into());
            }
            self.state.words.insert(offset, value);
            for (index, control) in CONTROL.into_iter().enumerate() {
                if (control - 8..=control).contains(&offset) {
                    self.state.locked &= !LOCK[index];
                    self.state.due[index] = if self.state.words[&control]
                        & (CONTROL_POWERED | CONTROL_LOCK_HELD)
                        == CONTROL_POWERED
                    {
                        Some(
                            cx.now()
                                .checked_add(self.options.lock_ticks)
                                .ok_or("PLL deadline overflow")?,
                        )
                    } else {
                        None
                    };
                    if let Some(at) = self.state.due[index] {
                        cx.schedule(at, 0);
                    }
                }
            }
        }
        Ok(self.state.words.get(&offset).copied().unwrap_or(0).into())
    }
}
impl Device for ClockControl {
    fn name(&self) -> &'static str {
        NAME
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match self.transfer(cx, address, size, None) {
            Ok(v) => v,
            Err(e) => {
                cx.fail(e);
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if let Err(e) = self.transfer(cx, address, size, Some(value)) {
            cx.fail(e);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        for (index, due) in self.state.due.iter_mut().enumerate() {
            if due.is_some_and(|at| at <= cx.now()) {
                *due = None;
                self.state.locked |= LOCK[index];
            }
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state.due.into_iter().flatten().min()
    }
    fn save(&self) -> Value {
        json!({"state":self.state})
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(&self.options, &self.state),
        )
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let (options, state): (Options, State) = saved.decode(self.name())?;
        if options != self.options {
            return Err("Clock checkpoint wiring differs".into());
        }
        if state.locked & !31 != 0
            || CONTROL
                .iter()
                .any(|control| !state.words.contains_key(control))
            || state.words.iter().any(|(&offset, &value)| {
                offset & 3 != 0 || !word(offset) || !control_valid(offset, value)
            })
        {
            return Err("Invalid clock checkpoint register shape".into());
        }
        for (index, control) in CONTROL.into_iter().enumerate() {
            if (state.due[index].is_some() || state.locked & LOCK[index] != 0)
                && state.words[&control] & (CONTROL_POWERED | CONTROL_LOCK_HELD) != CONTROL_POWERED
            {
                return Err("Clock checkpoint locks disagree with power/enable controls".into());
            }
        }
        Ok(())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (_, state): (Options, State) = saved.decode(self.name())?;
        self.state = state;
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.checkpoint()?.state())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};
    #[test]
    fn all_plls_lock_after_enable_and_power_down_cancels_pending_lock() {
        let mut d = ClockControl::new(Options {
            base: 0x10000,
            lock_ticks: 5,
            running: vec![],
        })
        .unwrap();
        let mut h = TestHost::new();
        for index in 0..5 {
            write(&mut d, &mut h, 0x10000 + CONTROL[index], 0x04000000);
            let saved = d.checkpoint().unwrap();
            let at = h.now;
            advance(&mut d, &mut h, at + 4);
            assert_eq!(read(&mut d, &mut h, 0x1006c) & LOCK[index] as u64, 0);
            advance(&mut d, &mut h, at + 5);
            assert_ne!(read(&mut d, &mut h, 0x1006c) & LOCK[index] as u64, 0);
            d.restore_checkpoint(&saved).unwrap();
            h.now = at;
            write(&mut d, &mut h, 0x10000 + CONTROL[index], 0x08000000);
            advance(&mut d, &mut h, at + 5);
            assert_eq!(read(&mut d, &mut h, 0x1006c) & LOCK[index] as u64, 0);
        }
        write(&mut d, &mut h, 0x10058, 0x14001);
        assert_eq!(read(&mut d, &mut h, 0x10058), 0x14001);
        assert_eq!(read(&mut d, &mut h, 0x10068), 0);
    }
    #[test]
    fn pll_control_bit_28_is_stored_and_does_not_block_lock() {
        let mut d = ClockControl::new(Options {
            base: 0x10000,
            lock_ticks: 5,
            running: vec![4],
        })
        .unwrap();
        let mut h = TestHost::new();
        write(&mut d, &mut h, 0x10080, 0x1000002e);
        assert_eq!(read(&mut d, &mut h, 0x10080), 0x1000002e);
        assert_eq!(read(&mut d, &mut h, 0x1006c) & u64::from(LOCK[4]), 0);
        write(&mut d, &mut h, 0x10080, 0x1400002e);
        let at = h.now;
        advance(&mut d, &mut h, at + 5);
        assert_ne!(read(&mut d, &mut h, 0x1006c) & u64::from(LOCK[4]), 0);
        assert!(d.restore_checkpoint(&d.checkpoint().unwrap()).is_ok());
    }
    #[test]
    fn running_plls_start_powered_and_locked() {
        let options = |running| Options {
            base: 0x10000,
            lock_ticks: 5,
            running,
        };
        assert!(ClockControl::new(options(vec![0, 0])).is_err());
        assert!(ClockControl::new(options(vec![5])).is_err());
        let mut d = ClockControl::new(options(vec![0, 3])).unwrap();
        let mut h = TestHost::new();
        assert_eq!(read(&mut d, &mut h, 0x1006c), u64::from(LOCK[0] | LOCK[3]));
        assert_eq!(read(&mut d, &mut h, 0x10030), 1 << 26);
        assert_eq!(read(&mut d, &mut h, 0x1003c), 1 << 27);
        let saved = d.checkpoint().unwrap();
        d.restore_checkpoint(&saved).unwrap();
    }
    #[test]
    fn fifth_pll_configuration_fields_roundtrip_and_keep_lock_delay() {
        let mut d = ClockControl::new(Options {
            base: 0x10000,
            lock_ticks: 5,
            running: vec![],
        })
        .unwrap();
        let mut h = TestHost::new();
        let fifth = 0x10000 + CONTROL[4];
        for config in [0, 1 << 28, 1 << 29, 3 << 28] {
            let pending = CONTROL_POWERED | config;
            write(&mut d, &mut h, fifth, pending);
            assert_eq!(read(&mut d, &mut h, fifth), u64::from(pending));
            let saved = d.checkpoint().unwrap();
            let at = h.now;
            advance(&mut d, &mut h, at + 4);
            assert_eq!(
                read(&mut d, &mut h, 0x10000 + LOCK_STATUS) & u64::from(LOCK[4]),
                0
            );
            d.restore_checkpoint(&saved).unwrap();
            assert_eq!(read(&mut d, &mut h, fifth), u64::from(pending));
            assert_eq!(
                read(&mut d, &mut h, 0x10000 + LOCK_STATUS) & u64::from(LOCK[4]),
                0
            );
            advance(&mut d, &mut h, at + 5);
            assert_eq!(
                read(&mut d, &mut h, 0x10000 + LOCK_STATUS) & u64::from(LOCK[4]),
                u64::from(LOCK[4])
            );
            let held = CONTROL_LOCK_HELD | config;
            write(&mut d, &mut h, fifth, held);
            assert_eq!(read(&mut d, &mut h, fifth), u64::from(held));
            assert_eq!(
                read(&mut d, &mut h, 0x10000 + LOCK_STATUS) & u64::from(LOCK[4]),
                0
            );
        }
    }
    #[test]
    fn fifth_pll_refuses_unsupported_bits_and_other_rows_refuse_configuration_fields() {
        let mut d = ClockControl::new(Options {
            base: 0x10000,
            lock_ticks: 5,
            running: vec![],
        })
        .unwrap();
        let mut h = TestHost::new();
        let fifth = 0x10000 + CONTROL[4];
        for bad in [1 << 30, 1 << 31, 3 << 30] {
            let mut cx = h.cx();
            d.write(&mut cx, fifth, 4, u64::from(CONTROL_POWERED | bad));
            assert!(cx.refusal().is_some());
            assert_eq!(read(&mut d, &mut h, fifth), u64::from(CONTROL_LOCK_HELD));
        }
        for control in &CONTROL[..CONTROL.len() - 1] {
            for bit in [1 << 28, 1 << 29, 1 << 30, 1 << 31] {
                let mut cx = h.cx();
                let at = 0x10000 + control;
                d.write(&mut cx, at, 4, u64::from(CONTROL_POWERED | bit));
                assert!(cx.refusal().is_some());
                assert_eq!(read(&mut d, &mut h, at), u64::from(CONTROL_LOCK_HELD));
            }
        }
    }
    #[test]
    fn checkpoint_refuses_unsupported_control_fields() {
        let options = Options {
            base: 0x10000,
            lock_ticks: 5,
            running: vec![],
        };
        let mut d = ClockControl::new(options.clone()).unwrap();
        let encode = |offset: u64, value: u32| {
            let mut state = State {
                words: CONTROL
                    .into_iter()
                    .map(|control| (control, CONTROL_LOCK_HELD))
                    .collect(),
                locked: 0,
                due: [None; 5],
            };
            state.words.insert(offset, value);
            fujimod_emulator_devices::checkpoint::Checkpoint::encode(NAME, &(&options, &state))
                .unwrap()
        };
        let fifth = CONTROL[4];
        d.validate_checkpoint(&encode(fifth, CONTROL_POWERED | CONTROL_CONFIG))
            .unwrap();
        let bad = encode(fifth, CONTROL_POWERED | 1 << 30);
        assert!(d.validate_checkpoint(&bad).is_err());
        assert!(d.restore_checkpoint(&bad).is_err());
        assert!(
            d.validate_checkpoint(&encode(CONTROL[0], CONTROL_POWERED | CONTROL_CONFIG))
                .is_err()
        );
        assert!(
            d.validate_checkpoint(&encode(CONTROL[0], CONTROL_POWERED | 1 << 31))
                .is_err()
        );
    }
}
