//! EXR II serial-audio control banks and their explicit external DMA sinks.
//!
//! The X70 control and transfer fields are retained as guest-written words.
//! DMA endpoints consume opaque bytes only when explicitly connected; this
//! model does not create samples, transfer deadlines, or completion IRQs.
pub mod construct;
pub mod duplex;
pub mod receive;
pub mod transmit;
use fujimod_emulator_parts_io::audio_sink::AudioSink;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const BASES: [u64; 2] = [0xfffa0000, 0xfffa1000];
const CONTROL: u16 = 0x04;
const FORMAT: u16 = 0x08;
const CHANNEL_ENABLE: u16 = 0x0c;
const IRQ_ENABLE: u16 = 0x40;
const STATUS: u16 = 0x50;
const SAMPLE_WIDTH: u16 = 0x100;
const FIFO_ENDPOINT: u64 = 0x10;

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct State {
    banks: [BTreeMap<u16, u32>; 2],
}

/// Two serial-audio register banks with separately attached byte sinks.
#[derive(Clone)]
pub struct SerialAudio {
    state: State,
    sinks: [AudioSink; 2],
}

impl SerialAudio {
    pub fn new(_options: Options, sinks: [AudioSink; 2]) -> Self {
        Self {
            state: State::default(),
            sinks,
        }
    }

    fn location(address: u64, size: u8) -> Option<(usize, u16)> {
        for (bank, base) in BASES.iter().enumerate() {
            if (*base..*base + 0x1000).contains(&address) {
                let offset = (address - base) as u16;
                return (size == 4 && offset & 3 == 0 && Self::mask(offset).is_some())
                    .then_some((bank, offset));
            }
        }
        None
    }

    fn mask(offset: u16) -> Option<u32> {
        Some(match offset {
            0x00 | CHANNEL_ENABLE => 0x0001_0001,
            CONTROL => 0x0103_0001,
            FORMAT => 0x0003_8003,
            0x10 | 0x20 => u32::MAX,
            IRQ_ENABLE => 1,
            STATUS => 0,
            SAMPLE_WIDTH => 3,
            0x14 | 0x18 | 0x1c | 0x24 | 0x28 | 0x2c | 0x30 | 0x34 | 0x38 => u32::MAX,
            _ => return None,
        })
    }

    fn is_transfer_offset(offset: u16) -> bool {
        matches!(
            offset,
            0x00 | 0x08 | 0x0c | 0x10 | 0x20 | 0x40 | 0x50 | 0x100
        )
    }

    fn valid_state(state: &State) -> bool {
        state.banks.iter().all(|registers| {
            registers.iter().all(|(offset, value)| {
                Self::mask(*offset).is_some_and(|mask| {
                    value & !mask == 0
                        && !(*offset == FORMAT && value & 3 == 3)
                        && !(*offset == CONTROL && value & 0x0003_0000 == 0x0003_0000)
                        && *offset != STATUS
                })
            })
        })
    }

    fn read_word(&self, bank: usize, offset: u16) -> u32 {
        self.state.banks[bank].get(&offset).copied().unwrap_or(0)
    }

    /// Whether the selected audio bank accepts a DMA transfer at this endpoint.
    pub fn can_transmit(&self, destination: u64) -> Result<bool, String> {
        let Some((bank, _base)) = BASES
            .iter()
            .enumerate()
            .find(|(_, base)| destination == **base + FIFO_ENDPOINT)
        else {
            return Err(format!(
                "unsupported serial-audio DMA endpoint {destination:#x}"
            ));
        };
        Ok(self.read_word(bank, FORMAT) & 0x0002_0000 != 0)
    }

    /// Deliver bytes from a DMA engine through the declared audio hardware endpoint.
    pub fn transmit_dma(&mut self, destination: u64, bytes: &[u8]) -> Result<(), String> {
        let Some((bank, _)) = BASES
            .iter()
            .enumerate()
            .find(|(_, base)| destination == **base + FIFO_ENDPOINT)
        else {
            return Err(format!(
                "unsupported serial-audio DMA endpoint {destination:#x}"
            ));
        };
        if !self.can_transmit(destination)? {
            return Err("serial-audio DMA reached a disabled transmitter".into());
        }
        self.sinks[bank].receive_dma(bytes)
    }

    /// An explicitly clocked stereo input supplies 16-bit frames when RX is enabled.
    pub fn can_receive(&self, source: u64, sample_rate: u32) -> Result<bool, String> {
        let bank = BASES
            .iter()
            .position(|base| source == base + 0x20)
            .ok_or("unsupported serial-audio receive FIFO")?;
        if self.read_word(bank, FORMAT) & 0x10000 == 0 {
            return Ok(false);
        }
        if self.read_word(bank, SAMPLE_WIDTH) != 0
            || self.read_word(bank, FORMAT) & 3 != 2
            || sample_rate != 48000
        {
            return Err("serial-audio input requires declared 48 kHz stereo 16-bit packing".into());
        }
        Ok(true)
    }

    pub fn sink_bytes_received(&self, bank: usize) -> Option<u64> {
        self.sinks.get(bank).map(AudioSink::bytes_received)
    }
}

impl Default for SerialAudio {
    fn default() -> Self {
        Self::new(
            Options::default(),
            [AudioSink::default(), AudioSink::default()],
        )
    }
}

impl Device for SerialAudio {
    fujimod_emulator_devices::checkpoint_fields!(state: State, sinks: [AudioSink;2]);
    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr2-serial-audio"
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some((bank, offset)) = Self::location(address, size) else {
            cx.unmodeled();
            return 0;
        };
        u64::from(self.read_word(bank, offset))
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some((bank, offset)) = Self::location(address, size) else {
            cx.unmodeled();
            return;
        };
        if value > u64::from(u32::MAX) {
            cx.fail("serial-audio register value exceeds 32 bits");
            return;
        }
        let value = value as u32;
        let Some(mask) = Self::mask(offset) else {
            cx.unmodeled();
            return;
        };
        if value & !mask != 0 {
            cx.fail(format!(
                "unmodeled serial-audio control bits {address:#x}={value:#x}"
            ));
            return;
        }
        if offset == STATUS {
            cx.fail("serial-audio FIFO status is read-only");
            return;
        }
        if Self::is_transfer_offset(offset) && cx.cpu != 0 {
            cx.fail("audio transfer registers accept CPU0 accesses only");
            return;
        }
        if offset == CONTROL && value & 0x0003_0000 == 0x0003_0000 {
            cx.fail("unobserved serial-audio mode field 3");
            return;
        }
        if offset == FORMAT && value & 3 == 3 {
            cx.fail("serial-audio format field 3 is unsupported");
            return;
        }
        if offset == 0 {
            self.state.banks[bank].insert(offset, 0);
        } else {
            self.state.banks[bank].insert(offset, value);
        }
    }

    fn save(&self) -> Value {
        json!({
            "state": self.state,
            "sinks": [self.sinks[0].save(), self.sinks[1].save()],
        })
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value["state"].clone())
            .map_err(|error| format!("invalid serial-audio checkpoint: {error}"))?;
        if !Self::valid_state(&state) {
            return Err("invalid serial-audio register state".into());
        }
        let sinks = value["sinks"]
            .as_array()
            .ok_or("serial-audio checkpoint has no sink states")?;
        if sinks.len() != 2 {
            return Err("serial-audio checkpoint requires two sink states".into());
        }
        let mut restored_sinks = self.sinks.clone();
        restored_sinks[0].restore(&sinks[0])?;
        restored_sinks[1].restore(&sinks[1])?;
        self.state = state;
        self.sinks = restored_sinks;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{Host, Record};

    #[derive(Default)]
    struct Clock;

    impl Host for Clock {
        fn now(&self) -> u64 {
            0
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {}
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

    fn write(model: &mut SerialAudio, address: u64, value: u64) -> Option<String> {
        let mut host = Clock;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        model.write(&mut cx, address, 4, value);
        cx.take_refusal()
    }

    fn read(model: &mut SerialAudio, address: u64) -> (u64, Option<String>) {
        let mut host = Clock;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        let value = model.read(&mut cx, address, 4);
        (value, cx.take_refusal())
    }

    #[test]
    fn retains_control_and_transfer_words_with_register_laws() {
        let mut model = SerialAudio::default();
        assert_eq!(
            write(&mut model, BASES[0] + CONTROL as u64, 0x0101_0000),
            None
        );
        assert_eq!(
            write(&mut model, BASES[0] + FORMAT as u64, 0x0002_0001),
            None
        );
        assert_eq!(write(&mut model, BASES[0] + CHANNEL_ENABLE as u64, 1), None);
        assert_eq!(write(&mut model, BASES[0] + 0x20, 0x1234), None);
        assert_eq!(write(&mut model, BASES[0] + 0, 1), None);
        assert_eq!(read(&mut model, BASES[0]), (0, None));
        assert!(write(&mut model, BASES[0] + CONTROL as u64, 0x0003_0000).is_some());
        assert!(write(&mut model, BASES[0] + FORMAT as u64, 3).is_some());
        assert!(write(&mut model, BASES[0] + STATUS as u64, 1).is_some());
        assert!(read(&mut model, BASES[0] + 0x44).1.is_some());
    }

    #[test]
    fn transfer_fields_reject_other_cores_without_a_lifetime_write_limit() {
        let mut model = SerialAudio::default();
        let mut host = Clock;
        let mut cx = Cx::new(&mut host, 1, 0, 0);
        model.write(&mut cx, BASES[0] + FORMAT as u64, 4, 1);
        assert!(cx.take_refusal().is_some());

        // X-E2S lib-bindings/values/values_0001_0031 exhausted the former
        // diagnostic ledger during stock UI sounds. Register programming
        // has no hardware lifetime quota, even when the same sound repeats.
        for n in 0..8193 {
            assert_eq!(write(&mut model, BASES[0] + FORMAT as u64, n & 1), None);
        }
        assert_eq!(read(&mut model, BASES[0] + FORMAT as u64), (0, None));
        assert!(SerialAudio::valid_state(&model.state));
    }

    #[test]
    fn dma_sink_requires_enabled_format_and_explicit_connection() {
        let mut model = SerialAudio::new(
            Options::default(),
            [
                AudioSink::new(fujimod_emulator_parts_io::audio_sink::Options { connected: true }),
                AudioSink::default(),
            ],
        );
        assert!(!model.can_transmit(BASES[0] + FIFO_ENDPOINT).unwrap());
        assert!(
            model
                .transmit_dma(BASES[0] + FIFO_ENDPOINT, &[1, 2])
                .is_err()
        );
        assert_eq!(write(&mut model, BASES[0] + FORMAT as u64, 0x2_0000), None);
        assert_eq!(model.can_transmit(BASES[0] + FIFO_ENDPOINT), Ok(true));
        model
            .transmit_dma(BASES[0] + FIFO_ENDPOINT, &[1, 2, 3])
            .unwrap();
        assert_eq!(model.sink_bytes_received(0), Some(3));
        assert!(model.can_transmit(BASES[1] + FIFO_ENDPOINT + 4).is_err());
    }

    #[test]
    fn checkpoint_preserves_banks_and_sink_counts() {
        let mut model = SerialAudio::new(
            Options::default(),
            [
                AudioSink::new(fujimod_emulator_parts_io::audio_sink::Options { connected: true }),
                AudioSink::default(),
            ],
        );
        write(&mut model, BASES[0] + FORMAT as u64, 0x2_0000);
        model
            .transmit_dma(BASES[0] + FIFO_ENDPOINT, &[1, 2])
            .unwrap();
        let saved = model.save();
        let mut restored = SerialAudio::new(
            Options::default(),
            [
                AudioSink::new(fujimod_emulator_parts_io::audio_sink::Options { connected: true }),
                AudioSink::default(),
            ],
        );
        restored.restore(&saved).unwrap();
        assert_eq!(restored.sink_bytes_received(0), Some(2));
        assert_eq!(
            read(&mut restored, BASES[0] + FORMAT as u64),
            (0x2_0000, None)
        );
    }
}
