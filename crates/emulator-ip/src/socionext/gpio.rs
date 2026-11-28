//! Sixteen masked GPIO banks. Register base and external levels are explicit.
//!
//! A pad may also be wired to a level-sensitive processor interrupt input:
//! the line is held asserted while the pad's external level equals the
//! route's active level. The interrupt controller latches and clears it.
//! One pad may drive two inputs of opposite active levels.
//!
//! A modeled peer may drive input pads itself: it records each change with
//! the tick it takes effect (`Pins::drive_input`), and the bank applies it and
//! routes its interrupt inputs at that tick, its next deadline. A peer that
//! answers output levels follows them as an `OutputWatcher`: the bank tells
//! it, at the write's tick, about every output bit a guest write changes.
use fujimod_emulator_devices::parts::gpio::InputIrq;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};
/// A modeled peer that follows output pads and may drive input pads in
/// return (`Pins::drive_input`). Called with the bank's pins locked.
pub trait OutputWatcher: Send {
    /// Output `pin` of bank `port` changed to `high` at tick `now`.
    fn output_changed(&mut self, pins: &mut Pins, port: usize, pin: usize, high: bool, now: u64);
}
pub type SharedWatcher = Arc<Mutex<dyn OutputWatcher>>;
/// An input level a modeled peer drives from a tick on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputChange {
    pub at: u64,
    pub port: u8,
    pub pin: u8,
    pub high: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Pins {
    regs: [[u32; 8]; 16],
    inputs: [u16; 16],
    changes: [[u64; 16]; 16],
    /// Peer-driven input changes not yet applied, in tick order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending: Vec<InputChange>,
    /// Wiring, not state: restoring keeps the connected peers.
    #[serde(skip)]
    watchers: Vec<(usize, SharedWatcher)>,
}
pub type SharedPins = Arc<Mutex<Pins>>;
impl Pins {
    pub fn new(inputs: [u16; 16]) -> SharedPins {
        Arc::new(Mutex::new(Self {
            regs: [[0; 8]; 16],
            inputs,
            changes: [[0; 16]; 16],
            pending: Vec::new(),
            watchers: Vec::new(),
        }))
    }
    /// Connects a peer that follows changes to a particular bank register.
    pub fn watch(&mut self, register: usize, watcher: SharedWatcher) {
        self.watchers.push((register, watcher));
    }
    /// A modeled peer drives input `pin` of bank `port` to `high` from tick
    /// `at` on. The bank applies it, and routes its interrupt inputs, at that
    /// tick; changes of one pad apply in tick order.
    pub fn drive_input(
        &mut self,
        at: u64,
        port: usize,
        pin: usize,
        high: bool,
    ) -> Result<(), String> {
        if port >= 16 || pin >= 16 {
            return Err("Invalid GPIO input pin".into());
        }
        let change = InputChange {
            at,
            port: port as u8,
            pin: pin as u8,
            high,
        };
        let index = self.pending.partition_point(|c| c.at <= at);
        self.pending.insert(index, change);
        Ok(())
    }
    /// Withdraws the changes not yet applied to input `pin` of bank `port`.
    pub fn cancel_inputs(&mut self, port: usize, pin: usize) {
        self.pending
            .retain(|c| (usize::from(c.port), usize::from(c.pin)) != (port, pin));
    }
    /// Tick of the earliest peer-driven input change not yet applied.
    pub fn next_input_change(&self) -> Option<u64> {
        self.pending.first().map(|c| c.at)
    }
    /// Peer-driven input changes not yet applied, in tick order.
    pub fn pending_inputs(&self) -> &[InputChange] {
        &self.pending
    }
    /// Applies the peer-driven changes due by `now`.
    fn apply_inputs(&mut self, now: u64) {
        let due = self.pending.partition_point(|c| c.at <= now);
        for change in self.pending.drain(..due).collect::<Vec<_>>() {
            self.set_input(
                usize::from(change.port),
                usize::from(change.pin),
                change.high,
            );
        }
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
            // One pad may hold two inputs only at opposite active levels.
            if interrupts[..i].iter().any(|other| {
                (other.port, other.pin, other.active_high) == (irq.port, irq.pin, irq.active_high)
                    || other.line == irq.line
            }) {
                return Err("Duplicate GPIO interrupt route".into());
            }
        }
        self.previous = interrupts.iter().map(|irq| !irq.active_high).collect();
        self.interrupts = interrupts;
        Ok(self)
    }
    /// Applies the peer-driven input changes due by now, then routes the pads.
    fn settle(&mut self, cx: &mut Cx<'_>) {
        self.pins.lock().unwrap().apply_inputs(cx.now());
        self.route_interrupts(cx);
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
impl fujimod_emulator_devices::capability::GpioInputs for Gpio {
    fn admit_inputs(
        &self,
        changes: &[(fujimod_emulator_devices::parts::pins::Pin, bool)],
    ) -> Result<(), String> {
        if changes
            .iter()
            .any(|(pin, _)| pin.port >= 16 || pin.pin >= 16)
        {
            return Err("Invalid GPIO input pin".into());
        }
        Ok(())
    }
    fn request_inputs(
        &mut self,
        changes: &[(fujimod_emulator_devices::parts::pins::Pin, bool)],
    ) -> Result<(), String> {
        self.admit_inputs(changes)?;
        let mut pins = self.pins.lock().unwrap();
        for (pin, high) in changes {
            pins.set_input(usize::from(pin.port), usize::from(pin.pin), *high);
        }
        Ok(())
    }
}
impl Device for Gpio {
    fn future_deadline_is_stable(&self) -> bool {
        // Pending pin changes are absolute ticks. An already changed pin is due now.
        true
    }
    fn name(&self) -> &'static str {
        "masked-gpio"
    }
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
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(self.base) as usize;
        let p = offset / 64;
        let r = offset % 64 / 4;
        if size != 4 || offset & 3 != 0 || p >= 16 || r >= 8 {
            cx.unmodeled();
            return 0;
        }
        self.settle(cx);
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
        self.settle(cx);
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
            let new = pins.regs[p][r];
            for (bit, changes) in pins.changes[p].iter_mut().enumerate() {
                if (old ^ new) & (1 << bit) != 0 {
                    *changes += 1;
                }
            }
        }
        let new = pins.regs[p][r];
        let changed = (old ^ new) & 0xffff;
        if changed == 0 {
            return;
        }
        // Watching peers follow each changed bit of the register they wired.
        let watchers: Vec<_> = pins
            .watchers
            .iter()
            .filter(|(register, _)| *register == r)
            .map(|(_, watcher)| watcher.clone())
            .collect();
        if !watchers.is_empty() {
            for bit in (0..16).filter(|bit| changed & (1 << bit) != 0) {
                for watcher in &watchers {
                    watcher.lock().unwrap().output_changed(
                        &mut pins,
                        p,
                        bit,
                        new & (1 << bit) != 0,
                        cx.now(),
                    );
                }
            }
            let due = pins.next_input_change().is_some_and(|at| at <= cx.now());
            drop(pins);
            if due {
                self.settle(cx);
            }
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _token: u64) {
        self.advance(cx);
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        self.settle(cx);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        let pins = self.pins.lock().unwrap();
        let changed = self
            .interrupts
            .iter()
            .zip(&self.previous)
            .any(|(irq, previous)| pins.input(irq.port, irq.pin) != *previous);
        let driven = pins.next_input_change().map(|at| at.max(now));
        if changed { Some(now) } else { driven }
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
        let (base, pins, previous): (u64, Pins, Vec<bool>) = saved.decode(self.name())?;
        if base != self.base {
            return Err("GPIO checkpoint base differs".into());
        }
        if pins.pending.iter().any(|c| c.port >= 16 || c.pin >= 16)
            || pins.pending.windows(2).any(|pair| pair[0].at > pair[1].at)
        {
            return Err("Invalid GPIO peer-driven input changes".into());
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
        let (_, mut pins, previous): (u64, Pins, Vec<bool>) = saved.decode(self.name())?;
        let mut current = self.pins.lock().map_err(|_| "GPIO pins poisoned")?;
        pins.watchers = std::mem::take(&mut current.watchers);
        *current = pins;
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

    fn pad(line: u32, active_high: bool) -> InputIrq {
        InputIrq {
            port: 0,
            pin: 4,
            line,
            active_high,
        }
    }
    #[test]
    fn one_pad_may_hold_two_inputs_of_opposite_levels_but_lines_stay_unique() {
        let pins = Pins::new([0x10; 16]);
        let gpio = |routes| {
            Gpio::new(0xfff41000, pins.clone())
                .unwrap()
                .with_interrupts(routes)
        };
        assert!(gpio(vec![pad(171, false), pad(155, true)]).is_ok());
        for routes in [
            vec![pad(171, false), pad(155, false)],
            vec![pad(171, false), pad(171, true)],
            vec![
                pad(171, false),
                InputIrq {
                    port: 1,
                    pin: 2,
                    line: 171,
                    active_high: true,
                },
            ],
        ] {
            assert_eq!(
                gpio(routes).err().as_deref(),
                Some("Duplicate GPIO interrupt route")
            );
        }
    }
    #[test]
    fn a_peer_driven_input_change_applies_and_routes_at_its_tick() {
        use fujimod_emulator_devices::test_host::recording::RecordingHost;
        let pins = Pins::new([0x10; 16]);
        let mut d = Gpio::new(0xfff41000, pins.clone())
            .unwrap()
            .with_interrupts(vec![pad(171, false), pad(155, true)])
            .unwrap();
        let mut h = RecordingHost::default();
        // The high pad asserts the active-high input at the first scan.
        assert_eq!(d.next_deadline(0), Some(0));
        d.advance(&mut Cx::new(&mut h, 0, 0, 0));
        assert_eq!(h.irqs, [(155, true)]);
        pins.lock().unwrap().drive_input(100, 0, 4, false).unwrap();
        assert_eq!(d.next_deadline(10), Some(100));
        h.now = 99;
        d.advance(&mut Cx::new(&mut h, 0, 0, 0));
        assert_eq!(read_input(&mut d, &mut h), 0x10);
        assert_eq!(h.irqs.len(), 1);
        // The checkpoint keeps the change that is still to come.
        let saved = d.checkpoint().unwrap();
        h.now = 100;
        assert_eq!(read_input(&mut d, &mut h), 0, "a read at the tick sees it");
        assert_eq!(h.irqs, [(155, true), (171, true), (155, false)]);
        assert_eq!(d.next_deadline(100), None);
        d.restore_checkpoint(&saved).unwrap();
        assert_eq!(pins.lock().unwrap().pending_inputs().len(), 1);
        assert_eq!(d.next_deadline(99), Some(100));
        // A change already due is applied at once.
        pins.lock().unwrap().cancel_inputs(0, 4);
        assert_eq!(d.next_deadline(99), None);
        pins.lock().unwrap().drive_input(50, 0, 4, false).unwrap();
        assert_eq!(d.next_deadline(99), Some(99));
        assert!(pins.lock().unwrap().drive_input(0, 16, 0, true).is_err());
    }
    fn read_input(
        d: &mut Gpio,
        h: &mut fujimod_emulator_devices::test_host::recording::RecordingHost,
    ) -> u64 {
        d.read(&mut Cx::new(h, 0, 0, 0), 0xfff41000 + 0x14, 4)
    }
    /// Answers output 1.2 rising with input 0.4 falling ten ticks later.
    struct Echo(Vec<(usize, usize, bool, u64)>);
    impl OutputWatcher for Echo {
        fn output_changed(
            &mut self,
            pins: &mut Pins,
            port: usize,
            pin: usize,
            high: bool,
            now: u64,
        ) {
            self.0.push((port, pin, high, now));
            if (port, pin, high) == (1, 2, true) {
                pins.drive_input(now + 10, 0, 4, false).unwrap();
            }
        }
    }
    #[test]
    fn a_watching_peer_follows_output_changes_and_survives_restore() {
        use fujimod_emulator_devices::test_host::recording::RecordingHost;
        let pins = Pins::new([0x10; 16]);
        let echo = Arc::new(Mutex::new(Echo(vec![])));
        pins.lock().unwrap().watch(0, echo.clone());
        let mut d = Gpio::new(0xfff41000, pins.clone()).unwrap();
        let mut h = RecordingHost {
            now: 5,
            ..Default::default()
        };
        let saved = d.checkpoint().unwrap();
        d.write(
            &mut Cx::new(&mut h, 0, 0, 0),
            0xfff41000 + 64,
            4,
            0x0006_0006,
        );
        // Bits 1 and 2 changed; the same value again changes nothing.
        d.write(
            &mut Cx::new(&mut h, 0, 0, 0),
            0xfff41000 + 64,
            4,
            0x0006_0006,
        );
        assert_eq!(echo.lock().unwrap().0, [(1, 1, true, 5), (1, 2, true, 5)]);
        assert_eq!(d.next_deadline(5), Some(15));
        d.restore_checkpoint(&saved).unwrap();
        assert_eq!(d.next_deadline(5), None);
        h.now = 7;
        d.write(
            &mut Cx::new(&mut h, 0, 0, 0),
            0xfff41000 + 64,
            4,
            0x0004_0004,
        );
        assert_eq!(echo.lock().unwrap().0.last(), Some(&(1, 2, true, 7)));
        assert_eq!(d.next_deadline(7), Some(17));
    }
}
