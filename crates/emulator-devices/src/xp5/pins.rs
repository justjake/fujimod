//! XP5 GPIO pin state that the controller, its drivers and its inputs share.
use crate::parts::pins::{DriveInput, LowHistory, OutputLowHistory, Pin};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Port {
    pub registers: [u32; 9],
    pub input: u16,
    /// Pins whose external level something has set (a declared input, a wire, a live input).
    #[serde(default)]
    pub driven: u16,
    pub output_rising_edges: [u64; 16],
    /// Time the output-data latch last fell, while the pin is still low.
    #[serde(default)]
    pub output_low_since: [Option<u64>; 16],
    /// Duration of the most recently completed low interval of the latch.
    #[serde(default)]
    pub output_last_low_ticks: [u64; 16],
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
    /// The data-latch model, and a read of +0 gives the pad level of every pin whose output
    /// enable (+4) is clear. The stock level getter (GFX100RF 1.13 0x14711f8 / 0x147127c /
    /// 0x14712c0) reads +0x14 for a pin whose +8 bit is set and +0 for any other pin; the card
    /// detect reader 0x1496f80 goes that way for 11.12 and 9.3, whose +8 bits nothing sets, and
    /// takes a nonzero level as "no card". With +0 answering the latch both slots read a card
    /// that is not there. That +0 answers the pad for an input pin is inferred from this, not
    /// measured on a camera.
    DataLatchPins,
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
    /// Net focus motor steps (signed), for a body whose position counter counts both ways.
    #[serde(default)]
    pub focus_position: i64,
    /// A focus motor driven by a PWM pair, for a body whose companion core runs a position
    /// servo (GFX100RF). Absent on bodies that step their focus motor in counted batches.
    #[serde(default)]
    pub focus_servo: Option<FocusServo>,
}
/// Velocity plant of a PWM-driven focus motor: the position moves at `gain` counts per second
/// for each PWM count of drive. The gain is a stand-in, not a measured motor constant.
#[derive(Clone, Serialize, Deserialize)]
pub struct FocusServo {
    /// Present drive in PWM counts: second compare minus half the period.
    pub drive: i64,
    /// Guest time the position was last advanced to.
    pub at_ns: u64,
    /// Position remainder, in counts times 1e9.
    pub remainder: i64,
    pub gain: i64,
    /// The home sensor reads high while the position is above this.
    pub home_above: i64,
    pub home: (usize, usize),
    /// The servo has programmed its PWM channel. Until then the home sensor's pad and
    /// interrupt lines are left as the board set them: nothing is published at boot.
    #[serde(default)]
    pub started: bool,
    /// Bits 1:0 of the motor driver's register 0x1b (GFX100RF 1.13 field table 0x1baf200,
    /// fields 0x27 = bit 1 and 0x28 = bit 0), as last written.
    /// Bit 0 (0x1660474, from 0x165a330) is taken as the bridge's output enable: the lens module
    /// sets it, waits, and only then starts the position counter and the servo (0x165a8dc), and
    /// clears it when it stops or fails (0x165a890). The PWM moves the motor only while it is set.
    /// Bit 1 (0x16603e4, from 0x165a244) is not an enable of the motor: the module sets it before
    /// its first read of the home sensor and clears it when the home search ends, and it goes on
    /// moving the motor after that (run rdv1-t1: cleared at the state 7 -> 8 step, the move of
    /// state 8 is still expected to finish). It is probably the home sensor's light source; the
    /// model does not use it. Both readings come from those call sites, not from a datasheet.
    #[serde(default)]
    pub enable: u8,
    /// The pads that carry the PWM to the bridge. The drive counts only while every one of them
    /// is on its timer function (a nonzero 2-bit field in the port's function words +0xc / +0x10).
    /// The GFX100RF companion takes 2.0 and 2.1 off the timer and holds both high as plain GPIO
    /// whenever its servo is off (local 0x18ec0(2) -> 0x1d118(pin, 0), 0x1d04c(pin, 1)), and the
    /// imaging code reprograms the PWM period in that state, which leaves a duty that is not
    /// neutral for the new period. Without this gate that duty moved the lens for a second and
    /// the lens module's watchdog (0x165ac24) raised a focus error.
    #[serde(default)]
    pub pwm_pins: Vec<(usize, usize)>,
}
impl Pins {
    /// Whether the focus motor's PWM reaches a live bridge now: the driver's output enable is
    /// set and every PWM pad is on its timer function. `None` without a servo model.
    pub fn focus_driving(&self) -> Option<bool> {
        let servo = self.focus_servo.as_ref()?;
        let on_timer = servo.pwm_pins.iter().all(|&(port, pin)| {
            let registers = &self.ports[port].registers;
            let field = if pin < 8 {
                registers[3] >> (pin * 2)
            } else {
                registers[4] >> ((pin - 8) * 2)
            };
            field & 3 != 0
        });
        Some(servo.enable & 1 != 0 && on_timer)
    }
    /// Advance a PWM-driven focus motor to `now_ns`; returns the home level once the servo has
    /// started (see `FocusServo::started`).
    pub fn advance_focus(&mut self, now_ns: u64) -> Option<bool> {
        let driving = self.focus_driving()?;
        let servo = self.focus_servo.as_mut()?;
        let elapsed = now_ns.saturating_sub(servo.at_ns);
        servo.at_ns = now_ns;
        let drive = if driving { servo.drive } else { 0 };
        let moved = i128::from(servo.remainder)
            + i128::from(drive) * i128::from(servo.gain) * i128::from(elapsed);
        let whole = (moved / 1_000_000_000) as i64;
        servo.remainder = (moved % 1_000_000_000) as i64;
        let (above, home, started) = (servo.home_above, servo.home, servo.started);
        self.focus_position += whole;
        if !started {
            return None;
        }
        let level = self.focus_position > above;
        self.set_input(home.0, home.1, level);
        Some(level)
    }
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
impl OutputLowHistory for PinBank {
    fn low_history(&self, pin: Pin) -> LowHistory {
        let pins = self.lock().unwrap();
        let port = &pins.ports[usize::from(pin.port)];
        let pin = usize::from(pin.pin);
        LowHistory {
            rising_edges: port.output_rising_edges[pin],
            last_low_ticks: port.output_last_low_ticks[pin],
        }
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
            focus_position: 0,
            focus_servo: None,
        }
    }
}
impl Pins {
    pub fn shared() -> SharedPins {
        Arc::new(PinBank(Mutex::new(Self::default())))
    }
    pub fn set_input(&mut self, port: usize, pin: usize, high: bool) {
        let bit = 1u16 << pin;
        self.ports[port].driven |= bit;
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
            PadModel::DataLatch | PadModel::DataLatchPins => {
                (p.registers[1] & 0xffff, p.registers[0])
            }
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
