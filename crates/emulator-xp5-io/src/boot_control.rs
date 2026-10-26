//! XP5 boot-source inputs and boot-control latches. Source straps and initial
//! latch values are explicit board inputs; accesses never inspect guest code.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inputs {
    pub source_straps: u32,
    pub initial_control: u64,
    pub initial_primary_control: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    control: u64,
    primary_control: u32,
}
pub struct BootControl {
    inputs: Inputs,
    state: State,
}
impl BootControl {
    pub fn new(inputs: Inputs) -> Self {
        Self {
            state: State {
                control: inputs.initial_control,
                primary_control: inputs.initial_primary_control,
            },
            inputs,
        }
    }
}
impl Device for BootControl {
    fn name(&self) -> &'static str {
        "boot-control"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match (address, size) {
            (0xff90e000, 4) => self.inputs.source_straps.into(),
            (0xff90e400, 4) => self.state.control as u32 as u64,
            (0xff90e404, 4) => self.state.control >> 32,
            (0xff90e400, 8) => self.state.control,
            (0xfa140000, 4) => self.state.primary_control.into(),
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        match (address, size) {
            (0xff90e400, 4) => {
                self.state.control = (self.state.control & !0xffffffff) | (value & 0xffffffff)
            }
            (0xff90e404, 4) => {
                self.state.control =
                    (self.state.control & 0xffffffff) | ((value & 0xffffffff) << 32)
            }
            (0xff90e400, 8) => self.state.control = value,
            (0xfa140000, 4) => self.state.primary_control = value as u32,
            _ => cx.unmodeled(),
        }
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        self.state = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    #[test]
    fn source_is_external_and_control_lanes_preserve_the_other_half() {
        let mut device = BootControl::new(Inputs {
            source_straps: 5,
            initial_control: 0,
            initial_primary_control: 0,
        });
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        device.write(&mut cx, 0xff90e400, 4, 1);
        device.write(&mut cx, 0xff90e404, 4, 2);
        assert_eq!(device.read(&mut cx, 0xff90e400, 8), 0x200000001);
        assert_eq!(device.read(&mut cx, 0xff90e000, 4), 5);
        let saved = device.save();
        device.write(&mut cx, 0xff90e400, 8, 0);
        device.restore(&saved).unwrap();
        assert_eq!(device.save(), saved);
    }
}
