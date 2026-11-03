//! Register-only EXR Processor Pro block at 0xFFFA8000. Bit 24 starts and bit 25 stops a channel; bit 26 reads as ready after either. The block is unidentified; the completion interrupt the firmware also waits for is not raised.
use crate::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const BASE: u64 = 0xfffa8000;

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {}

#[derive(Serialize, Deserialize)]
struct State {
    control: [u32; 2],
    ready: [bool; 2],
    words: [u32; 2],
}

/// One unclassified start/stop/ready register block.
pub struct Blocka8000 {
    state: State,
}

impl Blocka8000 {
    pub fn new(_options: Options) -> Self {
        Self {
            state: State {
                control: [0; 2],
                ready: [false; 2],
                words: [0; 2],
            },
        }
    }

    fn offset(address: u64, size: u8) -> Option<u64> {
        if size != 4 || address < BASE {
            return None;
        }
        let offset = address - BASE;
        matches!(offset, 0 | 4 | 0x40 | 0x44).then_some(offset)
    }
}

impl Device for Blocka8000 {
    crate::checkpoint_fields!(state: State);

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr-pro-block-a8000"
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(offset) = Self::offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        match offset {
            0 | 4 => {
                let channel = (offset / 4) as usize;
                u64::from(
                    self.state.control[channel] & !(1u32 << 26)
                        | u32::from(self.state.ready[channel]) << 26,
                )
            }
            _ => u64::from(self.state.words[(offset / 4 - 0x10) as usize]),
        }
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = Self::offset(address, size) else {
            cx.unmodeled();
            return;
        };
        let Ok(value) = u32::try_from(value) else {
            cx.fail("0xfffa8000 block requires 32-bit register values");
            return;
        };
        match offset {
            0 | 4 => {
                let channel = (offset / 4) as usize;
                self.state.control[channel] = value;
                self.state.ready[channel] = value & (1 << 24 | 1 << 25) != 0;
            }
            _ => self.state.words[(offset / 4 - 0x10) as usize] = value,
        }
    }

    fn save(&self) -> Value {
        json!({"state": self.state})
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value["state"].clone())
            .map_err(|error| format!("invalid 0xfffa8000 state: {error}"))?;
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

    fn read(model: &mut Blocka8000, offset: u64, size: u8) -> (u64, Option<String>) {
        let mut host = Clock;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        let value = model.read(&mut cx, BASE + offset, size);
        (value, cx.take_refusal())
    }

    fn write(model: &mut Blocka8000, offset: u64, size: u8, value: u64) -> Option<String> {
        let mut host = Clock;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        model.write(&mut cx, BASE + offset, size, value);
        cx.take_refusal()
    }

    #[test]
    fn start_bit_sets_ready_within_the_control_word() {
        let mut model = Blocka8000::new(Options::default());
        assert_eq!(read(&mut model, 0, 4), (0, None));
        assert_eq!(write(&mut model, 0, 4, 0x0100_0000), None);
        let (value, refusal) = read(&mut model, 0, 4);
        assert_eq!(refusal, None);
        assert_eq!(value, 0x0500_0000);
        assert_eq!(value & 1 << 26, 1 << 26);
    }

    #[test]
    fn ready_follows_start_or_stop_and_not_plain_configuration() {
        let mut model = Blocka8000::new(Options::default());
        assert_eq!(write(&mut model, 4, 4, 0x4000_0ff0), None);
        assert_eq!(read(&mut model, 4, 4).0 & 1 << 26, 0);
        assert_eq!(write(&mut model, 4, 4, 0x4000_0ff0 | 0x0200_0000), None);
        assert_eq!(read(&mut model, 4, 4).0 & 1 << 26, 1 << 26);
        assert_eq!(read(&mut model, 4, 4).0 & !(1 << 26), 0x4200_0ff0);
    }

    #[test]
    fn data_words_round_trip() {
        let mut model = Blocka8000::new(Options::default());
        assert_eq!(write(&mut model, 0x40, 4, 0x1234_5678), None);
        assert_eq!(write(&mut model, 0x44, 4, 0x9abc_def0), None);
        assert_eq!(read(&mut model, 0x40, 4), (0x1234_5678, None));
        assert_eq!(read(&mut model, 0x44, 4), (0x9abc_def0, None));
    }

    #[test]
    fn unknown_or_misaligned_access_is_unmodeled() {
        let mut model = Blocka8000::new(Options::default());
        assert_eq!(read(&mut model, 8, 4).0, 0);
        assert_eq!(read(&mut model, 2, 2).0, 0);
        assert_eq!(read(&mut model, 0, 2).0, 0);
        assert!(write(&mut model, 8, 4, 1).is_some());
        assert_eq!(model.state.control, [0, 0]);
        assert_eq!(model.state.words, [0, 0]);
        assert!(write(&mut model, 0, 4, u64::from(u32::MAX) + 1).is_some());
    }

    #[test]
    fn checkpoint_round_trip_preserves_ready_bits() {
        let mut model = Blocka8000::new(Options::default());
        write(&mut model, 0, 4, 0x0100_0000);
        write(&mut model, 0x40, 4, 0x2aa);
        let mut restored = Blocka8000::new(Options::default());
        restored.restore(&model.save()).unwrap();
        assert_eq!(restored.state.ready, [true, false]);
        assert_eq!(restored.state.control, [0x0100_0000, 0]);
        assert_eq!(restored.state.words, [0x2aa, 0]);
        assert_eq!(read(&mut restored, 0, 4).0 & 1 << 26, 1 << 26);
        assert!(
            restored
                .restore(&json!({"state":{"control":"bad"}}))
                .is_err()
        );
    }
}
