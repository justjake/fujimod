//! Word-aligned write-only register storage for explicitly mapped windows.
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    base: u64,
    size: u64,
    registers: BTreeMap<u64, u32>,
}

pub struct WriteOnlyRegisters {
    state: State,
}

impl WriteOnlyRegisters {
    pub fn new(base: u64, size: u64) -> Result<Self, String> {
        if base & 3 != 0 || size == 0 || size & 3 != 0 || base.checked_add(size).is_none() {
            return Err("invalid write-only register window".into());
        }
        Ok(Self {
            state: State {
                base,
                size,
                registers: BTreeMap::new(),
            },
        })
    }

    fn valid_word_offset(&self, offset: u64) -> bool {
        offset < self.state.size && offset & 3 == 0
    }

    fn validate_state(&self, state: &State) -> Result<(), String> {
        if state.base != self.state.base
            || state.size != self.state.size
            || state
                .registers
                .keys()
                .any(|offset| !self.valid_word_offset(*offset))
        {
            return Err("invalid write-only register checkpoint".into());
        }
        Ok(())
    }
}

impl Device for WriteOnlyRegisters {
    fn name(&self) -> &'static str {
        "write-only-registers"
    }

    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        cx.unmodeled();
        0
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = address
            .checked_sub(self.state.base)
            .filter(|offset| self.valid_word_offset(*offset))
        else {
            cx.unmodeled();
            return;
        };
        if size != 4 || value > u64::from(u32::MAX) {
            cx.unmodeled();
            return;
        }
        self.state.registers.insert(offset, value as u32);
    }

    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), &self.state)
    }

    fn validate_checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), String> {
        self.validate_state(&checkpoint.decode(self.name())?)
    }

    fn restore_checkpoint(&mut self, checkpoint: &Checkpoint) -> Result<(), String> {
        let state: State = checkpoint.decode(self.name())?;
        self.validate_state(&state)?;
        self.state = state;
        Ok(())
    }

    fn fingerprint(&self) -> Result<Value, String> {
        Ok(serde_json::to_value(&self.state).unwrap())
    }

    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
        self.validate_state(&state)?;
        self.state = state;
        Ok(())
    }
}
