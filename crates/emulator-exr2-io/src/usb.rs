//! Bounded USB PHY/core startup controls (FFF54000 / FFF55000).
//!
//! Register resets are unmeasured zero fixtures. Cable presence is an explicit scenario input, independent of
//! written configuration. No ready bits, endpoint traffic, enumeration,
//! completion or interrupt is supplied here. The optional native endpoint part
//! owns overlapping controls on the same physical controller.
use fujimod_emulator_devices::record::Result;
use fujimod_emulator_devices::{Action, Record, Unsupported, Value, WordMap};

pub const PHY: u32 = 0xfff55000;
pub const CORE: u32 = 0xfff54000;
pub const WORDS: [u32; 3] = [PHY + 0x100, PHY + 0x104, PHY + 0x108];
pub const HALVES: [u32; 6] = [
    CORE + 8,
    CORE + 0x90,
    CORE + 0x94,
    CORE + 0x98,
    CORE + 0xac,
    PHY + 0x2fc,
];

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct UsbStartup {
    pub registers: WordMap,
    pub started: bool,
    pub configured: bool,
    pub reset_pulses: i64,
    pub writes: Vec<Value>,
}
impl Default for UsbStartup {
    fn default() -> Self {
        Self {
            registers: WORDS.into_iter().chain(HALVES).map(|a| (a, 0)).collect(),
            started: false,
            configured: false,
            reset_pulses: 0,
            writes: Vec::new(),
        }
    }
}
impl UsbStartup {
    pub fn contains(address: u64, size: u32) -> bool {
        u32::try_from(address).is_ok_and(|a| match size {
            4 => WORDS.contains(&a),
            2 => HALVES.contains(&a),
            _ => false,
        })
    }
    pub fn readable(address: u64, size: u32) -> bool {
        Self::contains(address, size)
            && address != u64::from(PHY + 0x100)
            && address != u64::from(CORE + 0xac)
    }
    pub fn read(&self, address: u64, size: u32, connected: bool) -> Result<u32> {
        if !Self::contains(address, size) {
            return Err(Unsupported(format!(
                "Unknown USB control read {address:#x}/{size}"
            )));
        }
        if !Self::readable(address, size) {
            return Err(Unsupported(format!(
                "Unproven USB control read {address:#x}/{size}"
            )));
        }
        let value = self.registers.get_or_zero(address as u32);
        Ok(value
            | if address == u64::from(PHY + 0x104) && connected {
                1 << 31
            } else {
                0
            })
    }
    pub fn write(
        &mut self,
        cpu: Value,
        address: u64,
        size: u32,
        source: u64,
        size_value: Value,
        actions: &mut Vec<Action>,
    ) -> Result<()> {
        if !Self::contains(address, size) {
            return Err(Unsupported(format!(
                "Unknown USB control write {address:#x}/{size}"
            )));
        }
        if source > u32::MAX as u64 {
            return Err(Unsupported(
                "USB control source register exceeds32 bits".into(),
            ));
        }
        let value = source as u32 & if size == 2 { 0xffff } else { u32::MAX };
        match address as u32 {
            a if a == PHY + 0x100 => {
                if value != 1 {
                    return Err(Unsupported(format!("Unknown USB PHY start {value:#x}")));
                }
                self.started = true;
            }
            a if a == PHY + 0x104 => {
                if !self.started || value != 0x1ecbc7 {
                    return Err(Unsupported(format!(
                        "Unknown USB PHY configuration/state {value:#x}"
                    )));
                }
                self.configured = true;
            }
            a if a == PHY + 0x108 => {
                if !matches!(value, 0 | 0x10000) {
                    return Err(Unsupported(format!(
                        "Unknown USB PHY reset control {value:#x}"
                    )));
                }
                if value == 0x10000 {
                    self.reset_pulses += 1;
                }
            }
            a => {
                if !self.configured {
                    return Err(Unsupported(
                        "USB core control precedes the known PHY configuration".into(),
                    ));
                }
                let admitted = match a {
                    0xfff54008 => matches!(value, 0 | 2),
                    0xfff54090 | 0xfff54098 | 0xfff552fc => matches!(value, 0 | 1),
                    0xfff54094 => matches!(value, 0 | 0xc0),
                    0xfff540ac => value == 0x3fff,
                    _ => false,
                };
                if !admitted {
                    return Err(Unsupported(format!(
                        "Unknown USB core command {address:#x}={value:#x}"
                    )));
                }
            }
        }
        self.registers.insert(address as u32, value);
        let fields = vec![
            ("cpu", cpu),
            ("address", Value::hex(address.into())),
            ("size", size_value),
            ("value", Value::hex(value.into())),
        ];
        self.writes.push(Value::dict(fields.clone()));
        actions.push(Action::Log(Record::new(
            "USB controller configuration write",
            fields,
        )));
        Ok(())
    }
}

mod device;
pub use device::{Options, UsbController, Wiring};
