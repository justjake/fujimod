//! EXR Processor Pro analog-to-digital converter at 0xFFFA8000.
//!
//! +00 and +04 are two control words: bit 24 starts and bit 25 stops, and bit 26 reads as ready after either.
//! A start on +04 also raises the declared completion interrupt after the declared delay: start-up 0x7a085c
//! writes the start, then registers its handler on IRQ index 0x42 and waits for it with a 100-tick timeout.
//! The +00 control (loader 0x44dc) is polled and raises nothing. +10 + 4n holds channel n's 10-bit result
//! (0x7a07c0 masks it with 0x3ff); each channel samples its external input level, declared at construction and
//! changed through the analog-input capability (dials, switches and resistor-ladder buttons).
use crate::bus::InterruptLine;
use crate::capability::AnalogInputs;
use crate::parts::analog::Samples;
use crate::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const BASE: u64 = 0xfffa8000;

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    /// Interrupt raised after a +04 start; none when absent.
    pub start_irq: Option<InterruptLine>,
    /// Ticks from a +04 start to its interrupt.
    pub start_ticks: u64,
    /// The 10-bit level each of the eight inputs converts to at construction.
    pub inputs: [u16; CHANNELS],
}

const CHANNELS: usize = 8;
const RESULTS: u64 = 0x10;

#[derive(Serialize, Deserialize)]
struct State {
    control: [u32; 2],
    ready: [bool; 2],
    words: [u32; 2],
    start_due: Option<u64>,
}

/// The converter's control, result and data words.
pub struct Adc {
    options: Options,
    state: State,
    samples: Samples,
}

impl Adc {
    pub fn new(options: Options) -> Result<Self, String> {
        if options.inputs.iter().any(|level| *level > 0x3ff) {
            return Err("ADC input levels are 10-bit".into());
        }
        Ok(Self {
            samples: Samples::new(options.inputs.to_vec())?,
            options,
            state: State {
                control: [0; 2],
                ready: [false; 2],
                words: [0; 2],
                start_due: None,
            },
        })
    }

    fn offset(address: u64, size: u8) -> Option<u64> {
        if size != 4 || address < BASE {
            return None;
        }
        let offset = address - BASE;
        (matches!(offset, 0 | 4 | 0x40 | 0x44) || (RESULTS..RESULTS + 4 * CHANNELS as u64).contains(&offset) && offset & 3 == 0)
            .then_some(offset)
    }
}

impl Device for Adc {
    crate::checkpoint_fields!(state: State);

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr-pro-adc"
    }

    fn analog_inputs(&self) -> Option<&(dyn AnalogInputs + 'static)> {
        Some(self)
    }

    fn analog_inputs_mut(&mut self) -> Option<&mut (dyn AnalogInputs + 'static)> {
        Some(self)
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
            0x40 | 0x44 => u64::from(self.state.words[(offset / 4 - 0x10) as usize]),
            _ => match self.samples.get(((offset - RESULTS) / 4) as usize) {
                Ok(level) => u64::from(level),
                Err(error) => {
                    cx.fail(error);
                    0
                }
            },
        }
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = Self::offset(address, size) else {
            cx.unmodeled();
            return;
        };
        let Ok(value) = u32::try_from(value) else {
            cx.fail("ADC requires 32-bit register values");
            return;
        };
        match offset {
            0 | 4 => {
                let channel = (offset / 4) as usize;
                let starting = value & 1 << 24 != 0 && self.state.control[channel] & 1 << 24 == 0;
                self.state.control[channel] = value;
                self.state.ready[channel] = value & (1 << 24 | 1 << 25) != 0;
                if channel == 1 && starting && self.options.start_irq.is_some() {
                    let Some(due) = cx.now().checked_add(self.options.start_ticks) else {
                        cx.fail("ADC start interrupt deadline overflow");
                        return;
                    };
                    self.state.start_due = Some(due);
                }
            }
            0x40 | 0x44 => self.state.words[(offset / 4 - 0x10) as usize] = value,
            _ => cx.fail("ADC results are read-only"),
        }
    }

    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.start_due
    }

    fn advance(&mut self, cx: &mut Cx<'_>) {
        if !self.state.start_due.is_some_and(|due| due <= cx.now()) {
            return;
        }
        self.state.start_due = None;
        match self.options.start_irq {
            Some(InterruptLine::Core { core, line }) => {
                if let Err(error) = cx.raise_irq(core, line) {
                    cx.fail(error);
                }
            }
            Some(InterruptLine::Shared(line)) => cx.set_irq(line, true),
            None => (),
        }
    }

    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }

    fn save(&self) -> Value {
        json!({"state": self.state, "samples": self.samples.values()})
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value["state"].clone())
            .map_err(|error| format!("invalid ADC state: {error}"))?;
        if let Some(samples) = value.get("samples") {
            let samples: Vec<u16> =
                serde_json::from_value(samples.clone()).map_err(|error| format!("invalid ADC samples: {error}"))?;
            self.samples.replace(&samples)?;
        }
        self.state = state;
        Ok(())
    }
}

impl AnalogInputs for Adc {
    fn external_inputs(&self) -> Samples {
        self.samples.clone()
    }

    fn samples(&self) -> Vec<u16> {
        self.samples.values()
    }

    fn set_sample(&mut self, channel: usize, value: u16) -> Result<(), String> {
        if value > 0x3ff {
            return Err("ADC input levels are 10-bit".into());
        }
        self.samples.set(channel, value)
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

    fn read(model: &mut Adc, offset: u64, size: u8) -> (u64, Option<String>) {
        let mut host = Clock;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        let value = model.read(&mut cx, BASE + offset, size);
        (value, cx.take_refusal())
    }

    fn write(model: &mut Adc, offset: u64, size: u8, value: u64) -> Option<String> {
        let mut host = Clock;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        model.write(&mut cx, BASE + offset, size, value);
        cx.take_refusal()
    }

    #[test]
    fn start_bit_sets_ready_within_the_control_word() {
        let mut model = Adc::new(Options::default()).unwrap();
        assert_eq!(read(&mut model, 0, 4), (0, None));
        assert_eq!(write(&mut model, 0, 4, 0x0100_0000), None);
        let (value, refusal) = read(&mut model, 0, 4);
        assert_eq!(refusal, None);
        assert_eq!(value, 0x0500_0000);
        assert_eq!(value & 1 << 26, 1 << 26);
    }

    #[test]
    fn ready_follows_start_or_stop_and_not_plain_configuration() {
        let mut model = Adc::new(Options::default()).unwrap();
        assert_eq!(write(&mut model, 4, 4, 0x4000_0ff0), None);
        assert_eq!(read(&mut model, 4, 4).0 & 1 << 26, 0);
        assert_eq!(write(&mut model, 4, 4, 0x4000_0ff0 | 0x0200_0000), None);
        assert_eq!(read(&mut model, 4, 4).0 & 1 << 26, 1 << 26);
        assert_eq!(read(&mut model, 4, 4).0 & !(1 << 26), 0x4200_0ff0);
    }

    #[test]
    fn data_words_round_trip() {
        let mut model = Adc::new(Options::default()).unwrap();
        assert_eq!(write(&mut model, 0x40, 4, 0x1234_5678), None);
        assert_eq!(write(&mut model, 0x44, 4, 0x9abc_def0), None);
        assert_eq!(read(&mut model, 0x40, 4), (0x1234_5678, None));
        assert_eq!(read(&mut model, 0x44, 4), (0x9abc_def0, None));
    }

    #[test]
    fn unknown_or_misaligned_access_is_unmodeled() {
        let mut model = Adc::new(Options::default()).unwrap();
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
        let mut model = Adc::new(Options::default()).unwrap();
        write(&mut model, 0, 4, 0x0100_0000);
        write(&mut model, 0x40, 4, 0x2aa);
        let mut restored = Adc::new(Options::default()).unwrap();
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

#[cfg(test)]
mod start_irq_tests {
    use super::*;
    use crate::test_host::{TestHost, advance, write};

    #[test]
    fn a_plus_04_start_raises_the_declared_interrupt_after_its_delay() {
        let mut model = Adc::new(Options {
            start_irq: Some(InterruptLine::Core { core: 0, line: 0x48 }),
            start_ticks: 10,
            inputs: [0; CHANNELS],
        })
        .unwrap();
        let mut host = TestHost::new();
        host.now = 5;
        write(&mut model, &mut host, BASE, 1 << 24);
        assert_eq!(model.next_deadline(host.now), None);
        write(&mut model, &mut host, BASE + 4, 1 << 24);
        assert_eq!(model.next_deadline(host.now), Some(15));
        advance(&mut model, &mut host, 14);
        assert!(host.irqs.is_empty());
        advance(&mut model, &mut host, 15);
        assert_eq!(host.irqs, [(0, 0x48)]);
        assert_eq!(model.next_deadline(host.now), None);
    }
}

#[cfg(test)]
mod result_tests {
    use super::*;
    use crate::test_host::{TestHost, read};

    #[test]
    fn changed_inputs_are_converted_and_checkpointed() {
        let mut adc = Adc::new(Options { inputs: [1; CHANNELS], ..Options::default() }).unwrap();
        let mut host = TestHost::new();
        adc.set_sample(3, 0x162).unwrap();
        assert!(adc.set_sample(3, 0x400).is_err());
        assert_eq!(read(&mut adc, &mut host, BASE + RESULTS + 12), 0x162);
        let saved = adc.save();
        adc.set_sample(3, 7).unwrap();
        adc.restore(&saved).unwrap();
        assert_eq!(read(&mut adc, &mut host, BASE + RESULTS + 12), 0x162);
    }

    #[test]
    fn results_report_the_declared_inputs_and_are_read_only() {
        let inputs = [368, 715, 415, 63, 516, 646, 18, 839];
        let mut model = Adc::new(Options { inputs, ..Options::default() }).unwrap();
        let mut host = TestHost::new();
        for (n, level) in inputs.iter().enumerate() {
            assert_eq!(read(&mut model, &mut host, BASE + 0x10 + 4 * n as u64), u64::from(*level));
        }
        let saved = model.save();
        let mut cx = host.cx();
        model.write(&mut cx, BASE + 0x14, 4, 1);
        assert!(cx.refusal().is_some());
        assert_eq!(model.save(), saved);
        let mut cx = host.cx();
        model.read(&mut cx, BASE + 0x30, 4);
        assert!(cx.is_unmodeled());
        assert!(Adc::new(Options { inputs: [0x400, 0, 0, 0, 0, 0, 0, 0], ..Options::default() }).is_err());
    }
}
