//! Clock divisors, gates and five PLLs on the X-Processor Pro clock bank.
//! PLL lock follows power/enable controls and virtual time; reads are passive.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
const CONTROL: [u64; 5] = [0x30, 0x3c, 0x48, 0x54, 0x80];
const LOCK: [u32; 5] = [2, 4, 16, 8, 1];
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub base: u64,
    pub lock_ticks: u64,
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
        Ok(Self {
            options,
            state: State {
                words: CONTROL
                    .into_iter()
                    .map(|offset| (offset, 1 << 27))
                    .collect(),
                locked: 0,
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
        if offset == 0x6c {
            if value.is_some() {
                return Err("PLL lock status is read-only".into());
            }
            return Ok(self.state.locked.into());
        }
        if !(offset <= 0x54 || matches!(offset, 0x70 | 0x78 | 0x7c | 0x80)) {
            return Err(format!("Unsupported clock register {offset:#x}"));
        }
        if let Some(value) = value {
            if CONTROL.contains(&offset) && value & 0xf0000000 != 0 {
                return Err("Unsupported PLL control bits".into());
            }
            self.state.words.insert(offset, value as u32);
            for (index, control) in CONTROL.into_iter().enumerate() {
                if (control - 8..=control).contains(&offset) {
                    self.state.locked &= !LOCK[index];
                    self.state.due[index] = if self.state.words[&control] & 0x0c000000 == 0x04000000
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
        "xpp-clock-control"
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
                offset & 3 != 0
                    || !(offset <= 0x54 || matches!(offset, 0x70 | 0x78 | 0x7c | 0x80))
                    || CONTROL.contains(&offset) && value & 0xf0000000 != 0
            })
        {
            return Err("Invalid clock checkpoint register shape".into());
        }
        for (index, control) in CONTROL.into_iter().enumerate() {
            if (state.due[index].is_some() || state.locked & LOCK[index] != 0)
                && state.words[&control] & 0x0c000000 != 0x04000000
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
    }
}
