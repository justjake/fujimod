//! XP5 GPIO pin state that the controller, its drivers and its inputs share.
use crate::parts::pins::{DriveInput, Pin};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Port {
    pub registers: [u32; 9],
    pub input: u16,
    pub output_rising_edges: [u64; 16],
}
/// How the pad-sampling word (+0x14) is formed from a port's registers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PadModel {
    /// +8 is the output mask and +4 the data (the original model; the default).
    #[default]
    Legacy,
    /// +4 is the output enable and +0 the output-data latch. Inferred from the stock drive writer
    /// (data at +0) and from a setup program that sets +4 and clears +8 to make a pin an output.
    DataLatch,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Pins {
    #[serde(default)]
    pub pad_model: PadModel,
    pub ports: Vec<Port>,
    pub global: u16,
    pub special: u32,
    #[serde(default)]
    pub focus_pulses: u64,
}
/// The GPIO pin state that the controller, its drivers and its inputs share.
pub struct PinBank(Mutex<Pins>);
impl std::ops::Deref for PinBank {
    type Target = Mutex<Pins>;
    fn deref(&self) -> &Mutex<Pins> {
        &self.0
    }
}
pub type SharedPins = Arc<PinBank>;
impl DriveInput for PinBank {
    fn set_input(&self, pin: Pin, high: bool) {
        self.lock()
            .unwrap()
            .set_input(usize::from(pin.port), usize::from(pin.pin), high);
    }
}
impl Default for Pins {
    fn default() -> Self {
        Self {
            pad_model: PadModel::default(),
            ports: vec![
                Port {
                    input: 0xffff,
                    ..Port::default()
                };
                20
            ],
            global: 0,
            special: 0,
            focus_pulses: 0,
        }
    }
}
impl Pins {
    pub fn shared() -> SharedPins {
        Arc::new(PinBank(Mutex::new(Self::default())))
    }
    pub fn set_input(&mut self, port: usize, pin: usize, high: bool) {
        let bit = 1u16 << pin;
        if high {
            self.ports[port].input |= bit;
        } else {
            self.ports[port].input &= !bit;
        }
    }
    /// Output-data latch at +0, written by the masked drive operation
    /// (146c3d4). Attached GPIO chip selects consume this drive signal.
    pub fn output_level(&self, port: usize, pin: usize) -> u32 {
        (self.ports[port].registers[0] >> pin) & 1
    }
    /// Count output-data low-to-high transitions even if a peripheral did not
    /// access its bus between deselection and selection.
    pub fn output_rising_edges(&self, port: usize, pin: usize) -> u64 {
        self.ports[port].output_rising_edges[pin]
    }
    pub fn level(&self, port: usize, pin: usize) -> u32 {
        (self.word(port) >> pin) & 1
    }
    /// The pad-sampling word (+0x14): an output pin reads its output-data latch, any other pin
    /// its external level. Which registers hold the output enable and the data depends on
    /// `pad_model`.
    pub fn word(&self, port: usize) -> u32 {
        let p = &self.ports[port];
        let (output, data) = match self.pad_model {
            PadModel::Legacy => (p.registers[2] & 0xffff, p.registers[1]),
            PadModel::DataLatch => (p.registers[1] & 0xffff, p.registers[0]),
        };
        (p.input as u32 & !output) | (data & output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_output_pin_reads_its_data_latch_and_an_input_pin_its_pad() {
        let mut pins = Pins::default();
        pins.ports[0].registers[0] = 0x00f0; // +0: output data
        pins.ports[0].registers[1] = 0x0030; // +4
        pins.ports[0].registers[2] = 0xffff; // +8
        pins.set_input(0, 0, false);
        pins.set_input(0, 4, false);
        // Legacy (default): +8 selects every pin as an output, and +4 is the data.
        assert_eq!(pins.pad_model, PadModel::Legacy);
        assert_eq!(pins.word(0), 0x0030);
        // Data latch: +4 selects pins 4 and 5; the others follow their pads.
        pins.pad_model = PadModel::DataLatch;
        assert_eq!(pins.word(0), 0xfffe);
        assert_eq!(pins.level(0, 0), 0);
        assert_eq!(pins.level(0, 4), 1);
        // With no output enabled, every pin follows its pad whatever +8 holds.
        pins.ports[0].registers[1] = 0;
        assert_eq!(pins.word(0), 0xffee);
    }
}
