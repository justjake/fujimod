//! X-Processor 5 GPIO register/pin state; external levels are body inputs.
use fujimod_emulator_devices::bus::InterruptLine;
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::parts::gpio::{Feedback, InputIrq};
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use fujimod_emulator_devices::xp5::pins::{Pins, SharedPins};



#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialOutput {
    pub port: usize,
    pub levels: u16,
}

/// One port register's value at the boot handoff (output enables, function selects), for a
/// start that enters after the stage that programmed it.
#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialRegister {
    pub port: usize,
    pub register: usize,
    pub value: u32,
}

/// One external pad level at the boot handoff, for a pin whose level the board
/// or the run declares. Undeclared pads keep the pin bank's default.
#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialInput {
    pub port: usize,
    pub pin: usize,
    pub high: bool,
}

pub struct Gpio {
    pins: SharedPins,
    special: bool,
    previous: Vec<bool>,
    base: u64,
    input_irqs: Vec<InputIrq>,
    /// Where each input route drives: the shared controller or a core.
    targets: Vec<InterruptLine>,
    feedback: Vec<Feedback>,
    /// Due tick of each feedback wire's pending rise.
    rising: Vec<Option<u64>>,
    /// Latching wires whose pad has risen and now stays high.
    held: Vec<bool>,
}
#[derive(Serialize)]
struct SavedRef<'a> {
    pins: &'a Pins,
    previous: &'a [bool],
    input_irqs: &'a [InputIrq],
    rising: &'a [Option<u64>],
    held: &'a [bool],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    pins: Pins,
    previous: Vec<bool>,
    input_irqs: Vec<InputIrq>,
    rising: Vec<Option<u64>>,
    held: Vec<bool>,
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
        let targets = input_irqs.iter().map(|irq| InterruptLine::Shared(irq.line)).collect();
        Ok(Self {
            pins,
            base,
            special,
            input_irqs,
            targets,
            previous,
            feedback: Vec::new(),
            rising: Vec::new(),
            held: Vec::new(),
        })
    }
    /// Pads wired to a core's own interrupt inputs (e.g. the companion's NVIC).
    pub fn with_core_input_irqs(mut self, core: usize, irqs: Vec<InputIrq>) -> Result<Self, String> {
        for irq in irqs {
            irq.validate(20)?;
            self.previous.push(!irq.active_high);
            self.targets.push(InterruptLine::Core { core, line: irq.line });
            self.input_irqs.push(irq);
        }
        Ok(self)
    }
    /// Wires from drive fields to input pads. A wire holds its pad at the
    /// drive bit's level from construction on, so apply it after the initial
    /// outputs and inputs; a fed pad cannot also declare an initial input.
    pub fn with_feedback(mut self, feedback: Vec<Feedback>) -> Result<Self, String> {
        for connection in &feedback {
            connection.validate(20)?;
        }
        {
            let mut pins = self.pins.lock().unwrap();
            for connection in &feedback {
                let level = pins.ports[connection.port].registers[connection.register]
                    & (1 << connection.bit)
                    != 0;
                pins.set_input(connection.input_port, connection.input_pin, level);
            }
        }
        self.rising = vec![None; feedback.len()];
        self.held = vec![false; feedback.len()];
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
    /// Explicit port register values at the selected boot handoff. Construction performs no
    /// guest write, pin transition, or interrupt delivery. Apply before the feedback wires,
    /// which sample their drive registers at construction.
    pub fn with_initial_registers(self, registers: Vec<InitialRegister>) -> Result<Self, String> {
        {
            let mut pins = self.pins.lock().unwrap();
            for (i, r) in registers.iter().enumerate() {
                if r.port >= pins.ports.len()
                    || r.register >= pins.ports[r.port].registers.len()
                    || registers[..i].iter().any(|p| (p.port, p.register) == (r.port, r.register))
                {
                    return Err(format!(
                        "invalid initial GPIO register: port {} register {}",
                        r.port, r.register
                    ));
                }
                pins.ports[r.port].registers[r.register] = r.value;
            }
        }
        Ok(self)
    }
    /// External pad levels at the selected boot handoff. Construction performs
    /// no guest access; an input route whose level is active sees it as an
    /// initially active level, as for the default pads.
    pub fn with_initial_inputs(self, inputs: Vec<InitialInput>) -> Result<Self, String> {
        if inputs.iter().enumerate().any(|(i, input)| {
            input.port >= 20
                || input.pin >= 16
                || inputs[..i]
                    .iter()
                    .any(|other| (other.port, other.pin) == (input.port, input.pin))
        }) {
            return Err("invalid initial GPIO input pins".into());
        }
        {
            let mut pins = self.pins.lock().unwrap();
            for input in inputs {
                pins.set_input(input.port, input.pin, input.high);
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
        for ((irq, target), previous) in
            self.input_irqs.iter().zip(&self.targets).zip(&mut self.previous)
        {
            let current = pins.level(irq.port, irq.pin) != 0;
            if current != *previous {
                if let Err(error) = target.drive(cx, current == irq.active_high) {
                    cx.fail(&error);
                }
                *previous = current;
            }
        }
    }
    fn complete_rises(&mut self, now: u64) {
        let mut pins = self.pins.lock().unwrap();
        for ((wire, rising), held) in self.feedback.iter().zip(&mut self.rising).zip(&mut self.held) {
            if rising.is_some_and(|due| due <= now) {
                pins.set_input(wire.input_port, wire.input_pin, true);
                *rising = None;
                *held = wire.latch_high;
            }
        }
    }
    fn validate_saved(&self, saved: &Saved) -> Result<(), String> {
        let pins = self.pins.lock().unwrap();
        if saved.pins.ports.len() != 20 {
            return Err("GPIO checkpoint needs 20 ports".into());
        }
        if saved.pins.pad_model != pins.pad_model {
            return Err("GPIO checkpoint pad model differs".into());
        }
        if saved.previous.len() != self.input_irqs.len()
            || saved.input_irqs.len() != self.input_irqs.len()
            || saved.input_irqs.iter().zip(&self.input_irqs).any(|(a, b)| {
                a.port != b.port
                    || a.pin != b.pin
                    || a.line != b.line
                    || a.active_high != b.active_high
            })
        {
            return Err("GPIO interrupt wiring differs".into());
        }
        if saved.rising.len() != self.feedback.len() || saved.held.len() != self.feedback.len() {
            return Err("GPIO feedback wiring differs".into());
        }
        Ok(())
    }
}
fn masked(old: u32, value: u32) -> u32 {
    let select = value >> 16;
    (old & !select) | (value & select)
}
impl fujimod_emulator_devices::capability::GpioInputs for Gpio {
    fn admit_inputs(
        &self,
        changes: &[(fujimod_emulator_devices::parts::pins::Pin, bool)],
    ) -> Result<(), String> {
        for (pin, _) in changes {
            if pin.port >= 20 || pin.pin >= 16 {
                return Err("XP5 GPIO input is outside the declared pin range".into());
            }
            let fed = self.feedback.iter().any(|wire| {
                (wire.input_port, wire.input_pin) == (usize::from(pin.port), usize::from(pin.pin))
            });
            if fed {
                return Err("XP5 GPIO input pad is driven by a board feedback wire".into());
            }
        }
        Ok(())
    }
    fn request_inputs(
        &mut self,
        changes: &[(fujimod_emulator_devices::parts::pins::Pin, bool)],
    ) -> Result<(), String> {
        self.admit_inputs(changes)?;
        let mut pins = self.pins.lock().map_err(|_| "XP5 GPIO pins poisoned")?;
        for (pin, high) in changes {
            pins.set_input(usize::from(pin.port), usize::from(pin.pin), *high);
        }
        Ok(())
    }
}
impl Device for Gpio {
    fn gpio_inputs(
        &self,
    ) -> Option<&(dyn fujimod_emulator_devices::capability::GpioInputs + 'static)> {
        Some(self)
    }
    fn gpio_inputs_mut(
        &mut self,
    ) -> Option<&mut (dyn fujimod_emulator_devices::capability::GpioInputs + 'static)> {
        Some(self)
    }
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
            if r == 5 && !self.special {
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
            if r == 5 && !self.special {
                cx.unmodeled();
                return;
            }
            let old = pins.ports[p].registers[r];
            // Bits16..31 are write masks for 0/4/8/c/10/14/20, never pin
            // levels. +18/+1c are ordinary 2-bit fields read/modified by stock.
            pins.ports[p].registers[r] = if matches!(r, 6 | 7) {
                value as u32
            } else {
                masked(old, value as u32)
            };
            if r == 0 {
                let risen = !old & pins.ports[p].registers[0];
                let fallen = old & !pins.ports[p].registers[0];
                let now = cx.now();
                let port = &mut pins.ports[p];
                for (pin, ((edges, low_since), last_low)) in port
                    .output_rising_edges
                    .iter_mut()
                    .zip(&mut port.output_low_since)
                    .zip(&mut port.output_last_low_ticks)
                    .enumerate()
                {
                    if risen & (1 << pin) != 0 {
                        *edges += 1;
                        if let Some(since) = low_since.take() {
                            *last_low = now.saturating_sub(since);
                        }
                    }
                    if fallen & (1 << pin) != 0 {
                        *low_since = Some(now);
                    }
                }
            }
            for ((connection, rising), held) in
                self.feedback.iter().zip(&mut self.rising).zip(&self.held)
            {
                if *held {
                    continue;
                }
                if p == connection.port
                    && r == connection.register
                    && value as u32 & (1 << (connection.bit + 16)) != 0
                {
                    let high = value as u32 & (1 << connection.bit) != 0;
                    let pad = pins.level(connection.input_port, connection.input_pin) != 0;
                    if high && connection.rise_delay > 0 {
                        if !pad && rising.is_none() {
                            let due = cx.now() + connection.rise_delay;
                            *rising = Some(due);
                            cx.schedule(due, 0);
                        }
                    } else {
                        *rising = None;
                        pins.set_input(connection.input_port, connection.input_pin, high);
                    }
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
        self.complete_rises(cx.now());
        self.route_inputs(cx);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        let pins = self.pins.lock().unwrap();
        let changed = self
            .input_irqs
            .iter()
            .zip(&self.previous)
            .any(|(irq, previous)| (pins.level(irq.port, irq.pin) != 0) != *previous);
        let rise = self.rising.iter().flatten().min().copied();
        if changed {
            Some(now)
        } else {
            rise.map(|due| due.max(now))
        }
    }
    fn save(&self) -> Value {
        json!({"pins":*self.pins.lock().unwrap(), "previous":self.previous, "rising":self.rising, "held":self.held})
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
        let rising: Vec<Option<u64>> = serde_json::from_value(v["rising"].clone())
            .map_err(|e| format!("GPIO feedback checkpoint: {e}"))?;
        let held: Vec<bool> = serde_json::from_value(v["held"].clone())
            .map_err(|e| format!("GPIO feedback checkpoint: {e}"))?;
        if rising.len() != self.feedback.len() || held.len() != self.feedback.len() {
            return Err("GPIO feedback wiring differs".into());
        }
        *self.pins.lock().unwrap() = pins;
        self.previous = previous;
        self.rising = rising;
        self.held = held;
        Ok(())
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        let pins = self.pins.lock().unwrap();
        Checkpoint::encode(
            self.name(),
            &SavedRef {
                pins: &pins,
                previous: &self.previous,
                input_irqs: &self.input_irqs,
                rising: &self.rising,
                held: &self.held,
            },
        )
    }
    fn validate_checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), String> {
        self.validate_saved(&checkpoint.decode(self.name())?)
    }
    fn restore_checkpoint(&mut self, checkpoint: &Checkpoint) -> Result<(), String> {
        let saved: Saved = checkpoint.decode(self.name())?;
        self.validate_saved(&saved)?;
        *self.pins.lock().unwrap() = saved.pins;
        self.previous = saved.previous;
        self.rising = saved.rising;
        self.held = saved.held;
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(json!({"pins":*self.pins.lock().unwrap(), "previous":self.previous, "rising":self.rising, "held":self.held}))
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
    fn the_pin_control_bank_accepts_its_sixth_register() {
        let pins = Pins::shared();
        let mut control = Gpio::new(0xff90b000, pins.clone(), true, Vec::new()).unwrap();
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        for (offset, value) in [(0u64, 0x0002_0002u64), (4, 0x000f_000f), (0x14, 0x3ff2_3ff2)] {
            control.write(&mut cx, 0xff90b000 + offset, 4, value);
            assert_eq!(cx.refusal(), None, "{offset:#x}");
        }
        assert_eq!(pins.lock().unwrap().ports[3].registers[5], 0x3ff2);
        // The bank has no pad word: +0x14 reads back what was written.
        assert_eq!(control.read(&mut cx, 0xff90b000 + 0x14, 4), 0x3ff2);
        // The bank's data ports stay input-only: an ordinary port refuses the same store.
        let mut ports = Gpio::new(0xff907000, pins, false, Vec::new()).unwrap();
        ports.write(&mut cx, 0xff907000 + 0x14, 4, 1);
        assert!(cx.refusal().is_some());
    }
    #[test]
    fn exact_checkpoint_restores_pins_edges_and_input_history() {
        let pins = Pins::shared();
        let mut gpio = Gpio::new(
            0xff907000,
            pins.clone(),
            false,
            vec![InputIrq {
                port: 0,
                pin: 0,
                line: 116,
                active_high: false,
            }],
        )
        .unwrap()
        .with_initial_outputs(vec![InitialOutput {
            port: 12,
            levels: 0x100,
        }])
        .unwrap();
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        gpio.write(&mut cx, 0xff907000 + 12 * 0x40, 4, (1 << 19) | (1 << 3));
        pins.lock().unwrap().set_input(0, 0, false);
        gpio.event(&mut cx, 0);
        let saved = gpio.save();
        let checkpoint = gpio.checkpoint().unwrap();
        gpio.validate_checkpoint(&checkpoint).unwrap();
        pins.lock().unwrap().ports[12].registers[0] = 0;
        pins.lock().unwrap().set_input(0, 0, true);
        gpio.event(&mut cx, 0);
        gpio.restore_checkpoint(&checkpoint).unwrap();
        assert_eq!(gpio.save(), saved);
        let other = Gpio::new(
            0xff907000,
            pins,
            false,
            vec![InputIrq {
                port: 0,
                pin: 1,
                line: 117,
                active_high: false,
            }],
        )
        .unwrap();
        assert!(other.validate_checkpoint(&checkpoint).is_err());
    }
    #[test]
    fn initial_input_levels_set_only_the_declared_pads() {
        let pins = Pins::shared();
        let mut gpio = Gpio::new(0xff907000, pins.clone(), false, Vec::new())
            .unwrap()
            .with_initial_inputs(vec![InitialInput { port: 11, pin: 4, high: false }])
            .unwrap();
        assert_eq!(pins.lock().unwrap().level(11, 4), 0);
        assert_eq!(pins.lock().unwrap().level(11, 3), 1);
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        assert_eq!(gpio.read(&mut cx, 0xff907000 + 11 * 0x40 + 0x14, 4), 0xffef);
        for invalid in [
            vec![InitialInput { port: 20, pin: 0, high: true }],
            vec![InitialInput { port: 1, pin: 16, high: true }],
            vec![
                InitialInput { port: 1, pin: 2, high: true },
                InitialInput { port: 1, pin: 2, high: false },
            ],
        ] {
            assert!(Gpio::new(0xff907000, Pins::shared(), false, Vec::new())
                .unwrap()
                .with_initial_inputs(invalid)
                .is_err());
        }
    }
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
                rise_delay: 0,
                latch_high: false,
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

    #[test]
    fn a_fed_pad_starts_at_its_drive_bit_level() {
        let wire = Feedback { port: 4, register: 0, bit: 4, input_port: 4, input_pin: 4, rise_delay: 0, latch_high: false };
        // Undeclared pads read high; the wire pulls a low drive bit's pad low.
        let pins = Pins::shared();
        assert_eq!(pins.lock().unwrap().level(4, 4), 1);
        Gpio::new(0xff907000, pins.clone(), false, Vec::new())
            .unwrap()
            .with_feedback(vec![wire])
            .unwrap();
        assert_eq!(pins.lock().unwrap().level(4, 4), 0);
        // A high initial drive bit holds the pad high.
        let pins = Pins::shared();
        pins.lock().unwrap().set_input(4, 4, false);
        Gpio::new(0xff907000, pins.clone(), false, Vec::new())
            .unwrap()
            .with_initial_outputs(vec![InitialOutput { port: 4, levels: 1 << 4 }])
            .unwrap()
            .with_feedback(vec![wire])
            .unwrap();
        assert_eq!(pins.lock().unwrap().level(4, 4), 1);
    }

    #[test]
    fn a_delayed_wire_rises_after_its_delay_and_edges_a_core_irq() {
        let wire = Feedback { port: 1, register: 0, bit: 14, input_port: 0, input_pin: 4, rise_delay: 1000, latch_high: false };
        let pins = Pins::shared();
        let route = InputIrq { port: 0, pin: 4, line: 27, active_high: true };
        let mut gpio = Gpio::new(0xff907000, pins.clone(), false, Vec::new())
            .unwrap()
            .with_core_input_irqs(1, vec![route])
            .unwrap()
            .with_feedback(vec![wire])
            .unwrap();
        let mut host = RecordingHost::default();
        host.now = 100;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        gpio.advance(&mut cx);
        assert_eq!(pins.lock().unwrap().level(0, 4), 0);
        gpio.write(&mut cx, 0xff907040, 4, (1 << 30) | (1 << 14));
        assert_eq!(pins.lock().unwrap().level(0, 4), 0);
        assert_eq!(gpio.next_deadline(100), Some(1100));
        drop(cx);
        assert_eq!(host.events, [(0, 1100, 0)]);
        host.now = 1100;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        gpio.event(&mut cx, 0);
        assert_eq!(pins.lock().unwrap().level(0, 4), 1);
        // A fall is immediate and cancels nothing pending.
        gpio.write(&mut cx, 0xff907040, 4, 1 << 30);
        gpio.advance(&mut cx);
        assert_eq!(pins.lock().unwrap().level(0, 4), 0);
        // A rise canceled by a fall before its due tick never reaches the pad.
        gpio.write(&mut cx, 0xff907040, 4, (1 << 30) | (1 << 14));
        gpio.write(&mut cx, 0xff907040, 4, 1 << 30);
        assert!(cx.refusal().is_none());
        drop(cx);
        host.now = 5000;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        gpio.event(&mut cx, 0);
        assert_eq!(pins.lock().unwrap().level(0, 4), 0);
        drop(cx);
        assert_eq!(host.core_irqs, [(1, 27, true), (1, 27, false)]);
        assert!(host.irqs.is_empty());
    }

    #[test]
    fn a_latched_wire_holds_its_pad_high_after_the_first_rise() {
        let wire = Feedback {
            port: 1, register: 0, bit: 14, input_port: 0, input_pin: 4,
            rise_delay: 1000, latch_high: true,
        };
        let pins = Pins::shared();
        let mut gpio = Gpio::new(0xff907000, pins.clone(), false, Vec::new())
            .unwrap()
            .with_feedback(vec![wire])
            .unwrap();
        let mut host = RecordingHost::default();
        host.now = 100;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        // A fall before the rise lands cancels it, as for an unlatched wire.
        gpio.write(&mut cx, 0xff907040, 4, (1 << 30) | (1 << 14));
        gpio.write(&mut cx, 0xff907040, 4, 1 << 30);
        drop(cx);
        host.now = 2000;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        gpio.event(&mut cx, 0);
        assert_eq!(pins.lock().unwrap().level(0, 4), 0);
        // The first completed rise latches the pad high.
        gpio.write(&mut cx, 0xff907040, 4, (1 << 30) | (1 << 14));
        drop(cx);
        host.now = 3000;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        gpio.event(&mut cx, 0);
        assert_eq!(pins.lock().unwrap().level(0, 4), 1);
        // Later drive falls and rises leave it high, through a checkpoint.
        gpio.write(&mut cx, 0xff907040, 4, 1 << 30);
        gpio.advance(&mut cx);
        assert_eq!(pins.lock().unwrap().level(0, 4), 1);
        let saved = gpio.checkpoint().unwrap();
        let mut copy = Gpio::new(0xff907000, Pins::shared(), false, Vec::new())
            .unwrap()
            .with_feedback(vec![wire])
            .unwrap();
        copy.restore_checkpoint(&saved).unwrap();
        assert_eq!(copy.held, [true]);
        assert!(cx.refusal().is_none());
    }

    #[test]
    fn output_latch_records_the_length_of_each_completed_low_interval() {
        use fujimod_emulator_devices::parts::pins::{LowHistory, OutputLowHistory, Pin};
        let pins = Pins::shared();
        let mut gpio = Gpio::new(0xff907000, pins.clone(), false, Vec::new()).unwrap();
        let pin = Pin { port: 13, pin: 6 };
        let set = |gpio: &mut Gpio, now: u64, high: bool| {
            let mut host = RecordingHost {
                now,
                ..RecordingHost::default()
            };
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            gpio.write(
                &mut cx,
                0xff907340,
                4,
                (1u64 << 22) | (u64::from(high) << 6),
            );
            assert!(cx.refusal().is_none());
        };
        set(&mut gpio, 10, true);
        set(&mut gpio, 100, false);
        assert_eq!(pins.low_history(pin).last_low_ticks, 0);
        set(&mut gpio, 700, true);
        assert_eq!(
            pins.low_history(pin),
            LowHistory {
                rising_edges: 2,
                last_low_ticks: 600
            }
        );
    }

    #[test]
    fn port3_register_five_retains_masked_writes_through_a_checkpoint() {
        let base = 0xff90b000;
        let pins = Pins::shared();
        let mut gpio = Gpio::new(base, pins.clone(), true, Vec::new()).unwrap();
        let mut host = RecordingHost::default();

        for value in [0x0005_0001, 0x000a_0008] {
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            gpio.write(&mut cx, base + 0x14, 4, value);
            assert!(cx.refusal().is_none());
        }
        assert_eq!(pins.lock().unwrap().ports[3].registers[5], 9);

        let mut cx = Cx::new(&mut host, 0, 0, 0);
        assert_eq!(gpio.read(&mut cx, base + 0x14, 4), 9);
        assert!(cx.refusal().is_none());

        let saved = gpio.save();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        gpio.write(&mut cx, base + 0x14, 2, 0x0001);
        assert!(cx.is_unmodeled());
        assert_eq!(pins.lock().unwrap().ports[3].registers[5], 9);

        let restored_pins = Pins::shared();
        let mut restored = Gpio::new(base, restored_pins.clone(), true, Vec::new()).unwrap();
        restored.restore(&saved).unwrap();
        assert_eq!(restored_pins.lock().unwrap().ports[3].registers[5], 9);
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        assert_eq!(restored.read(&mut cx, base + 0x14, 4), 9);
        assert!(cx.refusal().is_none());

        let mut ordinary = Gpio::new(0xff907000, Pins::shared(), false, Vec::new()).unwrap();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        ordinary.write(&mut cx, 0xff907014, 4, 0x0001_0001);
        assert!(cx.is_unmodeled());
    }
}
