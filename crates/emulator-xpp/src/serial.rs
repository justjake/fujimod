//! X-Processor Pro serial configuration and a provisional discovery endpoint.
//!
//! On the discovery endpoint, +0x28 holds flags; +0x2c clears written bits.
//! A send word written to +0x60 completes at once and sets done bit 2. Bit 3
//! reads set because the send queue never fills. The receive word +0x48
//! reads zero. Other words retain writes. No peer, timing or IRQ is modeled;
//! this is a provisional law for discovery.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Register window size in bytes.
pub const REGISTERS: u64 = 0x100;
const STATUS: usize = 0x28 / 4;
const CLEAR: u64 = 0x2c;
const RECEIVE: u64 = 0x48;
const SEND: u64 = 0x60;
const DONE: u32 = 1 << 2;
const ROOM: u32 = 1 << 3;

/// Controller configuration only. Status and data transfers require a modeled
/// controller/endpoint protocol and refuse on this port.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationPort {
    base: u64,
    registers: BTreeMap<u64, u32>,
}
impl ConfigurationPort {
    pub fn new(base: u64) -> Result<Self, String> {
        if base & 0xfff != 0 {
            return Err("Invalid XPP serial configuration base".into());
        }
        Ok(Self {
            base,
            registers: BTreeMap::new(),
        })
    }
    fn configuration(offset: u64) -> bool {
        matches!(
            offset,
            0 | 4
                | 8
                | 0xc
                | 0x10
                | 0x14
                | 0x18
                | 0x1c
                | 0x20
                | 0x24
                | 0x30
                | 0x34
                | 0x38
                | 0x3c
                | 0x4c
                | 0x50
                | 0x54
                | 0xf0
        )
    }
}
impl Device for ConfigurationPort {
    fn name(&self) -> &'static str {
        "xpp-serial-configuration"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address - self.base;
        if size != 4 || !Self::configuration(offset) {
            cx.fail(format!(
                "XPP serial operation at +{offset:#x} needs a modeled controller/endpoint protocol"
            ));
            return 0;
        }
        self.registers.get(&offset).copied().unwrap_or(0).into()
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address - self.base;
        if size != 4 || value > u32::MAX.into() {
            cx.fail("XPP serial configuration width/value");
        } else if offset == CLEAR && value & !0x7f == 0 {
            // Acknowledge flags while no transfer engine is attached.
        } else if offset == 0xf0 && value > 4 {
            cx.fail("Unsupported XPP serial configuration selector");
        } else if Self::configuration(offset) {
            self.registers.insert(offset, value as u32);
        } else {
            cx.fail(format!(
                "XPP serial operation at +{offset:#x} needs a modeled controller/endpoint protocol"
            ));
        }
    }
    fn save(&self) -> Value {
        json!({"base":self.base,"configuration":self.registers})
    }
    fn restore(&mut self, _: &Value) -> Result<(), String> {
        Err("Serial configuration restoration requires its full checkpoint".into())
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(self.name(), self)
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        if !saved.children().is_empty() {
            return Err("Serial configuration checkpoint has unexpected children".into());
        }
        let state: Self = saved.decode(self.name())?;
        if state.base != self.base || state.registers.keys().any(|&o| !Self::configuration(o)) {
            return Err("Serial configuration checkpoint wiring/registers differ".into());
        }
        Ok(())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        *self = saved.decode(self.name())?;
        Ok(())
    }
}

/// Discovery endpoint with immediate completion and fixed receive words.
pub struct DiscoveryPort {
    base: u64,
    registers: Vec<u32>,
}
impl DiscoveryPort {
    pub fn new(base: u64) -> Result<Self, String> {
        if base & 0xfff != 0 {
            return Err("Invalid X-Processor Pro serial port base".into());
        }
        Ok(Self {
            base,
            registers: vec![0; (REGISTERS / 4) as usize],
        })
    }
}
impl Device for DiscoveryPort {
    fn name(&self) -> &'static str {
        "xpp-serial"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let o = address - self.base;
        if size != 4 || o & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        u64::from(match o {
            RECEIVE => 0,
            0x28 => self.registers[STATUS] | ROOM,
            _ => self.registers[(o / 4) as usize],
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let o = address - self.base;
        if size != 4 || o & 3 != 0 || o == 0x28 {
            cx.unmodeled();
            return;
        }
        match o {
            CLEAR => self.registers[STATUS] &= !(value as u32),
            SEND => {
                self.registers[(SEND / 4) as usize] = value as u32;
                self.registers[STATUS] |= DONE;
            }
            _ => self.registers[(o / 4) as usize] = value as u32,
        }
    }
    fn pollable(&self, address: u64, _size: u8) -> bool {
        address - self.base == 0x28
    }
    fn save(&self) -> Value {
        json!({ "registers": self.registers })
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let registers: Vec<u32> =
            serde_json::from_value(value["registers"].clone()).map_err(|e| e.to_string())?;
        if registers.len() != self.registers.len() {
            return Err("X-Processor Pro serial checkpoint size differs".into());
        }
        self.registers = registers;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, read, write};

    #[test]
    fn configuration_restores_but_status_and_data_require_an_endpoint() {
        let base = 0x80000;
        let mut d = ConfigurationPort::new(base).unwrap();
        let mut h = TestHost::new();
        write(&mut d, &mut h, base + 8, 0);
        write(&mut d, &mut h, base + 0x14, 10);
        write(&mut d, &mut h, base + 0x2c, 0x7f);
        let saved = d.checkpoint().unwrap();
        write(&mut d, &mut h, base + 0x14, 11);
        d.restore_checkpoint(&saved).unwrap();
        assert_eq!(read(&mut d, &mut h, base + 0x14), 10);
        assert!(
            ConfigurationPort::new(base + 0x1000)
                .unwrap()
                .validate_checkpoint(&saved)
                .is_err()
        );
        for offset in [0x28, 0x48] {
            let mut cx = h.cx();
            d.read(&mut cx, base + offset, 4);
            assert!(
                cx.refusal()
                    .unwrap()
                    .contains("needs a modeled controller/endpoint protocol")
            );
        }
        let mut cx = h.cx();
        d.write(&mut cx, base + 0x60, 4, 0x1234);
        assert!(
            cx.refusal()
                .unwrap()
                .contains("needs a modeled controller/endpoint protocol")
        );
    }

    #[test]
    fn send_sets_done_and_clear_drops_written_status_bits() {
        let base = 0xfff2a000;
        let mut port = DiscoveryPort::new(base).unwrap();
        let mut h = TestHost::new();
        assert_eq!(read(&mut port, &mut h, base + 0x28), u64::from(ROOM));
        write(&mut port, &mut h, base + 0x2c, 0x7f);
        assert_eq!(read(&mut port, &mut h, base + 0x48), 0);
        write(&mut port, &mut h, base + 0x60, 0xe800);
        assert_eq!(read(&mut port, &mut h, base + 0x28), u64::from(ROOM | DONE));
        write(&mut port, &mut h, base + 0x2c, DONE);
        assert_eq!(read(&mut port, &mut h, base + 0x28), u64::from(ROOM));
        write(&mut port, &mut h, base + 0x14, 0xa);
        assert_eq!(read(&mut port, &mut h, base + 0x14), 0xa);
        let mut cx = h.cx();
        port.write(&mut cx, base + 0x28, 4, 0);
        assert!(cx.is_unmodeled());
    }
}
