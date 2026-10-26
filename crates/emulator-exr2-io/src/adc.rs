//! Twelve-channel ADC at FFF8A000. Register selection controls which external
//! channels a conversion samples; result and completion registers expose it.
//! Sample values and conversion delay are explicit inputs, not analog hardware.
use fujimod_emulator_devices::record::{Result, Unsupported};
use fujimod_emulator_devices::{Action, Record, Value};
pub const BASE: u32 = 0xfff8a000;
#[derive(Clone, Debug)]
pub struct Adc {
    pub busy: bool,
    pub done: u32,
    pub generation: u64,
    pub samples: Vec<u32>,
    // Host edits may install a mutable/non-numeric Python input. Such sample
    // reads must return to that host rather than use a stale numeric copy.
    pub native_samples: bool,
}
impl Default for Adc {
    fn default() -> Self {
        Self {
            busy: false,
            done: 0,
            generation: 0,
            samples: vec![0; 12],
            native_samples: true,
        }
    }
}
impl Adc {
    pub fn start(
        &mut self,
        cpu: i64,
        channels: u32,
        actions: &mut Vec<Action>,
    ) -> Result<(u64, u32)> {
        if self.busy {
            return Err(Unsupported::from("Boot ADC start while busy"));
        }
        self.generation += 1;
        self.busy = true;
        self.done = 0;
        let channels = channels & 0xfff;
        actions.push(Action::Log(Record::new(
            "Boot ADC sample start",
            [
                ("cpu", Value::int(cpu)),
                ("channels", Value::hex(channels as u128)),
            ],
        )));
        Ok((self.generation, channels))
    }
    pub fn complete(
        &mut self,
        cpu: i64,
        generation: u64,
        channels: u32,
        actions: &mut Vec<Action>,
    ) {
        if generation == self.generation {
            self.busy = false;
            self.done = channels;
            actions.push(Action::Log(Record::new(
                "Boot ADC sample complete",
                [
                    ("cpu", Value::int(cpu)),
                    ("provenance", Value::str("unmeasured sample fixtures")),
                ],
            )));
        }
    }
    pub fn reset(&mut self) {
        self.generation += 1;
        self.busy = false;
        self.done = 0;
    }
}

pub mod controller;
