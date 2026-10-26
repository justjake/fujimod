//! Register-only output block at 0xfff86000.
//!
//! The unit retains a three-word descriptor and accepts control 0/0x300,
//! enable 0/1, and a ten-bit output value. No pin assignment or electrical
//! effect is established.
use crate::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const BASE: u64 = 0xfff86000;

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {}

#[derive(Clone, Serialize, Deserialize)]
struct State {
    registers: [u32; 3],
}

/// One unclassified three-word output unit.
pub struct Block86000 {
    state: State,
}

impl Block86000 {
    pub fn new(_options: Options) -> Self {
        Self {
            state: State { registers: [0; 3] },
        }
    }

    fn offset(address: u64, size: u8) -> Option<usize> {
        if size != 4 || address < BASE {
            return None;
        }
        let offset = address - BASE;
        if offset & 3 != 0 || offset > 8 {
            return None;
        }
        Some(offset as usize)
    }

    fn valid_state(state: &State) -> bool {
        matches!(state.registers[0], 0 | 0x300)
            && matches!(state.registers[1], 0 | 1)
            && state.registers[2] <= 0x3ff
    }
}

impl Default for Block86000 {
    fn default() -> Self {
        Self::new(Options::default())
    }
}

impl Device for Block86000 {
    crate::checkpoint_fields!(state: State);

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr2-block-86000"
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(offset) = Self::offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        if offset != 0 {
            cx.fail("0xfff86000 output block only supports reads of its control word");
            return 0;
        }
        u64::from(self.state.registers[0])
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = Self::offset(address, size) else {
            cx.unmodeled();
            return;
        };
        if value > u64::from(u32::MAX) {
            cx.fail("0xfff86000 output block requires 32-bit register values");
            return;
        }
        let value = value as u32;
        let valid = match offset {
            0 => matches!(value, 0 | 0x300),
            4 => matches!(value, 0 | 1) && (value == 0 || self.state.registers[0] == 0x300),
            8 => value <= 0x3ff,
            _ => false,
        };
        if !valid {
            cx.fail("unsupported 0xfff86000 output register value or enable sequence");
            return;
        }
        self.state.registers[offset / 4] = value;
    }

    fn save(&self) -> Value {
        json!({"state": self.state})
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value["state"].clone())
            .map_err(|error| format!("invalid 0xfff86000 state: {error}"))?;
        if !Self::valid_state(&state) {
            return Err("invalid 0xfff86000 output register state".into());
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Host, Record};

    struct Clock;

    impl Host for Clock {
        fn now(&self) -> u64 {
            0
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            unreachable!()
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

    fn read(model: &mut Block86000, offset: u64, size: u8) -> (u64, Option<String>) {
        let mut host = Clock;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        let value = model.read(&mut cx, BASE + offset, size);
        (value, cx.take_refusal())
    }

    fn write(model: &mut Block86000, offset: u64, size: u8, value: u64) -> Option<String> {
        let mut host = Clock;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        model.write(&mut cx, BASE + offset, size, value);
        cx.take_refusal()
    }

    #[test]
    fn retains_supported_control_enable_and_ten_bit_value() {
        let mut model = Block86000::new(Options::default());
        assert_eq!(read(&mut model, 0, 4), (0, None));
        assert_eq!(write(&mut model, 0, 4, 0x300), None);
        assert_eq!(write(&mut model, 4, 4, 1), None);
        assert_eq!(write(&mut model, 8, 4, 0x3ff), None);
        assert_eq!(model.state.registers, [0x300, 1, 0x3ff]);
        assert_eq!(
            read(&mut model, 4, 4).1.as_deref(),
            Some("0xfff86000 output block only supports reads of its control word")
        );
    }

    #[test]
    fn refuses_unknown_values_and_enable_before_control_setup() {
        let mut model = Block86000::new(Options::default());
        assert!(write(&mut model, 4, 4, 1).is_some());
        assert!(write(&mut model, 0, 4, 0x301).is_some());
        assert!(write(&mut model, 8, 4, 0x400).is_some());
        assert!(write(&mut model, 0, 2, 0).is_some());
        assert!(write(&mut model, 0xc, 4, 0).is_some());
        assert!(write(&mut model, 8, 4, u64::from(u32::MAX) + 1).is_some());
        assert_eq!(model.state.registers, [0; 3]);
    }

    #[test]
    fn checkpoint_preserves_registers_and_rejects_invalid_state() {
        let mut model = Block86000::new(Options::default());
        write(&mut model, 0, 4, 0x300);
        write(&mut model, 8, 4, 0x2aa);
        let mut restored = Block86000::new(Options::default());
        restored.restore(&model.save()).unwrap();
        assert_eq!(restored.state.registers, [0x300, 0, 0x2aa]);

        assert!(
            restored
                .restore(&json!({"state":{"registers":[0,0,0x400]}}))
                .is_err()
        );
    }
}
