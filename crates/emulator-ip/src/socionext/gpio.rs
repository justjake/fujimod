//! Sixteen masked GPIO banks. Register base and external levels are explicit.
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
    pub fn output(&self, port: usize, pin: usize) -> (bool, u64) {
        (
            self.regs[port][0] & (1 << pin) != 0,
            self.changes[port][pin],
        )
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
}
impl Gpio {
    pub fn new(base: u64, pins: SharedPins) -> Result<Self, String> {
        if base & 0xfff != 0 || base.checked_add(0x400).is_none() {
            return Err("Invalid GPIO bank base".into());
        }
        Ok(Self { base, pins })
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
    fn save(&self) -> Value {
        serde_json::to_value(&*self.pins.lock().unwrap()).unwrap()
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(
                self.base,
                &*self.pins.lock().map_err(|_| "GPIO pins poisoned")?,
            ),
        )
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let (base, _): (u64, Pins) = saved.decode(self.name())?;
        if base != self.base {
            return Err("GPIO checkpoint base differs".into());
        }
        Ok(())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (_, pins): (u64, Pins) = saved.decode(self.name())?;
        *self.pins.lock().map_err(|_| "GPIO pins poisoned")? = pins;
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
}
