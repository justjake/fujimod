//! EXR II DRAM controller and PHY register apertures. Register retention and
//! immediate training completion preserve the established boot model; the
//! reset values and physical training time remain unmeasured inputs.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};

pub const CONTROLLER: u64 = 0xfff99000;
pub const PHY: u64 = 0xfff01000;
const OFFSETS: [u64; 20] = [
    0, 4, 8, 0xc, 0x10, 0x14, 0x18, 0x1c, 0x40, 0x44, 0xc0, 0xc8, 0xd0, 0xd8, 0xe0, 0xe4, 0xe8,
    0xec, 0xf4, 0xf8,
];
#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {}
pub struct Wiring;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControllerState {
    registers: [u32; 20],
}
pub struct Controller {
    state: ControllerState,
}
impl Controller {
    pub fn new(_: Options, _: Wiring) -> Result<Self, String> {
        Ok(Self {
            state: ControllerState { registers: [0; 20] },
        })
    }
    fn index(address: u64, size: u8) -> Option<usize> {
        (size == 4)
            .then(|| address.checked_sub(CONTROLLER))
            .flatten()
            .and_then(|offset| OFFSETS.iter().position(|&o| o == offset))
    }
}
impl Device for Controller {
    fujimod_emulator_devices::checkpoint_fields!(state: ControllerState);

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "dram-controller"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match Self::index(address, size) {
            Some(index) => self.state.registers[index].into(),
            None => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(index) = Self::index(address, size) else {
            cx.unmodeled();
            return;
        };
        let Ok(word) = u32::try_from(value) else {
            cx.fail("DRAM word exceeds 32 bits");
            return;
        };
        self.state.registers[index] = word;
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        Self::index(address, size).is_some()
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        self.state = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PhyState {
    registers: Vec<u32>,
    training_started: bool,
}
pub struct Phy {
    state: PhyState,
}
impl Phy {
    pub fn new(_: Options, _: Wiring) -> Result<Self, String> {
        Ok(Self {
            state: PhyState {
                registers: vec![0; 0x248 / 4],
                training_started: false,
            },
        })
    }
    fn index(address: u64, size: u8) -> Option<usize> {
        let offset = address.checked_sub(PHY)?;
        (size == 4 && offset < 0x248 && offset.is_multiple_of(4)).then_some((offset / 4) as usize)
    }
}
impl Device for Phy {
    fujimod_emulator_devices::checkpoint_fields!(state: PhyState);

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "dram-phy"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(index) = Self::index(address, size) else {
            cx.unmodeled();
            return 0;
        };
        u64::from(
            self.state.registers[index]
                | if index == 0x90 / 4 && self.state.training_started {
                    1 << 28
                } else {
                    0
                },
        )
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(index) = Self::index(address, size) else {
            cx.unmodeled();
            return;
        };
        let Ok(word) = u32::try_from(value) else {
            cx.fail("DRAM PHY word exceeds 32 bits");
            return;
        };
        self.state.registers[index] = word;
        if index == 0 && word & 1 != 0 {
            self.state.training_started = true;
        }
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        Self::index(address, size).is_some()
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let state: PhyState = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state.registers.len() != 0x248 / 4 {
            return Err("invalid DRAM PHY register extent".into());
        }
        self.state = state;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};
    #[test]
    fn training_is_caused_by_the_phy_start_write_and_inspection_is_pure() {
        let mut host = TestHost::new();
        let mut phy = Phy::new(Options {}, Wiring).unwrap();
        assert_eq!(read(&mut phy, &mut host, PHY + 0x90), 0);
        write(&mut phy, &mut host, PHY + 4, 1);
        assert_eq!(read(&mut phy, &mut host, PHY + 0x90), 0);
        write(&mut phy, &mut host, PHY, 1);
        assert_eq!(read(&mut phy, &mut host, PHY + 0x90), 1 << 28);
        write(&mut phy, &mut host, PHY, 0);
        let saved = phy.save();
        advance(&mut phy, &mut host, 10);
        assert_eq!(phy.save(), saved);
        assert_eq!(phy.next_deadline(10), None);
        phy.restore(&saved).unwrap();
        assert_eq!(read(&mut phy, &mut host, PHY + 0x90), 1 << 28);
        assert!(host.effects.is_empty());
    }
    #[test]
    fn independent_blocks_refuse_other_system_control_owners_and_unknown_registers() {
        let mut host = TestHost::new();
        let mut controller = Controller::new(Options {}, Wiring).unwrap();
        write(&mut controller, &mut host, CONTROLLER + 0xf8, 0x12345678);
        assert_eq!(
            read(&mut controller, &mut host, CONTROLLER + 0xf8),
            0x12345678
        );
        for address in [
            CONTROLLER + 0x20,
            PHY,
            0xfff88000,
            0xfff89000,
            0xfff98008,
            0xfff5002c,
            0xfff5102c,
        ] {
            let mut cx = host.cx();
            controller.read(&mut cx, address, 4);
            assert!(cx.is_unmodeled());
        }
    }
}
