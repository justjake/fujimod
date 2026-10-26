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
    pub fn word(&self, port: usize) -> u32 {
        let p = &self.ports[port];
        let output = p.registers[2] & 0xffff;
        (p.input as u32 & !output) | (p.registers[1] & output)
    }
}
