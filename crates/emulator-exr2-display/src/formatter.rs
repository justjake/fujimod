//! Register banks used by output-format commits.
//!
//! The model retains masked words and the bit-28 latch request. The latch
//! delay is an explicit inferred virtual delay; pixel formats and frame
//! effects remain outside this device.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const BASES: [u64; 2] = [0xfff69000, 0xfff6c000];
pub const DEFAULT_LATCH_DELAY: u64 = 2;
const STROBE: u32 = 1 << 28;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(default = "default_latch_delay")]
    pub latch_delay: u64,
}

const fn default_latch_delay() -> u64 {
    DEFAULT_LATCH_DELAY
}

impl Default for Options {
    fn default() -> Self {
        Self {
            latch_delay: DEFAULT_LATCH_DELAY,
        }
    }
}

const MASKS: &[(u16, u32)] = &[
    (0x00, 0xfffff7f7),
    (0x04, 0x00001fff),
    (0x20, 0x7fff0fff),
    (0x24, 0x00000fff),
    (0x28, 0x1fff1fff),
    (0x2c, 0x1fff1fff),
    (0x34, u32::MAX),
    (0x38, 0x03ff07ff),
    (0x3c, 0x03ff07ff),
    (0x40, 0x03ff07ff),
    (0x44, 0x03ff07ff),
    (0x48, 0x03ff07ff),
    (0x4c, 0x1fff1fff),
    (0x50, 0x00001fff),
    (0x54, 0x1fff1fff),
    (0x58, 0x00001fff),
    (0x5c, 0x1fff1fff),
    (0x60, 0x00001fff),
    (0x64, u32::MAX),
    (0x68, u32::MAX),
    (0x6c, u32::MAX),
    (0x70, u32::MAX),
    (0x74, u32::MAX),
    (0x78, 0x00ffffff),
    (0x7c, u32::MAX),
    (0x80, u32::MAX),
    (0x84, u32::MAX),
    (0x88, u32::MAX),
    (0x8c, u32::MAX),
    (0x90, u32::MAX),
    (0x94, u32::MAX),
    (0x98, u32::MAX),
    (0x9c, u32::MAX),
    (0xa0, u32::MAX),
    (0xa4, u32::MAX),
    (0xa8, u32::MAX),
    (0xac, u32::MAX),
    (0xb0, u32::MAX),
    (0xb4, u32::MAX),
    (0xb8, u32::MAX),
    (0xbc, u32::MAX),
    (0xc0, u32::MAX),
    (0xc4, 0xf1ffdfff),
    (0xc8, 0),
    (0x100, 0x1fff1fff),
    (0x104, 0x00001fff),
    (0x108, 0x1fff1fff),
    (0x10c, 0x00001fff),
    (0x110, 0x1fff1fff),
    (0x114, 0x00001fff),
];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Bank {
    registers: BTreeMap<u16, u32>,
    latched: BTreeMap<u16, u32>,
    pending: Option<BTreeMap<u16, u32>>,
    deadline: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct State {
    banks: [Bank; 2],
    latch_delay: u64,
}

/// Two independent, register-only output-format banks.
pub struct Formatter {
    state: State,
}

impl Formatter {
    pub fn new(options: Options) -> Result<Self, String> {
        let latch_delay = options.latch_delay;
        if latch_delay == 0 {
            return Err("display formatter latch delay must be positive".into());
        }
        Ok(Self {
            state: State {
                banks: Default::default(),
                latch_delay,
            },
        })
    }

    fn address(address: u64, size: u8) -> Option<(usize, u16)> {
        for (instance, base) in BASES.iter().enumerate() {
            if (*base..*base + 0x1000).contains(&address) {
                let offset = (address - base) as u16;
                if size != 4 || offset & 3 != 0 || !MASKS.iter().any(|(known, _)| *known == offset)
                {
                    return None;
                }
                return Some((instance, offset));
            }
        }
        None
    }

    fn mask(offset: u16) -> u32 {
        MASKS
            .iter()
            .find_map(|(known, mask)| (*known == offset).then_some(*mask))
            .unwrap()
    }

    fn valid_registers(registers: &BTreeMap<u16, u32>) -> bool {
        registers.iter().all(|(offset, value)| {
            MASKS
                .iter()
                .find(|(known, _)| known == offset)
                .is_some_and(|(_, mask)| value & !mask == 0)
        })
    }

    fn advance(&mut self, cx: &mut Cx<'_>) {
        let now = cx.now();
        for bank in &mut self.state.banks {
            if bank.deadline.is_some_and(|deadline| deadline <= now) {
                bank.latched = bank.pending.take().unwrap_or_default();
                bank.deadline = None;
                if let Some(control) = bank.registers.get_mut(&0) {
                    *control &= !STROBE;
                }
            }
        }
    }

    fn validate_state(state: &State) -> bool {
        state.latch_delay > 0
            && state.banks.iter().all(|bank| {
                Self::valid_registers(&bank.registers)
                    && Self::valid_registers(&bank.latched)
                    && bank.pending.as_ref().is_none_or(Self::valid_registers)
                    && bank.pending.is_some() == bank.deadline.is_some()
                    && bank.pending.as_ref().is_none_or(|pending| {
                        pending.get(&0).copied().unwrap_or(0) & STROBE == 0
                            && bank.registers.get(&0).copied().unwrap_or(0) & STROBE != 0
                    })
            })
    }
}

impl Default for Formatter {
    fn default() -> Self {
        Self::new(Options::default()).expect("default latch delay is positive")
    }
}

impl Device for Formatter {
    fujimod_emulator_devices::checkpoint_fields!(state: State);

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr2-display-formatter"
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some((instance, offset)) = Self::address(address, size) else {
            cx.unmodeled();
            return 0;
        };
        self.advance(cx);
        u64::from(
            self.state.banks[instance]
                .registers
                .get(&offset)
                .copied()
                .unwrap_or(0),
        )
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some((instance, offset)) = Self::address(address, size) else {
            cx.unmodeled();
            return;
        };
        if value > u64::from(u32::MAX) || (value as u32) & !Self::mask(offset) != 0 {
            cx.fail(format!(
                "unrecovered display formatter bits {address:#x}={value:#x}"
            ));
            return;
        }
        self.advance(cx);
        let bank = &mut self.state.banks[instance];
        if bank.pending.is_some() {
            cx.fail("display formatter mutation during pending latch is unproved");
            return;
        }
        let value = value as u32;
        if offset == 0 && value & STROBE != 0 {
            let Some(previous) = bank.registers.get(&0).copied() else {
                cx.fail("display formatter latch requires the expected control read-modify-write");
                return;
            };
            if value & !STROBE != previous {
                cx.fail("display formatter latch requires the expected control read-modify-write");
                return;
            }
            let Some(deadline) = cx.now().checked_add(self.state.latch_delay) else {
                cx.fail("display formatter latch deadline overflow");
                return;
            };
            bank.pending = Some(bank.registers.clone());
            bank.deadline = Some(deadline);
            cx.schedule(deadline, instance as u64);
        }
        bank.registers.insert(offset, value);
    }

    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state
            .banks
            .iter()
            .filter_map(|bank| bank.deadline)
            .min()
    }

    fn event(&mut self, cx: &mut Cx<'_>, _token: u64) {
        self.advance(cx);
    }

    fn save(&self) -> Value {
        serde_json::to_value(&self.state).expect("formatter state serializes")
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid display formatter state: {error}"))?;
        if !Self::validate_state(&state) {
            return Err("invalid display formatter state".into());
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{Host, Record};

    #[derive(Default)]
    struct Clock {
        now: u64,
        deadlines: Vec<(usize, u64, u64)>,
    }

    impl Host for Clock {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, device: usize, at: u64, token: u64) {
            self.deadlines.push((device, at, token));
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            unreachable!()
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            unreachable!()
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            unreachable!()
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            unreachable!()
        }
        fn record(&mut self, _: Record) {}
    }

    fn write(model: &mut Formatter, host: &mut Clock, address: u64, value: u64) -> Option<String> {
        let mut cx = Cx::new(host, 0, 0, 7);
        model.write(&mut cx, address, 4, value);
        cx.take_refusal()
    }

    fn read(model: &mut Formatter, host: &mut Clock, address: u64) -> (u64, Option<String>) {
        let mut cx = Cx::new(host, 0, 0, 7);
        let value = model.read(&mut cx, address, 4);
        (value, cx.take_refusal())
    }

    #[test]
    fn latches_a_copy_after_the_inferred_deadline() {
        let mut model = Formatter::new(Options::default()).unwrap();
        let mut host = Clock {
            now: 10,
            ..Clock::default()
        };
        let base = BASES[0];
        assert_eq!(write(&mut model, &mut host, base, 0x101), None);
        assert_eq!(write(&mut model, &mut host, base + 4, 0x123), None);
        assert_eq!(write(&mut model, &mut host, base, 0x10000101), None);
        assert_eq!(model.next_deadline(host.now), Some(12));
        assert_eq!(host.deadlines, [(7, 12, 0)]);

        host.now = 11;
        assert_eq!(read(&mut model, &mut host, base), (0x10000101, None));
        host.now = 12;
        assert_eq!(read(&mut model, &mut host, base), (0x101, None));
        assert_eq!(model.state.banks[0].latched.get(&0), Some(&0x101));
        assert_eq!(model.state.banks[0].latched.get(&4), Some(&0x123));
        assert_eq!(model.next_deadline(host.now), None);
    }

    #[test]
    fn banks_have_independent_latch_deadlines() {
        let mut model = Formatter::new(Options::default()).unwrap();
        let mut host = Clock {
            now: 4,
            ..Clock::default()
        };
        for base in BASES {
            assert_eq!(write(&mut model, &mut host, base, 1), None);
            assert_eq!(write(&mut model, &mut host, base, 0x10000001), None);
            host.now += 1;
        }
        assert_eq!(model.next_deadline(host.now), Some(6));
        host.now = 6;
        let mut cx = Cx::new(&mut host, 0, 0, 7);
        model.advance(&mut cx);
        assert_eq!(model.state.banks[0].deadline, None);
        assert_eq!(model.state.banks[1].deadline, Some(7));
    }

    #[test]
    fn refuses_unknown_bits_first_strobe_and_pending_mutation() {
        let mut model = Formatter::new(Options::default()).unwrap();
        let mut host = Clock::default();
        let base = BASES[0];
        assert!(write(&mut model, &mut host, base, 0x10000001).is_some());
        assert!(write(&mut model, &mut host, base, 0x800).is_some());
        assert_eq!(write(&mut model, &mut host, base, 1), None);
        assert_eq!(write(&mut model, &mut host, base, 0x10000001), None);
        assert!(write(&mut model, &mut host, base + 4, 1).is_some());
        assert!(read(&mut model, &mut host, base + 8).1.is_some());
    }

    #[test]
    fn checkpoint_keeps_latch_deadline_and_rejects_invalid_masks() {
        let mut model = Formatter::new(Options::default()).unwrap();
        let mut host = Clock::default();
        let base = BASES[1];
        assert_eq!(write(&mut model, &mut host, base, 1), None);
        assert_eq!(write(&mut model, &mut host, base, 0x10000001), None);

        let mut restored = Formatter::new(Options::default()).unwrap();
        restored.restore(&model.save()).unwrap();
        assert_eq!(restored.next_deadline(0), Some(DEFAULT_LATCH_DELAY));
        assert!(
            restored
                .restore(&serde_json::json!({"banks":[], "latch_delay":2}))
                .is_err()
        );
    }
}
