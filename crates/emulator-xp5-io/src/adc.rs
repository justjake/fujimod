//! XP5 15-channel ADC sequencer. External resistor values are explicit inputs.
use fujimod_emulator_devices::parts::controls::Controls;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

pub const BASE: u64 = 0xff906000;
#[derive(Serialize, Deserialize)]
struct State {
    enable: u32,
    config: [u32; 2],
    ack: u32,
    mask: u32,
    #[serde(default)]
    timing: u32,
    samples: [u16; 15],
}
pub struct Adc {
    state: State,
    input: Arc<dyn Controls>,
}
impl Adc {
    pub fn new(input: Arc<dyn Controls>, samples: [u16; 15]) -> Result<Self, String> {
        if samples.iter().any(|v| *v > 4095) {
            return Err("ADC samples must be 12-bit".into());
        }
        Ok(Self {
            state: State {
                enable: 0,
                config: [0; 2],
                ack: 0,
                mask: 0,
                timing: 0,
                samples,
            },
            input,
        })
    }
}
impl Device for Adc {
    fn name(&self) -> &'static str {
        "adc-sequencer-input"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(BASE);
        if size != 4 || offset & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        match offset {
            0 => self.state.enable as u64,
            4 | 8 => self.state.config[(offset / 4 - 1) as usize] as u64,
            0xc => self.state.ack as u64,
            0x10..=0x48 => {
                let n = ((offset - 0x10) / 4) as usize;
                self.input.sample(n, self.state.samples[n]) as u64
            }
            0x4c => self.state.mask as u64,
            0x50 => 8,
            0x54 => self.state.timing as u64,
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(BASE);
        if size != 4 {
            cx.unmodeled();
            return;
        }
        match offset {
            0 => {
                self.state.enable = value as u32;
                if value & 1 != 0 {
                    self.state.ack |= 1;
                }
            }
            4 | 8 => self.state.config[(offset / 4 - 1) as usize] = value as u32,
            0xc => self.state.ack = value as u32 & 1,
            0x4c => self.state.mask = value as u32 & 0x7fff,
            0x54 => {
                if value > 15 {
                    cx.fail("unmodeled ADC +54 configuration outside stock 4-bit range");
                } else {
                    self.state.timing = value as u32;
                }
            }
            _ => cx.unmodeled(),
        }
    }
    fn save(&self) -> Value {
        json!({"adc":self.state, "input":self.input.save()})
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        let state: State =
            serde_json::from_value(v["adc"].clone()).map_err(|e| format!("ADC checkpoint: {e}"))?;
        if state.samples.iter().any(|v| *v > 4095) {
            return Err("ADC checkpoint sample exceeds 12-bit range".into());
        }
        self.input.restore(&v["input"])?;
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{parts::pins::{DriveInput, Pin}, test_host::recording::RecordingHost};
    use fujimod_emulator_parts_controls::resistor_keys::{InitialLevel, KeySamples, Layout, Options, ResistorKeys};
    use fujimod_emulator_devices::xp5::pins::Pins;
    #[test]
    fn adc_reads_declared_external_levels_without_firmware_ram() {
        let pins = Pins::shared();
        let driver: Arc<dyn DriveInput> = pins;
        let input = ResistorKeys::new(
            driver,
            Options {
                layout: Layout {
                    shutter_half: Pin { port: 0, pin: 0 },
                    shutter_full: Pin { port: 0, pin: 1 },
                    rear_dial: [Pin { port: 5, pin: 7 }, Pin { port: 5, pin: 9 }],
                    front_dial: [Pin { port: 5, pin: 8 }, Pin { port: 13, pin: 5 }],
                },
                initial_levels: [
                    (0, 0, true),
                    (0, 1, true),
                    (5, 7, false),
                    (5, 9, false),
                    (5, 8, false),
                    (13, 5, false),
                ]
                .into_iter()
                .map(|(port, pin, high)| InitialLevel {
                    pin: Pin { port, pin },
                    high,
                })
                .collect(),
                samples: KeySamples {
                    menu: 3472,
                    back: 2227,
                    menu_back: 1999,
                    up: 1558,
                    down: 3409,
                    left: 2829,
                    right: 2068,
                },
                detent_return_ns: 60_000_000,
                shutter_half_delay_ns: 30_000_000,
            },
        )
        .unwrap();
        input.script("t=1:MENU;t=1:UP").unwrap();
        let mut samples = [4095; 15];
        samples[7] = 2241;
        let mut adc = Adc::new(Arc::new(input.clone()), samples).unwrap();
        let mut host = RecordingHost::default(); // No RAM, no private config tables.
        let mut cx = Cx::new(&mut host, 3, 0xdeadbeef, 0);
        assert_eq!(adc.read(&mut cx, BASE + 0x10 + 7 * 4, 4), 2241);
        assert_eq!(adc.read(&mut cx, BASE + 0x10 + 11 * 4, 4), 4095);
        input.advance(1_000_000_000);
        assert_eq!(adc.read(&mut cx, BASE + 0x10 + 11 * 4, 4), 3472);
        assert_eq!(adc.read(&mut cx, BASE + 0x10 + 14 * 4, 4), 1558);
        assert!(cx.refusal().is_none());
        let saved = adc.save();
        input.script("t=1:-MENU;t=1:-UP").unwrap();
        input.advance(2_000_000_000);
        adc.restore(&saved).unwrap();
        assert_eq!(adc.read(&mut cx, BASE + 0x10 + 11 * 4, 4), 3472);
    }
}
