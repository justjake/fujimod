//! The X-T20's shutter motor and its position sensors.
//!
//! imgdc (X-T20 2.11 section 6 0x17d3800..0x17d4600) runs a shutter
//! sequence by arming the external interrupt source of one sensor
//! (0xd09c3c: source 32 from 0x17d3b50, source 31 from 0x17d3b98; both on
//! external line 3, GIC 154, which the distributor configures edge
//! triggered), driving the motor (0x17fbc54(1, ..): GPIO bank 1 +0x0c holds
//! two mode bits per pin; pin 0 in mode 1, pin 1 in mode 0) and waiting up
//! to 100 ms for the interrupt; then it brakes (both pins in mode 0, their
//! outputs high) and releases. The sensors are GPIO inputs (source table
//! 0xf67220: 31 = bank 0 bit 3, 32 = bank 10 bit 2, 33 = bank 15 bit 1). At
//! the end of a sequence 0x17c46ec wants 31 and 32 high and 33 low; else
//! imgdc reports an error that re-initializes REC. The power-on inputs hold
//! 31 low and 32 high, so the boot's sequence (a wait on 32, then one on 31)
//! must leave both high.
//!
//! While exactly one of the two motor pins has a non-zero mode, the motor
//! turns: every step period it moves the sensors to the next declared
//! position and gives the line one edge. The positions and the step period
//! are inferred from what the firmware checks, not measured. The motor
//! modes are sampled once per poll period.
use crate::resets::{FIRST_LINE_IRQ, LINES};
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_ip::socionext::gpio::SharedPins;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where the motor driver and the sensors are.
#[derive(Clone, Debug)]
pub struct Wiring {
    /// GPIO bank and register of the motor pins' mode bits.
    pub mode: (usize, usize),
    /// The two motor pins.
    pub motor: (usize, usize),
    /// Sensor inputs as bank and pin.
    pub sensors: Vec<(usize, usize)>,
    /// External interrupt line the sensors' edges reach.
    pub line: usize,
}
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub step: u64,
    pub poll: u64,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    poll_at: u64,
    due: Option<u64>,
    steps: u64,
}
pub struct Shutter {
    wiring: Wiring,
    timing: Timing,
    /// Sensor levels, in the order the motor reaches them.
    positions: Vec<Vec<bool>>,
    pins: SharedPins,
    state: State,
}
impl Shutter {
    pub fn new(
        wiring: Wiring,
        timing: Timing,
        positions: Vec<Vec<bool>>,
        pins: SharedPins,
    ) -> Result<Self, String> {
        let pin = |(port, pin): (usize, usize)| port < 16 && pin < 16;
        let (motor_a, motor_b) = wiring.motor;
        if wiring.mode.0 >= 16
            || wiring.mode.1 >= 8
            || wiring.mode.1 == 5
            || motor_a >= 16
            || motor_b >= 16
            || motor_a == motor_b
            || wiring.sensors.is_empty()
            || !wiring.sensors.iter().all(|&s| pin(s))
            || wiring.line >= LINES
            || positions.len() < 2
            || positions.iter().any(|p| p.len() != wiring.sensors.len())
            || timing.step == 0
            || timing.poll == 0
        {
            return Err("Invalid shutter wiring, timing or positions".into());
        }
        Ok(Self {
            wiring,
            timing,
            positions,
            pins,
            state: State {
                poll_at: timing.poll,
                ..State::default()
            },
        })
    }
    fn turning(&self) -> bool {
        let (bank, register) = self.wiring.mode;
        let mode = self.pins.lock().unwrap().register(bank, register);
        let driven = |pin: usize| mode >> (2 * pin) & 3 != 0;
        driven(self.wiring.motor.0) != driven(self.wiring.motor.1)
    }
    fn step(&mut self, cx: &mut Cx<'_>) {
        let mut pins = self.pins.lock().unwrap();
        let levels: Vec<bool> = self
            .wiring
            .sensors
            .iter()
            .map(|&(bank, pin)| pins.input(bank, pin))
            .collect();
        let next = self
            .positions
            .iter()
            .position(|p| *p == levels)
            .map_or(0, |i| (i + 1) % self.positions.len());
        for (&(bank, pin), &high) in self.wiring.sensors.iter().zip(&self.positions[next]) {
            pins.set_input(bank, pin, high);
        }
        drop(pins);
        let irq = FIRST_LINE_IRQ + self.wiring.line as u32;
        cx.set_irq(irq, true);
        cx.set_irq(irq, false);
        self.state.steps += 1;
    }
}
impl Device for Shutter {
    fn name(&self) -> &'static str {
        "xpp-shutter"
    }
    fn read(&mut self, cx: &mut Cx<'_>, _address: u64, _size: u8) -> u64 {
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, _address: u64, _size: u8, _value: u64) {
        cx.unmodeled();
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let now = cx.now();
        if now >= self.state.poll_at {
            self.state.poll_at = (now / self.timing.poll + 1) * self.timing.poll;
            self.state.due = match (self.turning(), self.state.due) {
                (false, _) => None,
                (true, Some(at)) => Some(at),
                (true, None) => now.checked_add(self.timing.step),
            };
        }
        if let Some(at) = self.state.due.filter(|&at| now >= at) {
            self.step(cx);
            self.state.due = at.checked_add(self.timing.step);
        }
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        [Some(self.state.poll_at), self.state.due]
            .into_iter()
            .flatten()
            .min()
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state.poll_at % self.timing.poll != 0 {
            return Err("Invalid shutter checkpoint".into());
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, advance, write};
    use fujimod_emulator_ip::socionext::gpio::{Gpio, Pins};

    const GPIO: u64 = 0xfff0_c000;

    #[test]
    fn a_driven_motor_steps_the_sensors_through_their_positions() {
        let pins = Pins::new([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1 << 2, 0, 0, 0, 0, 0]);
        let mut gpio = Gpio::new(GPIO, pins.clone()).unwrap();
        let mut shutter = Shutter::new(
            Wiring {
                mode: (1, 3),
                motor: (0, 1),
                sensors: vec![(0, 3), (10, 2)],
                line: 3,
            },
            Timing { step: 30, poll: 1 },
            vec![vec![false, true], vec![false, false], vec![true, true]],
            pins.clone(),
        )
        .unwrap();
        let mut h = TestHost::new();
        let sensors = |pins: &SharedPins| {
            let pins = pins.lock().unwrap();
            (pins.input(0, 3), pins.input(10, 2))
        };
        // 0x17fbc54(1, ..): pin 0 in mode 1, pin 1 in mode 0.
        write(&mut gpio, &mut h, GPIO + 0x4c, 0x3_0001);
        write(&mut gpio, &mut h, GPIO + 0x4c, 0xc_0000);
        advance(&mut shutter, &mut h, 1);
        assert_eq!(shutter.next_deadline(1), Some(2));
        advance(&mut shutter, &mut h, 30);
        assert_eq!(sensors(&pins), (false, true));
        advance(&mut shutter, &mut h, 31);
        assert_eq!((sensors(&pins), shutter.state.steps), ((false, false), 1));
        let saved = shutter.save();
        // Braking puts both pins in mode 0: the motor stops.
        write(&mut gpio, &mut h, GPIO + 0x4c, 0x3_0000);
        advance(&mut shutter, &mut h, 40);
        assert_eq!(shutter.state.due, None);
        advance(&mut shutter, &mut h, 100);
        assert_eq!(shutter.state.steps, 1);
        shutter.restore(&saved).unwrap();
        assert_eq!(shutter.state.due, Some(61));
        write(&mut gpio, &mut h, GPIO + 0x4c, 0x3_0001);
        advance(&mut shutter, &mut h, 61);
        assert_eq!((sensors(&pins), shutter.state.steps), ((true, true), 2));
    }
}
