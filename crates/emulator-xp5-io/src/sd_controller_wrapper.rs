//! Retained 32-bit register window for an SD controller wrapper.
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    base: u64,
    initial_word: u32,
    value: u32,
    writable_mask: u32,
    write_width: u8,
    readable: bool,
}

pub struct SdControllerWrapper {
    state: State,
}

impl SdControllerWrapper {
    pub fn new(
        base: u64,
        size: u64,
        initial_word: u32,
        writable_mask: u32,
        write_width: u8,
        readable: bool,
    ) -> Result<Self, String> {
        if base & 3 != 0
            || size != 4
            || !matches!(write_width, 2 | 4)
            || write_width == 2 && writable_mask & !u32::from(u16::MAX) != 0
        {
            return Err("invalid SD controller wrapper window".into());
        }
        Ok(Self {
            state: State {
                base,
                initial_word,
                value: initial_word,
                writable_mask,
                write_width,
                readable,
            },
        })
    }

    fn valid_read(&self, address: u64, size: u8) -> bool {
        self.state.readable && address == self.state.base && address & 3 == 0 && size == 4
    }

    fn valid_write(&self, address: u64, size: u8) -> bool {
        address == self.state.base && address & 3 == 0 && size == self.state.write_width
    }

    fn validate_state(&self, state: &State) -> Result<(), String> {
        if state.base == self.state.base
            && state.initial_word == self.state.initial_word
            && state.writable_mask == self.state.writable_mask
            && state.write_width == self.state.write_width
            && state.readable == self.state.readable
            && (state.value ^ state.initial_word) & !state.writable_mask == 0
        {
            Ok(())
        } else {
            Err("SD controller wrapper checkpoint configuration differs".into())
        }
    }
}

impl Device for SdControllerWrapper {
    fn name(&self) -> &'static str {
        "sd-controller-wrapper"
    }

    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), &self.state)
    }

    fn validate_checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), String> {
        self.validate_state(&checkpoint.decode(self.name())?)
    }

    fn restore_checkpoint(&mut self, checkpoint: &Checkpoint) -> Result<(), String> {
        let state = checkpoint.decode(self.name())?;
        self.validate_state(&state)?;
        self.state = state;
        Ok(())
    }

    fn fingerprint(&self) -> Result<Value, String> {
        Ok(
            json!({"base": self.state.base, "initial_word": self.state.initial_word, "value": self.state.value, "writable_mask": self.state.writable_mask, "write_width": self.state.write_width, "readable": self.state.readable}),
        )
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if !self.valid_read(address, size) {
            cx.unmodeled();
            return 0;
        }
        u64::from(self.state.value)
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let max_value = if size == 2 {
            u64::from(u16::MAX)
        } else {
            u64::from(u32::MAX)
        };
        if !self.valid_write(address, size) || value > max_value {
            cx.unmodeled();
            return;
        }
        let value = value as u32;
        let writable_mask = if size == 2 {
            self.state.writable_mask & u32::from(u16::MAX)
        } else {
            self.state.writable_mask
        };
        let current_value = if size == 2 {
            self.state.value & u32::from(u16::MAX)
        } else {
            self.state.value
        };
        if (value ^ current_value) & !writable_mask != 0 {
            cx.fail("SD controller wrapper write changes protected bits");
            return;
        }
        self.state.value = (self.state.value & !writable_mask) | (value & writable_mask);
    }

    fn pollable(&self, address: u64, size: u8) -> bool {
        self.valid_read(address, size)
    }

    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid SD controller wrapper state: {error}"))?;
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
    fn writable_bits_are_retained_checkpointed_and_protected_bits_refuse_changes() {
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        let mut device = SdControllerWrapper::new(0xff9b2080, 4, 0, 0xf00, 4, true).unwrap();

        assert_eq!(device.read(&mut cx, 0xff9b2080, 4), 0);
        device.write(&mut cx, 0xff9b2080, 4, 0xf00);
        assert_eq!(device.read(&mut cx, 0xff9b2080, 4), 0xf00);
        let checkpoint = device.checkpoint().unwrap();

        device.write(&mut cx, 0xff9b2080, 4, 0);
        device.restore_checkpoint(&checkpoint).unwrap();
        assert_eq!(device.read(&mut cx, 0xff9b2080, 4), 0xf00);
        assert_eq!(cx.refusal(), None);

        device.write(&mut cx, 0xff9b2080, 4, 0x1f00);
        assert!(cx.refusal().is_some());
        assert_eq!(device.read(&mut cx, 0xff9b2080, 4), 0xf00);

        let mut width_host = RecordingHost::default();
        let mut width_cx = Cx::new(&mut width_host, 0, 0, 0);
        assert_eq!(device.read(&mut width_cx, 0xff9b2080, 2), 0);
        assert!(width_cx.refusal().is_some());
    }

    #[test]
    fn halfword_writes_preserve_upper_bits_and_checkpoint_write_configuration() {
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        let mut device =
            SdControllerWrapper::new(0xff9b2078, 4, 0xabcd0000, 0xffff, 2, true).unwrap();

        assert_eq!(device.read(&mut cx, 0xff9b2078, 4), 0xabcd0000);
        device.write(&mut cx, 0xff9b2078, 2, 1);
        assert_eq!(device.read(&mut cx, 0xff9b2078, 4), 0xabcd0001);
        let checkpoint = device.checkpoint().unwrap();

        device.write(&mut cx, 0xff9b2078, 2, 2);
        device.restore_checkpoint(&checkpoint).unwrap();
        assert_eq!(device.read(&mut cx, 0xff9b2078, 4), 0xabcd0001);

        let fullword = SdControllerWrapper::new(0xff9b2078, 4, 0, 0xffff, 4, true).unwrap();
        assert!(fullword.validate_checkpoint(&checkpoint).is_err());
        assert!(SdControllerWrapper::new(0xff9b2078, 4, 0, 0x1_0000, 2, true).is_err());
        let mut invalid = device.save();
        invalid["value"] = json!(0xabce0001_u32);
        assert!(device.restore(&invalid).is_err());
        assert_eq!(device.save()["value"], json!(0xabcd0001_u32));
        assert_eq!(cx.refusal(), None);

        let mut bad_host = RecordingHost::default();
        let mut bad_cx = Cx::new(&mut bad_host, 0, 0, 0);
        let mut protected =
            SdControllerWrapper::new(0xff9b2078, 4, 0xabcd0000, 1, 2, true).unwrap();
        protected.write(&mut bad_cx, 0xff9b2078, 2, 2);
        assert!(bad_cx.refusal().is_some());
        assert_eq!(protected.read(&mut bad_cx, 0xff9b2078, 4), 0xabcd0000);
    }

    #[test]
    fn write_only_word_retains_writes_and_refuses_reads_and_protected_changes() {
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        let mut device = SdControllerWrapper::new(0xff9b20ec, 4, 0, 1, 4, false).unwrap();

        assert!(!device.pollable(0xff9b20ec, 4));
        device.write(&mut cx, 0xff9b20ec, 4, 1);
        assert_eq!(cx.refusal(), None);
        let checkpoint = device.checkpoint().unwrap();

        let mut read_host = RecordingHost::default();
        let mut read_cx = Cx::new(&mut read_host, 0, 0, 0);
        assert_eq!(device.read(&mut read_cx, 0xff9b20ec, 4), 0);
        assert!(read_cx.refusal().is_some());

        let mut protected_host = RecordingHost::default();
        let mut protected_cx = Cx::new(&mut protected_host, 0, 0, 0);
        device.write(&mut protected_cx, 0xff9b20ec, 4, 2);
        assert!(protected_cx.refusal().is_some());

        let readable = SdControllerWrapper::new(0xff9b20ec, 4, 0, 1, 4, true).unwrap();
        assert!(readable.validate_checkpoint(&checkpoint).is_err());
        device.restore_checkpoint(&checkpoint).unwrap();
        assert_eq!(device.state.value, 1);
    }
}
