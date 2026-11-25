//! Write-only register storage for explicitly mapped windows.
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    base: u64,
    size: u64,
    /// Register width in bytes. The word default is left out of the encoding,
    /// so a default window's state keeps the word-register form.
    #[serde(default = "word_width", skip_serializing_if = "is_word_width")]
    width: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    allowed_values: Option<Vec<u32>>,
    registers: BTreeMap<u64, u32>,
}

fn word_width() -> u8 {
    4
}

fn is_word_width(width: &u8) -> bool {
    *width == 4
}

pub struct WriteOnlyRegisters {
    state: State,
    /// Emulator-only option (`readback = true`): a read returns the last write, zero before one,
    /// instead of refusing. Labelled assumption for blocks whose registers are unidentified.
    readback: bool,
}

impl WriteOnlyRegisters {
    pub fn new(
        base: u64,
        size: u64,
        width: u8,
        allowed_values: Option<Vec<u32>>,
    ) -> Result<Self, String> {
        if !matches!(width, 2 | 4)
            || !base.is_multiple_of(u64::from(width))
            || size == 0
            || !size.is_multiple_of(u64::from(width))
            || base.checked_add(size).is_none()
            || !Self::allowed_values_valid(width, allowed_values.as_deref())
        {
            return Err("invalid write-only register window".into());
        }
        Ok(Self {
            state: State {
                base,
                size,
                width,
                allowed_values,
                registers: BTreeMap::new(),
            },
            readback: false,
        })
    }

    pub fn with_readback(mut self) -> Self {
        self.readback = true;
        self
    }

    fn allowed_values_valid(width: u8, allowed_values: Option<&[u32]>) -> bool {
        let Some(values) = allowed_values else {
            return true;
        };
        let max_value = u32::MAX >> (8 * (4 - width));
        !values.is_empty()
            && values
                .iter()
                .enumerate()
                .all(|(index, value)| *value <= max_value && !values[..index].contains(value))
    }

    fn valid_register_offset(&self, offset: u64) -> bool {
        offset < self.state.size && offset.is_multiple_of(u64::from(self.state.width))
    }

    fn max_value(&self) -> u64 {
        u64::from(u32::MAX >> (8 * (4 - self.state.width)))
    }

    fn validate_state(&self, state: &State) -> Result<(), String> {
        if state.base != self.state.base
            || state.size != self.state.size
            || state.width != self.state.width
            || state.allowed_values != self.state.allowed_values
            || !Self::allowed_values_valid(state.width, state.allowed_values.as_deref())
            || state
                .registers
                .keys()
                .any(|offset| !self.valid_register_offset(*offset))
            || state
                .registers
                .values()
                .any(|value| u64::from(*value) > self.max_value())
            || state.allowed_values.as_ref().is_some_and(|allowed| {
                state
                    .registers
                    .values()
                    .any(|value| !allowed.contains(value))
            })
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

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if self.readback
            && size == self.state.width
            && let Some(offset) = address
                .checked_sub(self.state.base)
                .filter(|offset| self.valid_register_offset(*offset))
        {
            return u64::from(self.state.registers.get(&offset).copied().unwrap_or(0));
        }
        cx.unmodeled();
        0
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = address
            .checked_sub(self.state.base)
            .filter(|offset| self.valid_register_offset(*offset))
        else {
            cx.unmodeled();
            return;
        };
        if size != self.state.width
            || value > self.max_value()
            || self
                .state
                .allowed_values
                .as_ref()
                .is_some_and(|allowed| !allowed.contains(&(value as u32)))
        {
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
        let state: State =
            serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
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
    fn selected_halfword_width_preserves_write_only_state_and_word_default() {
        let mut host = RecordingHost::default();
        let mut halfwords = WriteOnlyRegisters::new(0x1000, 0x20, 2, None).unwrap();
        halfwords.write(&mut Cx::new(&mut host, 0, 0, 0), 0x1002, 2, 0xabcd);
        let before_refusal = halfwords.save();

        let mut cx = Cx::new(&mut host, 0, 0, 0);
        halfwords.write(&mut cx, 0x1001, 2, 1);
        assert!(cx.is_unmodeled());
        assert_eq!(halfwords.save(), before_refusal);

        let mut cx = Cx::new(&mut host, 0, 0, 0);
        halfwords.write(&mut cx, 0x1004, 4, 1);
        assert!(cx.is_unmodeled());
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        halfwords.write(&mut cx, 0x1004, 2, 0x1_0000);
        assert!(cx.is_unmodeled());

        let mut cx = Cx::new(&mut host, 0, 0, 0);
        assert_eq!(halfwords.read(&mut cx, 0x1002, 2), 0);
        assert!(cx.is_unmodeled());

        let checkpoint = halfwords.checkpoint().unwrap();
        let mut restored = WriteOnlyRegisters::new(0x1000, 0x20, 2, None).unwrap();
        restored.validate_checkpoint(&checkpoint).unwrap();
        restored.restore_checkpoint(&checkpoint).unwrap();
        assert_eq!(restored.save(), before_refusal);
        let mut invalid = before_refusal.clone();
        invalid["registers"]["2"] = Value::from(0x1_0000);
        assert!(restored.restore(&invalid).is_err());
        assert_eq!(restored.save(), before_refusal);

        let mut words = WriteOnlyRegisters::new(0x2000, 0x20, 4, None).unwrap();
        words.write(&mut Cx::new(&mut host, 0, 0, 0), 0x2004, 4, 0x1234_5678);
        assert_eq!(words.save()["registers"]["4"], 0x1234_5678);
        assert_eq!(
            words.save(),
            serde_json::json!({"base": 0x2000, "size": 0x20, "registers": {"4": 0x1234_5678}})
        );
        assert!(WriteOnlyRegisters::new(0x1001, 0x20, 2, None).is_err());
        assert!(WriteOnlyRegisters::new(0x1000, 0x20, 1, None).is_err());
    }

    #[test]
    fn exact_allowed_values_refuse_other_writes_and_bind_checkpoint_policy() {
        let mut host = RecordingHost::default();
        let mut device = WriteOnlyRegisters::new(0x3000, 4, 4, Some(vec![1])).unwrap();

        let mut cx = Cx::new(&mut host, 0, 0, 0);
        device.write(&mut cx, 0x3000, 4, 1);
        assert!(cx.refusal().is_none());
        let saved = device.save();

        let mut cx = Cx::new(&mut host, 0, 0, 0);
        device.write(&mut cx, 0x3000, 4, 2);
        assert!(cx.is_unmodeled());
        assert_eq!(device.save(), saved);

        let mut cx = Cx::new(&mut host, 0, 0, 0);
        device.read(&mut cx, 0x3000, 4);
        assert!(cx.is_unmodeled());
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        device.write(&mut cx, 0x3000, 2, 1);
        assert!(cx.is_unmodeled());

        let checkpoint = device.checkpoint().unwrap();
        let mut restored = WriteOnlyRegisters::new(0x3000, 4, 4, Some(vec![1])).unwrap();
        restored.validate_checkpoint(&checkpoint).unwrap();
        restored.restore_checkpoint(&checkpoint).unwrap();
        assert_eq!(restored.save(), saved);

        let different_policy = WriteOnlyRegisters::new(0x3000, 4, 4, None).unwrap();
        assert!(different_policy.validate_checkpoint(&checkpoint).is_err());
        let mut invalid_state = saved.clone();
        invalid_state["registers"]["0"] = serde_json::json!(2);
        assert!(restored.restore(&invalid_state).is_err());
        assert_eq!(restored.save(), saved);
        assert!(WriteOnlyRegisters::new(0x3000, 4, 4, Some(Vec::new())).is_err());
        assert!(WriteOnlyRegisters::new(0x3000, 4, 4, Some(vec![1, 1])).is_err());
    }

    #[test]
    fn readback_option_returns_the_last_write_and_default_still_refuses() {
        let mut host = RecordingHost::default();
        let mut plain = WriteOnlyRegisters::new(0xff960000, 0x10000, 4, None).unwrap();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        plain.write(&mut cx, 0xff96cb30, 4, 7);
        assert!(cx.refusal().is_none() && !cx.is_unmodeled());
        plain.read(&mut cx, 0xff96cb30, 4);
        assert!(cx.is_unmodeled());
        let mut kept = WriteOnlyRegisters::new(0xff960000, 0x10000, 4, None)
            .unwrap()
            .with_readback();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        assert_eq!(kept.read(&mut cx, 0xff96cb30, 4), 0);
        kept.write(&mut cx, 0xff96cb30, 4, 7);
        assert_eq!(kept.read(&mut cx, 0xff96cb30, 4), 7);
        assert!(!cx.is_unmodeled());
    }
}
