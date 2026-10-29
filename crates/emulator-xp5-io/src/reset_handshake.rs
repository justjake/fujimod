//! Immediate reset completion for one explicitly configured handshake word.
//! Reset completes immediately here as a session assumption, not a hardware observation.
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    base: u64,
    trigger_offset: u64,
}

pub struct ResetHandshake {
    state: State,
}

impl ResetHandshake {
    pub fn new(base: u64, trigger_offset: u64) -> Result<Self, String> {
        if trigger_offset & 3 != 0 || base.checked_add(trigger_offset).is_none() {
            return Err("invalid reset-handshake trigger offset".into());
        }
        Ok(Self {
            state: State {
                base,
                trigger_offset,
            },
        })
    }

    fn at_trigger(&self, address: u64, size: u8) -> bool {
        size == 4 && address.checked_sub(self.state.base) == Some(self.state.trigger_offset)
    }

    fn validate_state(&self, state: State) -> Result<(), String> {
        if state == self.state {
            Ok(())
        } else {
            Err("reset-handshake checkpoint configuration differs".into())
        }
    }
}

impl Device for ResetHandshake {
    fn name(&self) -> &'static str {
        "reset-handshake"
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if self.at_trigger(address, size) {
            0
        } else {
            cx.unmodeled();
            0
        }
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if !self.at_trigger(address, size) || value != 1 {
            cx.unmodeled();
        }
    }

    fn pollable(&self, address: u64, size: u8) -> bool {
        self.at_trigger(address, size)
    }

    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), &self.state)
    }

    fn validate_checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), String> {
        self.validate_state(checkpoint.decode(self.name())?)
    }

    fn restore_checkpoint(&mut self, checkpoint: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(checkpoint)
    }

    fn fingerprint(&self) -> Result<Value, String> {
        Ok(json!({
            "base": self.state.base,
            "trigger_offset": self.state.trigger_offset,
        }))
    }

    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }

    fn restore(&mut self, state: &Value) -> Result<(), String> {
        self.validate_state(
            serde_json::from_value(state.clone())
                .map_err(|error| format!("invalid reset-handshake state: {error}"))?,
        )
    }
}
