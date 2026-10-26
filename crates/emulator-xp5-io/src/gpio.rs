//! X-Processor 5 GPIO register/pin state; external levels are body inputs.
use fujimod_emulator_devices::parts::gpio::{Feedback, InputIrq};
use fujimod_emulator_devices::{Cx, Device};
use serde::Deserialize;
use serde_json::{json, Value};
use fujimod_emulator_devices::xp5::pins::{Pins, SharedPins};



#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialOutput {
    pub port: usize,
    pub levels: u16,
}

pub struct Gpio {
    pins: SharedPins,
    special: bool,
    previous: Vec<bool>,
    base: u64,
    input_irqs: Vec<InputIrq>,
    feedback: Vec<Feedback>,
}
impl Gpio {
    pub fn new(
        base: u64,
        pins: SharedPins,
        special: bool,
        input_irqs: Vec<InputIrq>,
    ) -> Result<Self, String> {
        for irq in &input_irqs {
            irq.validate(20)?;
        }
        // Processor interrupt inputs begin deasserted. An initially active
        // physical level is ready immediately, without waiting for an edge.
        let previous = input_irqs.iter().map(|irq| !irq.active_high).collect();
        Ok(Self {
            pins,
            base,
            special,
            input_irqs,
            previous,
            feedback: Vec::new(),
        })
    }
    pub fn with_feedback(mut self, feedback: Vec<Feedback>) -> Result<Self, String> {
        for connection in &feedback {
            connection.validate(20)?;
        }
        self.feedback = feedback;
        Ok(self)
    }
    /// Explicit register state at the selected boot handoff. Construction
    /// performs no guest write, pin transition, or interrupt delivery.
    pub fn with_initial_outputs(self, outputs: Vec<InitialOutput>) -> Result<Self, String> {
        if outputs.iter().enumerate().any(|(i, o)| {
            o.port >= 20 || outputs[..i].iter().any(|p| p.port == o.port)
        }) {
            return Err("invalid initial GPIO output ports".into());
        }
        {
            let mut pins = self.pins.lock().unwrap();
            for output in outputs {
                pins.ports[output.port].registers[0] = output.levels.into();
            }
        }
        Ok(self)
    }
    fn address(&self, offset: u64) -> Option<(usize, usize)> {
        if self.special {
            if offset <= 0x20 && offset & 3 == 0 {
                Some((3, offset as usize / 4))
            } else {
                None
            }
        } else {
            let p = offset / 0x40;
            let r = offset % 0x40;
            if p < 20 && p != 3 && r <= 0x20 && r & 3 == 0 {
                Some((p as usize, r as usize / 4))
            } else {
                None
            }
        }
    }
    fn route_inputs(&mut self, cx: &mut Cx<'_>) {
        let pins = self.pins.lock().unwrap();
        for (irq, previous) in self.input_irqs.iter().zip(&mut self.previous) {
            let current = pins.level(irq.port, irq.pin) != 0;
            if current != *previous {
                cx.set_irq(irq.line, current == irq.active_high);
                *previous = current;
            }
        }
    }
}
fn masked(old: u32, value: u32) -> u32 {
    let select = value >> 16;
    (old & !select) | (value & select)
}
impl Device for Gpio {
    fn name(&self) -> &'static str {
        if self.special {
            "gpio-port3"
        } else {
            "gpio-ports"
        }
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(self.base);
        if size != 4 {
            cx.unmodeled();
            return 0;
        }
        self.route_inputs(cx);
        let pins = self.pins.lock().unwrap();
        if !self.special && offset == 0x500 {
            return pins.global as u64;
        }
        if self.special && offset == 0x100 {
            return pins.special as u64;
        }
        if let Some((p, r)) = self.address(offset) {
            if r == 5 {
                pins.word(p) as u64
            } else {
                pins.ports[p].registers[r] as u64
            }
        } else {
            cx.unmodeled();
            0
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(self.base);
        if size != 4 {
            cx.unmodeled();
            return;
        }
        self.route_inputs(cx);
        let mut pins = self.pins.lock().unwrap();
        if !self.special && offset == 0x500 {
            pins.global = masked(pins.global as u32, value as u32) as u16;
            return;
        }
        if self.special && offset == 0x100 {
            pins.special = value as u32;
            return;
        }
        if let Some((p, r)) = self.address(offset) {
            if r == 5 {
                cx.unmodeled();
                return;
            }
            let old = pins.ports[p].registers[r];
            // Bits16..31 are write masks for 0/4/8/c/10/20, never pin
            // levels. +18/+1c are ordinary 2-bit fields read/modified by stock.
            pins.ports[p].registers[r] = if matches!(r, 6 | 7) {
                value as u32
            } else {
                masked(old, value as u32)
            };
            if r == 0 {
                let risen = !old & pins.ports[p].registers[0];
                for pin in 0..16 {
                    if risen & (1 << pin) != 0 {
                        pins.ports[p].output_rising_edges[pin] += 1;
                    }
                }
            }
            for connection in &self.feedback {
                if p == connection.port
                    && r == connection.register
                    && value as u32 & (1 << (connection.bit + 16)) != 0
                {
                    pins.set_input(
                        connection.input_port,
                        connection.input_pin,
                        value as u32 & (1 << connection.bit) != 0,
                    );
                }
            }
        } else {
            cx.unmodeled();
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _token: u64) {
        self.advance(cx);
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        self.route_inputs(cx);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        let pins = self.pins.lock().unwrap();
        self.input_irqs
            .iter()
            .zip(&self.previous)
            .any(|(irq, previous)| (pins.level(irq.port, irq.pin) != 0) != *previous)
            .then_some(now)
    }
    fn save(&self) -> Value {
        json!({"pins":*self.pins.lock().unwrap(), "previous":self.previous})
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        let pins: Pins = serde_json::from_value(v["pins"].clone())
            .map_err(|e| format!("GPIO checkpoint: {e}"))?;
        if pins.ports.len() != 20 {
            return Err("GPIO checkpoint needs 20 ports".into());
        }
        let previous: Vec<bool> = serde_json::from_value(v["previous"].clone())
            .map_err(|e| format!("GPIO input checkpoint: {e}"))?;
        if previous.len() != self.input_irqs.len() {
            return Err("GPIO interrupt wiring differs".into());
        }
        *self.pins.lock().unwrap() = pins;
        self.previous = previous;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn initial_output_latches_do_not_emit_edges_or_interrupts() {
        let pins = Pins::shared();
        let gpio = Gpio::new(0xff907000, pins.clone(), false, Vec::new()).unwrap()
            .with_initial_outputs(vec![InitialOutput { port: 12, levels: 0x100 }])
            .unwrap();
        assert_eq!(pins.lock().unwrap().output_level(12, 8), 1);
        assert_eq!(pins.lock().unwrap().output_level(12, 10), 0);
        assert_eq!(pins.lock().unwrap().output_rising_edges(12, 8), 0);
        let saved = gpio.save();
        assert_eq!(gpio.save(), saved);
        assert!(Gpio::new(0xff907000, pins, false, Vec::new()).unwrap()
            .with_initial_outputs(vec![InitialOutput {port:20,levels:0}])
            .is_err());
    }
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    #[test]
    fn external_shutter_levels_wake_declared_irq_routes_without_polling() {
        let pins = Pins::shared();
        let mut gpio = Gpio::new(
            0xff907000,
            pins.clone(),
            false,
            vec![
                InputIrq {
                    port: 0,
                    pin: 0,
                    line: 116,
                    active_high: false,
                },
                InputIrq {
                    port: 0,
                    pin: 1,
                    line: 117,
                    active_high: false,
                },
            ],
        )
        .unwrap();
        let mut host = RecordingHost::default(); // No firmware RAM.
        assert_eq!(gpio.next_deadline(10), None);
        pins.lock().unwrap().set_input(0, 0, false);
        assert_eq!(gpio.next_deadline(10), Some(10));
        gpio.event(&mut Cx::new(&mut host, 0, 0, 0), 0);
        assert_eq!(host.irqs, [(116, true)]);
        assert!(host.events.is_empty()); // No invented recurring deadline.
        assert_eq!(gpio.next_deadline(10), None);
        let snapshot = gpio.save();
        gpio.event(&mut Cx::new(&mut host, 0, 0, 0), 0);
        assert_eq!(gpio.save(), snapshot);
        assert_eq!(host.irqs.len(), 1);
        pins.lock().unwrap().set_input(0, 0, true);
        pins.lock().unwrap().set_input(0, 1, false);
        gpio.event(&mut Cx::new(&mut host, 0, 0, 0), 0);
        assert_eq!(host.irqs, [(116, true), (116, false), (117, true)]);
        gpio.restore(&snapshot).unwrap();
        assert_eq!(host.irqs.len(), 3); // Restoring does not drive CPU inputs.
    }
}

#[cfg(test)]
mod wiring_tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    #[test]
    fn masked_drive_fields_use_declared_feedback_and_count_output_edges() {
        let pins = Pins::shared();
        pins.lock().unwrap().set_input(4, 4, false);
        let mut gpio = Gpio::new(0xff907000, pins.clone(), false, Vec::new())
            .unwrap()
            .with_feedback(vec![Feedback {
                port: 4,
                register: 0,
                bit: 4,
                input_port: 4,
                input_pin: 4,
            }])
            .unwrap();
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        gpio.write(&mut cx, 0xff907100, 4, 1 << 4); // no write mask
        assert_eq!(pins.lock().unwrap().level(4, 4), 0);
        gpio.write(&mut cx, 0xff907100, 4, (1 << 20) | (1 << 4));
        assert_eq!(pins.lock().unwrap().level(4, 4), 1);
        assert_eq!(pins.lock().unwrap().output_rising_edges(4, 4), 1);
        gpio.write(&mut cx, 0xff907100, 4, (1 << 20) | (1 << 4));
        assert_eq!(pins.lock().unwrap().output_rising_edges(4, 4), 1);
        // An unrelated output pin has the same edge-counting law.
        gpio.write(&mut cx, 0xff907008, 4, (1 << 19) | (1 << 3));
        gpio.write(&mut cx, 0xff907000, 4, (1 << 19) | (1 << 3));
        assert_eq!(pins.lock().unwrap().output_rising_edges(0, 3), 1);
        assert!(cx.refusal().is_none());
    }
}
