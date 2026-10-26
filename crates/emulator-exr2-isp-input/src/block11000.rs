//! Register bank at 0xfff11000 for input-adjacent configuration.
//!
//! Table words are retained as opaque configuration. The optional typed
//! clear-bit acknowledgment models only a delayed control 3-to-2 response;
//! it does not represent an image engine, DMA transfer or interrupt.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use fujimod_emulator_imaging::exr2_isp::options::block11000::{Options, ControlMode};

pub const BASE: u64 = 0xfff11000;
const STROBE_VALUE: u32 = 0x000d0000;
const CONFIG_MASKS: &[(u16, u32)] = &[
    (0x04, 0x31bf33ff),
    (0x08, u32::MAX),
    (0x0c, u32::MAX),
    (0x10, u32::MAX),
    (0x14, u32::MAX),
    (0x18, u32::MAX),
    (0x1c, u32::MAX),
    (0x20, u32::MAX),
    (0x24, u32::MAX),
    (0x28, u32::MAX),
    (0x2c, u32::MAX),
    (0x30, u32::MAX),
    (0x34, u32::MAX),
    (0x38, u32::MAX),
    (0x3c, u32::MAX),
    (0x40, u32::MAX),
    (0x44, u32::MAX),
    (0x48, u32::MAX),
    (0x4c, u32::MAX),
    (0x50, u32::MAX),
    (0x54, u32::MAX),
    (0x58, u32::MAX),
    (0x5c, u32::MAX),
    (0x60, u32::MAX),
    (0x64, u32::MAX),
    (0x68, u32::MAX),
    (0x6c, u32::MAX),
    (0x70, u32::MAX),
    (0x74, u32::MAX),
    (0x78, u32::MAX),
    (0x7c, u32::MAX),
    (0x80, u32::MAX),
    (0x84, u32::MAX),
    (0x88, u32::MAX),
    (0x8c, u32::MAX),
    (0x90, u32::MAX),
    (0x94, u32::MAX),
    (0x98, 0x77777777),
    (0x9c, 0x00007777),
    (0xa8, 0x000d000d),
    (0xac, 0x00003fff),
    (0xb0, 0x77777777),
    (0xb4, 0x00777777),
];
const READABLE: &[u16] = &[0x00, 0x98, 0x9c, 0xa0, 0xa4, 0xa8, 0xac];

#[derive(Clone, Debug, Serialize, Deserialize)]
struct State {
    registers: BTreeMap<u16, u32>,
    mode: ControlMode,
    generation: u64,
    deadline: Option<u64>,
}

/// Opaque configuration bank and absent event-status inputs.
pub struct Block11000 {
    state: State,
}

impl Block11000 {
    pub fn new(options: Options) -> Result<Self, String> {
        let mode = options.control_mode;
        if matches!(mode, ControlMode::ClearBit0Ack { delay_ticks: 0 }) {
            return Err("auxiliary control acknowledgment delay must be positive".into());
        }
        Ok(Self {
            state: State {
                registers: BTreeMap::from([(0, 0)]),
                mode,
                generation: 0,
                deadline: None,
            },
        })
    }

    fn offset(address: u64, size: u8) -> Option<u16> {
        if !(BASE..BASE + 0x100).contains(&address) {
            return None;
        }
        let offset = (address - BASE) as u16;
        if size != 4
            || offset & 3 != 0
            || !matches!(offset, 0 | 0xa0 | 0xa4)
                && !CONFIG_MASKS.iter().any(|(known, _)| *known == offset)
        {
            return None;
        }
        Some(offset)
    }

    fn mask(offset: u16) -> Option<u32> {
        CONFIG_MASKS
            .iter()
            .find_map(|(known, mask)| (*known == offset).then_some(*mask))
    }

    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self
            .state
            .deadline
            .is_some_and(|deadline| deadline <= cx.now())
        {
            self.state.deadline = None;
            self.state.registers.insert(0, 3);
        }
    }

    fn valid_state(state: &State) -> bool {
        let valid_mode = !matches!(state.mode, ControlMode::ClearBit0Ack { delay_ticks: 0 });
        let valid_registers = state.registers.iter().all(|(offset, value)| {
            if *offset == 0 {
                matches!(*value, 0 | 1 | 2 | 3 | 0x10 | 0x11 | 0x13)
            } else {
                Self::mask(*offset).is_some_and(|mask| value & !mask == 0)
            }
        });
        valid_mode
            && valid_registers
            && (state.deadline.is_none()
                || matches!(state.mode, ControlMode::ClearBit0Ack { .. })
                    && state.registers.get(&0).copied() == Some(2)
                    && state.generation != 0)
    }
}

impl Default for Block11000 {
    fn default() -> Self {
        Self::new(Options::default()).expect("register-only mode is valid")
    }
}

impl Device for Block11000 {
    fujimod_emulator_devices::checkpoint_fields!(state: State);

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr2-block-11000"
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(offset) = Self::offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        if !READABLE.contains(&offset) {
            cx.fail("no proven readable auxiliary configuration field");
            return 0;
        }
        self.advance(cx);
        if matches!(offset, 0xa0 | 0xa4) {
            return 0;
        }
        u64::from(self.state.registers.get(&offset).copied().unwrap_or(0))
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = Self::offset(address, size) else {
            cx.unmodeled();
            return;
        };
        if value > u64::from(u32::MAX) {
            cx.fail("auxiliary word exceeds 32 bits");
            return;
        }
        self.advance(cx);
        if self.state.deadline.is_some() {
            cx.fail("auxiliary configuration mutation during pending control acknowledgment");
            return;
        }
        let value = value as u32;
        if matches!(offset, 0xa0 | 0xa4) {
            if value != STROBE_VALUE {
                cx.fail(
                    "only the declared auxiliary acknowledgment strobe is supported",
                );
            }
            return;
        }
        let old = self.state.registers.get(&offset).copied().unwrap_or(0);
        if offset == 0 {
            if !matches!(value, 0 | 1 | 2 | 3 | 0x10 | 0x11 | 0x13) {
                cx.fail("unknown auxiliary control command");
                return;
            }
            if old & 1 != 0 && value & 1 == 0 {
                let ControlMode::ClearBit0Ack { delay_ticks } = self.state.mode else {
                    cx.fail("auxiliary clear-bit0/poll-bit0 response is not modeled");
                    return;
                };
                if old != 3 || value != 2 {
                    cx.fail("only the exact auxiliary control 3-to-2 request is modeled");
                    return;
                }
                let Some(deadline) = cx.now().checked_add(delay_ticks) else {
                    cx.fail("auxiliary control acknowledgment deadline overflow");
                    return;
                };
                let Some(generation) = self.state.generation.checked_add(1) else {
                    cx.fail("auxiliary control acknowledgment generation overflow");
                    return;
                };
                self.state.generation = generation;
                self.state.deadline = Some(deadline);
                self.state.registers.insert(0, value);
                cx.schedule(deadline, self.state.generation);
                return;
            }
        } else {
            let mask = Self::mask(offset).expect("known configuration offset has a mask");
            if value & !mask != 0 {
                cx.fail("unknown auxiliary configuration bits");
                return;
            }
            if offset == 0x04 && value & 0x31bf0000 != 0 && value & 0x200 == 0 {
                cx.fail("auxiliary extended fields require the bit-9 selector");
                return;
            }
        }
        self.state.registers.insert(offset, value);
    }

    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.deadline
    }

    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token == self.state.generation {
            self.advance(cx);
        }
    }

    fn save(&self) -> Value {
        serde_json::to_value(&self.state).expect("auxiliary state serializes")
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid auxiliary state: {error}"))?;
        if !Self::valid_state(&state) {
            return Err("invalid auxiliary state".into());
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

    fn read(model: &mut Block11000, host: &mut Clock, offset: u64) -> (u64, Option<String>) {
        let mut cx = Cx::new(host, 0, 0, 3);
        let value = model.read(&mut cx, BASE + offset, 4);
        (value, cx.take_refusal())
    }

    fn write(model: &mut Block11000, host: &mut Clock, offset: u64, value: u64) -> Option<String> {
        let mut cx = Cx::new(host, 0, 0, 3);
        model.write(&mut cx, BASE + offset, 4, value);
        cx.take_refusal()
    }

    #[test]
    fn retains_masked_tables_and_selector_rules() {
        let mut model = Block11000::default();
        let mut host = Clock::default();
        assert_eq!(write(&mut model, &mut host, 0x08, u32::MAX.into()), None);
        assert_eq!(write(&mut model, &mut host, 0x98, 0x77777777), None);
        assert_eq!(read(&mut model, &mut host, 0x98), (0x77777777, None));
        assert_eq!(
            read(&mut model, &mut host, 0x08).1.as_deref(),
            Some("no proven readable auxiliary configuration field")
        );
        assert!(write(&mut model, &mut host, 0x9c, 0x8000).is_some());
        assert!(write(&mut model, &mut host, 0x04, 0x00010000).is_some());
        assert_eq!(write(&mut model, &mut host, 0x04, 0x00010200), None);
    }

    #[test]
    fn absent_status_and_exact_acknowledgment_strobes_are_register_only() {
        let mut model = Block11000::default();
        let mut host = Clock::default();
        assert_eq!(read(&mut model, &mut host, 0xa0), (0, None));
        assert_eq!(read(&mut model, &mut host, 0xa4), (0, None));
        assert_eq!(write(&mut model, &mut host, 0xa0, 0xd0000), None);
        assert_eq!(write(&mut model, &mut host, 0xa4, 0xd0000), None);
        assert!(write(&mut model, &mut host, 0xa0, 0).is_some());
        assert_eq!(read(&mut model, &mut host, 0xa0), (0, None));
    }

    #[test]
    fn typed_ack_mode_schedules_only_the_exact_control_transition() {
        let mut model = Block11000::new(Options {
            control_mode: ControlMode::ClearBit0Ack { delay_ticks: 3 },
        })
        .unwrap();
        let mut host = Clock {
            now: 5,
            ..Clock::default()
        };
        assert_eq!(write(&mut model, &mut host, 0, 3), None);
        assert_eq!(write(&mut model, &mut host, 0, 2), None);
        assert_eq!(model.next_deadline(host.now), Some(8));
        assert_eq!(host.deadlines, [(3, 8, 1)]);
        assert_eq!(read(&mut model, &mut host, 0), (2, None));
        assert!(write(&mut model, &mut host, 0x08, 1).is_some());

        host.now = 8;
        assert_eq!(read(&mut model, &mut host, 0), (3, None));
        assert_eq!(model.next_deadline(host.now), None);
    }

    #[test]
    fn register_only_mode_refuses_control_clear_and_checkpoint_validates() {
        let mut model = Block11000::default();
        let mut host = Clock::default();
        assert_eq!(write(&mut model, &mut host, 0, 3), None);
        assert!(write(&mut model, &mut host, 0, 2).is_some());

        let mut restored = Block11000::default();
        restored.restore(&model.save()).unwrap();
        assert_eq!(read(&mut restored, &mut host, 0), (3, None));
        assert!(restored
            .restore(&serde_json::json!({"registers":{"0":2048}, "mode":"RegisterOnly", "generation":0, "deadline":null}))
            .is_err());
    }
}
