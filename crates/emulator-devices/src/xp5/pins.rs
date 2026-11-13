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
#[derive(Clone, Serialize, Deserialize)]
pub struct Pins {
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
    /// The pad-sampling word (+0x14): an output pin reads its output-data latch (+0), any other
    /// pin its external level. +4 selects the outputs: the stock drive writer addresses the data
    /// latch at +0 (selector 0) and a setup program makes a pin an output by setting its +4 bit
    /// while clearing its +8 bit. Inferred from those writers, not read from hardware.
    pub fn word(&self, port: usize) -> u32 {
        let p = &self.ports[port];
        let output = p.registers[1] & 0xffff;
        (p.input as u32 & !output) | (p.registers[0] & output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_output_pin_reads_its_data_latch_and_an_input_pin_its_pad() {
        let mut pins = Pins::default();
        pins.ports[0].registers[0] = 0x00f0; // +0: output data
        pins.ports[0].registers[1] = 0x0030; // +4: pins 4 and 5 are outputs
        pins.ports[0].registers[2] = 0xffff; // +8 does not select outputs
        pins.set_input(0, 0, false); // an input pin follows its pad
        pins.set_input(0, 4, false); // an output pin ignores its pad
        assert_eq!(pins.word(0), 0xfffe);
        assert_eq!(pins.level(0, 0), 0);
        assert_eq!(pins.level(0, 4), 1);
        // With no output enabled, every pin follows its pad whatever +8 holds.
        pins.ports[0].registers[1] = 0;
        assert_eq!(pins.word(0), 0xffee);
    }
}
