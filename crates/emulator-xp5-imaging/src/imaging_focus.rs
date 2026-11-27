//! Functional SPI4 focus pulse-end and home sensor for virtual optics.
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;

#[derive(Serialize, Deserialize)]
struct Motion {
    generation: u64,
    armed: bool,
    position: i32,
    batches: VecDeque<u16>,
}
impl Default for Motion {
    fn default() -> Self {
        Self {
            generation: 0,
            armed: false,
            position: 8,
            batches: VecDeque::new(),
        }
    }
}
pub struct Focus {
    serial: crate::serial_sink::Sink,
    pins: fujimod_emulator_devices::xp5::pins::SharedPins,
    motion: Motion,
    /// The pad the firmware reads as its focus home sensor (GPIO table index 0x34).
    home: (usize, usize),
}
impl Focus {
    pub fn new(
        serial: crate::serial_sink::Sink,
        pins: fujimod_emulator_devices::xp5::pins::SharedPins,
    ) -> Self {
        Self {
            serial,
            pins,
            motion: Motion::default(),
            home: (0, 8),
        }
    }
    /// The home sensor pad of a body whose GPIO table puts index 0x34 elsewhere than 0.8.
    pub fn with_home(mut self, port: usize, pin: usize) -> Self {
        self.home = (port, pin);
        self
    }
    fn schedule(&mut self, cx: &mut Cx<'_>) {
        let word = *self
            .motion
            .batches
            .front()
            .expect("empty focus pulse queue");
        self.motion.armed = true;
        // Functional10ms floor keeps distinct pulse-end edges from coalescing
        // in the VM while its stock high-priority ISR/tasks drain each batch.
        cx.schedule(
            cx.now() + (10_000 + u64::from(word & 0x3ff) * 20) * 1000,
            self.motion.generation | (1 << 63),
        );
    }
}
impl Device for Focus {
    fn name(&self) -> &'static str {
        "virtual-audio-spi"
    }
    fn read(&mut self, cx: &mut Cx<'_>, offset: u64, size: u8) -> u64 {
        self.serial.read(cx, offset, size)
    }
    fn write(&mut self, cx: &mut Cx<'_>, offset: u64, size: u8, value: u64) {
        self.serial.write(cx, offset, size, value);
        let offset = offset.wrapping_sub(self.serial.base());
        let word = value as u16;
        // Driver register 0x1b (word 0xd000 | data, address mask 0xfe00): bit 0 enables the
        // bridge of a PWM-driven focus motor (pins::FocusServo::enable has the evidence).
        if offset == 8 && size == 4 && word & 0xfe00 == 0xd000 {
            let mut pins = self.pins.lock().unwrap();
            let enable = (word & 3) as u8;
            if pins
                .focus_servo
                .as_ref()
                .is_some_and(|servo| servo.enable != enable)
            {
                // The motor moved under the old enables up to now.
                pins.advance_focus(cx.now());
                if let Some(servo) = pins.focus_servo.as_mut() {
                    servo.enable = enable;
                }
                drop(pins);
                cx.log(&format!(
                    "focus driver enables {enable:02b} (register 0x1b word {word:04x})"
                ));
            }
        }
        if offset != 8 || size != 4 || word & 0xf000 != 0x2000 {
            return;
        }
        // 164779c fields b/c/d, table1ba1a52: enable11/direction10/count0..9.
        if word & 0x800 == 0 {
            self.motion.generation = self.motion.generation.wrapping_add(1);
            self.motion.armed = false;
            self.motion.batches.clear();
            return;
        }
        // Stock finite sequences queue an enabled zero after the last batch.
        // It ends new pulses without canceling the outstanding end interrupt.
        // Initial2c00 likewise has no finite batch to complete.
        if word & 0x3ff == 0 {
            return;
        }
        if self.motion.batches.len() >= 0x44 {
            cx.fail("focus pulse queue exceeds stock capacity");
            return;
        }
        self.motion.batches.push_back(word);
        if !self.motion.armed {
            self.motion.generation = self.motion.generation.wrapping_add(1);
            self.schedule(cx);
        }
        cx.log(&format!("virtual focus pulse batch {word:04x}"));
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token & (1 << 63) == 0 {
            self.serial.event(cx, token);
            return;
        }
        if !self.motion.armed || token & !(1 << 63) != self.motion.generation {
            return;
        }
        self.motion.armed = false;
        let word = self
            .motion
            .batches
            .pop_front()
            .expect("empty focus pulse queue");
        let steps = i32::from(word & 0x3ff);
        let mut pins = self.pins.lock().unwrap();
        pins.focus_pulses = pins.focus_pulses.wrapping_add(steps as u64);
        // A body whose focus motor is a PWM servo (pins.focus_servo, GFX100RF) steps something
        // else through this driver channel: the batch then moves neither the focus position
        // nor the home sensor, and only the pulse-end interrupt below is served.
        if pins.focus_servo.is_none() {
            self.motion.position += if word & 0x400 != 0 { -steps } else { steps };
            pins.focus_position += i64::from(if word & 0x400 != 0 { -steps } else { steps });
            // Input34/resource8 is GPIO0.8. Functional home threshold at0;
            // logical10/selectorB9 is GPIO1.9 -> IRQ127 ->16419c4(data0).
            let home = self.motion.position > 0;
            // X100VI: GPIO 0.8. Another body may name its pad with `home_input`.
            pins.set_input(self.home.0, self.home.1, home);
            // The two home interrupts follow the home pad's pin number: logical inputs 0xe and
            // 0xf use selectors 0xb7 and 0xb8, which are INTID 100 + pin and 116 + pin (X100VI
            // pad 0.8: 108 and 124). B7/B8 descriptors are level-triggered, unlike pulse-end B9.
            // Keep opposite home conditions asserted; stock unregisters each one-shot callback.
            let pin = self.home.1;
            pins.set_input(2, pin, home);
            pins.set_input(1, pin, !home);
            cx.set_irq(100 + pin as u32, home);
            cx.set_irq(116 + pin as u32, !home);
        }
        pins.set_input(1, 9, true);
        cx.set_irq(127, true);
        cx.set_irq(127, false);
        pins.set_input(1, 9, false);
        drop(pins);
        cx.log(&format!(
            "virtual focus batch {word:04x} complete IRQ127 position{}",
            self.motion.position
        ));
        if !self.motion.batches.is_empty() {
            self.schedule(cx);
        }
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Ok(Checkpoint::encode(self.name(), &self.motion)?
            .with_children(vec![self.serial.checkpoint()?]))
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let _: Motion = saved.decode(self.name())?;
        let [serial] = saved.children() else {
            return Err("missing serial motion state".into());
        };
        self.serial.validate_checkpoint(serial)
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        self.serial.restore_checkpoint(&saved.children()[0])?;
        self.motion = saved.decode(self.name())?;
        Ok(())
    }
    fn save(&self) -> Value {
        serde_json::json!({"serial": self.serial.save(), "focus": self.motion})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let motion = serde_json::from_value(value["focus"].clone())
            .map_err(|e| format!("invalid focus checkpoint: {e}"))?;
        self.serial.restore(&value["serial"])?;
        self.motion = motion;
        Ok(())
    }
}
