//! Declared I2C targets by seven-bit address, behind the register-pointer
//! protocol: after a write START, the first byte selects a register (through
//! the part's `select`, which keeps command bits out of the register number);
//! later written bytes and read bytes move through consecutive registers. A
//! read START keeps the pointer. Each target keeps its own pointer.
//!
//! An address without a declared target is not acknowledged. A register a part
//! does not model refuses.
use fujimod_emulator_devices::parts::i2c::Bus;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A part's registers as the pointer protocol reaches them.
pub trait Registers: Send {
    fn name(&self) -> &'static str;
    /// The register a pointer byte selects.
    fn select(&self, byte: u8) -> u8 {
        byte
    }
    fn write(&mut self, register: u8, value: u8) -> Result<(), String>;
    fn read(&mut self, register: u8) -> Result<u8, String>;
    fn save(&self) -> Value;
    fn restore(&mut self, state: &Value) -> Result<(), String>;
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pointer {
    register: u8,
    /// A pointer byte arrived since the last write START.
    selected: bool,
}

pub struct Target {
    pub address: u8,
    pub registers: Box<dyn Registers>,
}

pub struct Targets {
    targets: Vec<Target>,
    pointers: Vec<Pointer>,
    active: Option<usize>,
}

impl Targets {
    pub fn new(targets: Vec<Target>) -> Result<Self, String> {
        let mut addresses: Vec<u8> = targets.iter().map(|t| t.address).collect();
        addresses.sort_unstable();
        addresses.dedup();
        if addresses.len() != targets.len() || addresses.iter().any(|a| !(8..=0x77).contains(a)) {
            return Err("I2C targets need distinct seven-bit addresses 0x08..0x77".into());
        }
        Ok(Self {
            pointers: vec![Pointer::default(); targets.len()],
            targets,
            active: None,
        })
    }
    fn addressed(&mut self) -> Result<(&mut Target, &mut Pointer), String> {
        let index = self.active.ok_or("I2C byte without an addressed target")?;
        Ok((&mut self.targets[index], &mut self.pointers[index]))
    }
}

impl Bus for Targets {
    fn start(&mut self, address: u16, read: bool) -> bool {
        self.active = self.targets.iter().position(|t| u16::from(t.address) == address);
        if let (Some(index), false) = (self.active, read) {
            self.pointers[index].selected = false;
        }
        self.active.is_some()
    }
    fn write(&mut self, byte: u8) -> Result<(), String> {
        let (target, pointer) = self.addressed()?;
        if !pointer.selected {
            pointer.register = target.registers.select(byte);
            pointer.selected = true;
            return Ok(());
        }
        target
            .registers
            .write(pointer.register, byte)
            .map_err(|e| format!("I2C target {:#04x}: {e}", target.address))?;
        pointer.register = pointer.register.wrapping_add(1);
        Ok(())
    }
    fn read(&mut self) -> Result<u8, String> {
        let (target, pointer) = self.addressed()?;
        let value = target
            .registers
            .read(pointer.register)
            .map_err(|e| format!("I2C target {:#04x}: {e}", target.address))?;
        pointer.register = pointer.register.wrapping_add(1);
        Ok(value)
    }
    fn stop(&mut self) {
        self.active = None;
    }
    fn save(&self) -> Value {
        json!({
            "active": self.active,
            "pointers": self.pointers,
            "targets": self.targets.iter().map(|t| t.registers.save()).collect::<Vec<_>>(),
        })
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let active: Option<usize> =
            serde_json::from_value(value["active"].clone()).map_err(|e| e.to_string())?;
        let pointers: Vec<Pointer> =
            serde_json::from_value(value["pointers"].clone()).map_err(|e| e.to_string())?;
        let targets = value["targets"].as_array().ok_or("I2C targets checkpoint")?;
        if pointers.len() != self.targets.len()
            || targets.len() != self.targets.len()
            || active.is_some_and(|a| a >= self.targets.len())
        {
            return Err("I2C targets checkpoint does not match the declared targets".into());
        }
        for (target, state) in self.targets.iter_mut().zip(targets) {
            target.registers.restore(state)?;
        }
        self.pointers = pointers;
        self.active = active;
        Ok(())
    }
}

/// Registers that read back what was written to them. A register never
/// written refuses a read; no reset value or part behavior is implied.
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedRegisters {
    written: BTreeMap<u8, u8>,
}
impl Registers for RetainedRegisters {
    fn name(&self) -> &'static str {
        "retained-registers"
    }
    fn write(&mut self, register: u8, value: u8) -> Result<(), String> {
        self.written.insert(register, value);
        Ok(())
    }
    fn read(&mut self, register: u8) -> Result<u8, String> {
        self.written
            .get(&register)
            .copied()
            .ok_or_else(|| format!("register {register:#04x} was never written"))
    }
    fn save(&self) -> Value {
        serde_json::to_value(self).unwrap()
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        *self = serde_json::from_value(state.clone()).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bus() -> Targets {
        Targets::new(vec![
            Target {
                address: 0x66,
                registers: Box::new(RetainedRegisters::default()),
            },
            Target {
                address: 0x68,
                registers: Box::new(RetainedRegisters::default()),
            },
        ])
        .unwrap()
    }

    #[test]
    fn declared_targets_answer_and_absent_addresses_are_not_acknowledged() {
        let mut bus = bus();
        assert!(!bus.start(0x6a, false));
        assert!(bus.write(0x12).is_err());
        assert!(bus.start(0x66, false));
        for byte in [0x10, 0xaa, 0xbb] {
            bus.write(byte).unwrap();
        }
        bus.stop();
        // Each target keeps its own pointer; a read START keeps it.
        assert!(bus.start(0x68, false));
        bus.write(0x10).unwrap();
        assert!(bus.start(0x66, false));
        bus.write(0x11).unwrap();
        assert!(bus.start(0x66, true));
        assert_eq!(bus.read().unwrap(), 0xbb);
        assert!(bus.read().unwrap_err().contains("never written"));
        assert!(bus.start(0x68, true));
        assert!(bus.read().unwrap_err().contains("0x68"));
    }

    #[test]
    fn targets_need_distinct_seven_bit_addresses_and_restore_their_state() {
        let retained = || Box::new(RetainedRegisters::default());
        assert!(
            Targets::new(vec![
                Target { address: 0x66, registers: retained() },
                Target { address: 0x66, registers: retained() },
            ])
            .is_err()
        );
        assert!(Targets::new(vec![Target { address: 0x78, registers: retained() }]).is_err());
        let mut bus = bus();
        bus.start(0x66, false);
        bus.write(1).unwrap();
        bus.write(2).unwrap();
        let saved = bus.save();
        let mut other = self::bus();
        other.restore(&saved).unwrap();
        assert!(other.start(0x66, false));
        other.write(1).unwrap();
        other.start(0x66, true);
        assert_eq!(other.read().unwrap(), 2);
    }
}
