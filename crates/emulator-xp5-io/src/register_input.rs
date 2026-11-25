//! A fixed read-only 32-bit register input.
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    base: u64,
    value: u32,
}

pub struct RegisterInput {
    state: State,
}

impl RegisterInput {
    pub fn new(base: u64, size: u64, value: u32) -> Result<Self, String> {
        if base & 3 != 0 || size != 4 {
            return Err("invalid register-input window".into());
        }
        Ok(Self {
            state: State { base, value },
        })
    }

    fn valid_read(&self, address: u64, size: u8) -> bool {
        address == self.state.base && address & 3 == 0 && size == 4
    }

    fn validate_state(&self, state: &State) -> Result<(), String> {
        if state.base == self.state.base && state.value == self.state.value {
            Ok(())
        } else {
            Err("register-input state differs".into())
        }
    }
}

impl Device for RegisterInput {
    fn name(&self) -> &'static str {
        "register-input"
    }

    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), &self.state)
    }

    fn validate_checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), String> {
        let state: State = checkpoint.decode(self.name())?;
        self.validate_state(&state)
    }

    fn restore_checkpoint(&mut self, checkpoint: &Checkpoint) -> Result<(), String> {
        let state: State = checkpoint.decode(self.name())?;
        self.validate_state(&state)?;
        self.state = state;
        Ok(())
    }

    fn fingerprint(&self) -> Result<Value, String> {
        Ok(json!({"base": self.state.base, "value": self.state.value}))
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if !self.valid_read(address, size) {
            cx.unmodeled();
            return 0;
        }
        u64::from(self.state.value)
    }

    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
        cx.unmodeled();
    }

    fn pollable(&self, address: u64, size: u8) -> bool {
        self.valid_read(address, size)
    }

    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid register-input state: {error}"))?;
        self.validate_state(&state)?;
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;

    #[test]
    fn returns_fixed_word_and_refuses_other_accesses_and_state() {
        let base = 0x1234;
        let mut device = RegisterInput::new(base, 4, 0x89ab_cdef).unwrap();
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        assert_eq!(device.read(&mut cx, base, 4), 0x89ab_cdef);
        assert_eq!(cx.refusal(), None);
        assert_eq!(device.read(&mut cx, base, 2), 0);
        assert!(cx.refusal().is_some());
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        assert_eq!(device.read(&mut cx, base + 4, 4), 0);
        assert!(cx.refusal().is_some());
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        device.write(&mut cx, base, 4, 1);
        assert!(cx.refusal().is_some());
        let mut other = RegisterInput::new(base, 4, 1).unwrap();
        assert!(other.restore(&device.save()).is_err());
        assert_eq!(other.fingerprint().unwrap(), json!({"base":base,"value":1}));
    }
}
