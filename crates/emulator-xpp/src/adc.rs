//! X-Processor Pro twelve-channel ADC.
//!
//! The X-T20 2.11 kernel (0x5ac00..0x5adf8) supplies the register contract:
//! control word +0 holds start bit 0, continuous-mode bit 1 and channel
//! enables in bits 16..25; +4 holds the conversion clock divider; +8 bit 0 is
//! written as the inverse of the enable argument (0x5abdc, a power-down bit);
//! each channel's 10-bit result reads at `+0x10 + 4*channel`. A single conversion
//! clears bit 0 after the declared conversion ticks; in continuous mode bit 0
//! stays set. Results are explicit inputs, not analog hardware, and read the
//! same before and after a conversion. The power-down bit is stored only;
//! conversions do not depend on it.
//!
//! [`Converter::set_input`] changes a channel's result, as a key on the
//! X-T20's channel 3 ladder does (pad, MENU/OK, Q, DISP/BACK; bands from the
//! configuration store). The changed results are part of the checkpoint.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Register window size in bytes.
pub const WINDOW: u64 = 0x40;
const CHANNELS: usize = 12;
const START: u32 = 1;
const CONTINUOUS: u32 = 1 << 1;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    control: u32,
    divider: u32,
    power_down: u32,
    deadline: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    inputs: BTreeMap<usize, u16>,
}
pub struct Converter {
    base: u64,
    samples: [u16; CHANNELS],
    conversion_ticks: u64,
    state: State,
}
impl Converter {
    pub fn new(base: u64, samples: &[u16], conversion_ticks: u64) -> Result<Self, String> {
        let samples: [u16; CHANNELS] = samples
            .try_into()
            .map_err(|_| "X-Processor Pro ADC needs twelve samples")?;
        if base & 0xfff != 0 || samples.iter().any(|v| *v > 0x3ff) || conversion_ticks == 0 {
            return Err("Invalid X-Processor Pro ADC base, sample or conversion clock".into());
        }
        Ok(Self {
            base,
            samples,
            conversion_ticks,
            state: State::default(),
        })
    }
    /// An external input: `channel` reads `sample` until the next change.
    pub fn set_input(&mut self, channel: usize, sample: u16) -> Result<(), String> {
        if channel >= CHANNELS || sample > 0x3ff {
            return Err("Invalid X-Processor Pro ADC input".into());
        }
        if sample == self.samples[channel] {
            self.state.inputs.remove(&channel);
        } else {
            self.state.inputs.insert(channel, sample);
        }
        Ok(())
    }
    fn sample(&self, channel: usize) -> u16 {
        self.state
            .inputs
            .get(&channel)
            .copied()
            .unwrap_or(self.samples[channel])
    }
}
impl Device for Converter {
    fn name(&self) -> &'static str {
        "xpp-adc"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let o = address - self.base;
        if size != 4 || o & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        u64::from(match o {
            0 => self.state.control,
            4 => self.state.divider,
            8 => self.state.power_down,
            0x10..WINDOW => u32::from(self.sample(((o - 0x10) / 4) as usize)),
            _ => {
                cx.unmodeled();
                0
            }
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let o = address - self.base;
        if size != 4 || value > u32::MAX.into() {
            cx.unmodeled();
            return;
        }
        let value = value as u32;
        match o {
            0 => {
                // A pending single conversion keeps its deadline; any other
                // single-mode start begins one.
                let single = value & (START | CONTINUOUS) == START;
                self.state.control = value;
                self.state.deadline = match (single, self.state.deadline) {
                    (false, _) => None,
                    (true, Some(at)) => Some(at),
                    (true, None) => match cx.now().checked_add(self.conversion_ticks) {
                        Some(at) => Some(at),
                        None => {
                            cx.fail("X-Processor Pro ADC deadline overflow");
                            None
                        }
                    },
                };
            }
            4 => self.state.divider = value,
            8 if value <= 1 => self.state.power_down = value,
            _ => cx.unmodeled(),
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.state.deadline.is_some_and(|at| at <= cx.now()) {
            self.state.deadline = None;
            self.state.control &= !START;
        }
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.deadline
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state.deadline.is_some() != (state.control & (START | CONTINUOUS) == START) {
            return Err("X-Processor Pro ADC checkpoint deadline differs from its control".into());
        }
        if state
            .inputs
            .iter()
            .any(|(&channel, &sample)| channel >= CHANNELS || sample > 0x3ff)
        {
            return Err("Invalid X-Processor Pro ADC checkpoint input".into());
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, read, write};

    const BASE: u64 = 0xfff19000;
    fn converter() -> Converter {
        let samples: Vec<u16> = (0..12).map(|n| n * 10).collect();
        Converter::new(BASE, &samples, 16).unwrap()
    }

    #[test]
    fn single_conversions_clear_start_after_the_conversion_clock() {
        assert!(Converter::new(BASE, &[0; 11], 16).is_err());
        assert!(Converter::new(BASE, &[0x400; 12], 16).is_err());
        let mut adc = converter();
        let mut h = TestHost::new();
        write(&mut adc, &mut h, BASE + 4, 0x40);
        write(&mut adc, &mut h, BASE, 0x3ff << 16 | START);
        assert_eq!(read(&mut adc, &mut h, BASE), u64::from(0x3ff << 16 | START));
        assert_eq!(adc.next_deadline(0), Some(16));
        h.now = 16;
        adc.advance(&mut h.cx());
        assert_eq!(read(&mut adc, &mut h, BASE), 0x3ff << 16);
        assert_eq!(read(&mut adc, &mut h, BASE + 0x10 + 4 * 3), 30);
        assert_eq!(read(&mut adc, &mut h, BASE + 4), 0x40);
        write(&mut adc, &mut h, BASE + 8, 1);
        assert_eq!(read(&mut adc, &mut h, BASE + 8), 1);
        let mut cx = h.cx();
        adc.write(&mut cx, BASE + 8, 4, 2);
        assert!(cx.is_unmodeled());
        let mut cx = h.cx();
        adc.read(&mut cx, BASE + 0xc, 4);
        assert!(cx.is_unmodeled());
    }

    #[test]
    fn continuous_mode_keeps_start_set_and_checkpoints_restore() {
        let mut adc = converter();
        let mut h = TestHost::new();
        write(&mut adc, &mut h, BASE, 1 << 16 | CONTINUOUS | START);
        assert_eq!(adc.next_deadline(0), None);
        let saved = adc.save();
        write(&mut adc, &mut h, BASE, START);
        assert_eq!(adc.next_deadline(0), Some(16));
        adc.restore(&saved).unwrap();
        assert_eq!(
            read(&mut adc, &mut h, BASE),
            u64::from(1 << 16 | CONTINUOUS | START)
        );
        assert!(
            adc.restore(
                &serde_json::json!({"control": 1, "divider": 0, "power_down": 0, "deadline": null})
            )
            .is_err()
        );
    }

    #[test]
    fn external_inputs_change_results_and_checkpoint() {
        let mut adc = converter();
        let mut h = TestHost::new();
        let saved = adc.save();
        // MENU/OK on the X-T20 ladder: band 350..516 on channel 3.
        adc.set_input(3, 433).unwrap();
        assert_eq!(read(&mut adc, &mut h, BASE + 0x10 + 4 * 3), 433);
        let pressed = adc.save();
        adc.restore(&saved).unwrap();
        assert_eq!(read(&mut adc, &mut h, BASE + 0x10 + 4 * 3), 30);
        adc.restore(&pressed).unwrap();
        assert_eq!(read(&mut adc, &mut h, BASE + 0x10 + 4 * 3), 433);
        adc.set_input(3, 30).unwrap();
        assert_eq!(adc.save(), saved);
        assert!(adc.set_input(12, 0).is_err());
        assert!(adc.set_input(3, 0x400).is_err());
        let mut state = pressed;
        state["inputs"] = serde_json::json!({"12": 1});
        assert!(adc.restore(&state).is_err());
    }
}
