//! Sixteen masked GPIO banks. Register base and external levels are explicit.
//!
//! A pad may also be wired to a level-sensitive processor interrupt input:
//! the line is held asserted while the pad's external level equals the
//! route's active level. The interrupt controller latches and clears it.
use fujimod_emulator_devices::parts::gpio::InputIrq;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};
#[derive(Clone, Serialize, Deserialize)]
pub struct Pins {
    regs: [[u32; 8]; 16],
    inputs: [u16; 16],
    changes: [[u64; 16]; 16],
}
pub type SharedPins = Arc<Mutex<Pins>>;
impl Pins {
    pub fn new(inputs: [u16; 16]) -> SharedPins {
        Arc::new(Mutex::new(Self {
            regs: [[0; 8]; 16],
            inputs,
            changes: [[0; 16]; 16],
        }))
    }
    /// Register words of `port`, indexed by offset / 4.
    pub fn registers(&self, port: usize) -> [u32; 8] {
        self.regs[port]
    }
    pub fn output(&self, port: usize, pin: usize) -> (bool, u64) {
        (
            self.regs[port][0] & (1 << pin) != 0,
            self.changes[port][pin],
        )
    }
    /// Input level of a pin.
    pub fn input(&self, port: usize, pin: usize) -> bool {
        self.inputs[port] & (1 << pin) != 0
    }
    /// Value of register `register` of bank `port`.
    pub fn register(&self, port: usize, register: usize) -> u32 {
        self.regs[port][register]
    }
    pub fn set_input(&mut self, port: usize, pin: usize, high: bool) {
        if high {
            self.inputs[port] |= 1 << pin;
        } else {
            self.inputs[port] &= !(1 << pin);
        }
    }
}
pub struct Gpio {
    base: u64,
    pins: SharedPins,
    interrupts: Vec<InputIrq>,
    /// Pad level last driven onto each interrupt route.
    previous: Vec<bool>,
}
impl Gpio {
    pub fn new(base: u64, pins: SharedPins) -> Result<Self, String> {
        if base & 0xfff != 0 || base.checked_add(0x400).is_none() {
            return Err("Invalid GPIO bank base".into());
        }
        Ok(Self {
            base,
            pins,
            interrupts: Vec::new(),
            previous: Vec::new(),
        })
    }
    /// Pads wired to level-sensitive interrupt inputs. The inputs begin
    /// deasserted; a pad already at its active level asserts its line at the
    /// next device scan.
    pub fn with_interrupts(mut self, interrupts: Vec<InputIrq>) -> Result<Self, String> {
        for (i, irq) in interrupts.iter().enumerate() {
            irq.validate(16)?;
            if interrupts[..i]
                .iter()
                .any(|other| (other.port, other.pin) == (irq.port, irq.pin) || other.line == irq.line)
            {
                return Err("Duplicate GPIO interrupt route".into());
            }
        }
        self.previous = interrupts.iter().map(|irq| !irq.active_high).collect();
        self.interrupts = interrupts;
        Ok(self)
    }
    fn route_interrupts(&mut self, cx: &mut Cx<'_>) {
        let pins = self.pins.lock().unwrap();
        for (irq, previous) in self.interrupts.iter().zip(&mut self.previous) {
            let level = pins.input(irq.port, irq.pin);
            if level != *previous {
                cx.set_irq(irq.line, level == irq.active_high);
                *previous = level;
            }
        }
    }
    /// An external level on input `pin` of bank `port`.
    pub fn set_input(&mut self, port: usize, pin: usize, high: bool) -> Result<(), String> {
        if port >= 16 || pin >= 16 {
            return Err("Invalid GPIO input pin".into());
        }
        self.pins.lock().unwrap().set_input(port, pin, high);
        Ok(())
    }
}
impl Device for Gpio {
    fn name(&self) -> &'static str {
        "masked-gpio"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(self.base) as usize;
        let p = offset / 64;
        let r = offset % 64 / 4;
        if size != 4 || offset & 3 != 0 || p >= 16 || r >= 8 {
            cx.unmodeled();
            return 0;
        }
        self.route_interrupts(cx);
        let pins = self.pins.lock().unwrap();
        if r == 5 {
            u64::from(pins.inputs[p])
        } else {
            u64::from(pins.regs[p][r])
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(self.base) as usize;
        let p = offset / 64;
        let r = offset % 64 / 4;
        if size != 4 || offset & 3 != 0 || p >= 16 || r >= 8 || r == 5 {
            cx.unmodeled();
            return;
        }
        self.route_interrupts(cx);
        let mut pins = self.pins.lock().unwrap();
        let old = pins.regs[p][r];
        let v = value as u32;
        pins.regs[p][r] = if r >= 6 {
            v
        } else {
            let mask = v >> 16;
            (old & !mask) | (v & mask)
        };
        if r == 0 {
            for bit in 0..16 {
                if (old ^ pins.regs[p][r]) & (1 << bit) != 0 {
                    pins.changes[p][bit] += 1;
                }
            }
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _token: u64) {
        self.advance(cx);
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        self.route_interrupts(cx);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        let pins = self.pins.lock().unwrap();
        self.interrupts
            .iter()
            .zip(&self.previous)
            .any(|(irq, previous)| pins.input(irq.port, irq.pin) != *previous)
            .then_some(now)
    }
    fn save(&self) -> Value {
        let pins = serde_json::to_value(&*self.pins.lock().unwrap()).unwrap();
        if self.interrupts.is_empty() {
            pins
        } else {
            serde_json::json!({"pins": pins, "previous": self.previous})
        }
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(
                self.base,
                &*self.pins.lock().map_err(|_| "GPIO pins poisoned")?,
                &self.previous,
            ),
        )
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let (base, _, previous): (u64, Pins, Vec<bool>) = saved.decode(self.name())?;
        if base != self.base {
            return Err("GPIO checkpoint base differs".into());
        }
        if previous.len() != self.interrupts.len() {
            return Err("GPIO interrupt wiring differs".into());
        }
        Ok(())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (_, pins, previous): (u64, Pins, Vec<bool>) = saved.decode(self.name())?;
        *self.pins.lock().map_err(|_| "GPIO pins poisoned")? = pins;
        self.previous = previous;
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.save())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, read, write};
    #[test]
    fn relocated_masks_input_status_and_checkpoints_preserve_shared_peer_handles() {
        for base in [0x10000, 0xfff0c000, 0xfff41000] {
            let pins = Pins::new([0x55aa; 16]);
            let mut d = Gpio::new(base, pins.clone()).unwrap();
            let mut h = TestHost::new();
            write(&mut d, &mut h, base + 2 * 64, 0x00080008);
            write(&mut d, &mut h, base + 2 * 64, 0x00020002);
            assert_eq!(read(&mut d, &mut h, base + 2 * 64), 10);
            assert_eq!(pins.lock().unwrap().output(2, 3), (true, 1));
            write(&mut d, &mut h, base + 2 * 64 + 4, 0xffffffff);
            write(&mut d, &mut h, base + 2 * 64 + 8, 0xffffffff);
            assert_eq!(read(&mut d, &mut h, base + 2 * 64 + 0x14), 0x55aa);
            let saved = d.checkpoint().unwrap();
            pins.lock().unwrap().set_input(2, 1, false);
            write(&mut d, &mut h, base + 2 * 64, 0x00080000);
            d.restore_checkpoint(&saved).unwrap();
            assert_eq!(pins.lock().unwrap().output(2, 3), (true, 1));
            assert_eq!(read(&mut d, &mut h, base + 2 * 64 + 0x14), 0x55aa);
            let mut cx = h.cx();
            d.write(&mut cx, base + 2 * 64 + 0x14, 4, 0);
            assert!(cx.refusal().is_some());
        }
    }

    #[test]
    fn an_active_low_pad_holds_its_level_interrupt_while_low() {
        use fujimod_emulator_devices::test_host::recording::RecordingHost;
        let route = |pin, line| InputIrq {
            port: 0,
            pin,
            line,
            active_high: false,
        };
        let pins = Pins::new([0b11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let mut d = Gpio::new(0xfff41000, pins.clone())
            .unwrap()
            .with_interrupts(vec![route(0, 167), route(1, 168)])
            .unwrap();
        let mut h = RecordingHost::default();
        // Released pads read high: nothing is due and nothing is driven.
        assert_eq!(d.next_deadline(5), None);
        d.advance(&mut Cx::new(&mut h, 0, 0, 0));
        assert!(h.irqs.is_empty());
        // Pressing pin 0 asserts line 167 at the next scan, once.
        d.set_input(0, 0, false).unwrap();
        assert_eq!(d.next_deadline(7), Some(7));
        d.advance(&mut Cx::new(&mut h, 0, 0, 0));
        assert_eq!(h.irqs, [(167, true)]);
        assert_eq!(d.next_deadline(8), None);
        d.advance(&mut Cx::new(&mut h, 0, 0, 0));
        assert_eq!(h.irqs.len(), 1);
        // The pressed level survives a checkpoint round trip.
        let saved = d.checkpoint().unwrap();
        d.set_input(0, 0, true).unwrap();
        d.restore_checkpoint(&saved).unwrap();
        assert_eq!(d.next_deadline(9), None);
        // Releasing deasserts it.
        d.set_input(0, 0, true).unwrap();
        d.advance(&mut Cx::new(&mut h, 0, 0, 0));
        assert_eq!(h.irqs, [(167, true), (167, false)]);
        // A board with other wiring rejects the checkpoint.
        let plain = Gpio::new(0xfff41000, pins.clone()).unwrap();
        assert!(plain.validate_checkpoint(&saved).is_err());
        assert!(
            Gpio::new(0xfff41000, pins)
                .unwrap()
                .with_interrupts(vec![route(0, 167), route(0, 167)])
                .is_err()
        );
    }
}
