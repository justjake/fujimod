//! EXR system-control latches at FFF98000. Reset values are declared inputs.
//! The mode and timing words retain programmed configuration; this model
//! does not infer a PLL frequency, lock transition, or DDR training result.
use crate::{Cx, Device};
use serde::{Deserialize, Serialize};

pub mod clocks;

/// The boot memory-interface command/status protocol has no qualified model.
/// Refuse before retaining a command or claiming calibration completed.
pub struct MemoryInterface;
impl Device for MemoryInterface {
    fn name(&self) -> &'static str {
        "exr-memory-interface"
    }
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(serde_json::Value::Null)
    }
    fn read(&mut self, cx: &mut Cx<'_>, _address: u64, _size: u8) -> u64 {
        cx.fail("EXR memory-interface training/calibration is unmodeled; needs register semantics or a reference bus trace covering commands, status, timing and failure cases");
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, _address: u64, _size: u8, _value: u64) {
        cx.fail("EXR memory-interface training/calibration is unmodeled; needs register semantics or a reference bus trace covering commands, status, timing and failure cases");
    }
}

const BASE: u64 = 0xfff98000;
const OFFSETS: [u64; 4] = [0, 8, 0x204, 0x208];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub boot_source: u32,
    pub selection_reset: u32,
    pub mode_reset: u32,
    pub timing_reset: u32,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub word: u32,
    pub choice: usize,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Wiring {
    pub decoder: usize,
    pub selections: Vec<Selection>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    registers: [u32; 4],
}
pub struct SystemControl {
    wiring: Wiring,
    state: State,
}
impl SystemControl {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        if !wiring
            .selections
            .iter()
            .any(|s| s.word == options.selection_reset)
            || wiring
                .selections
                .iter()
                .enumerate()
                .any(|(i, s)| wiring.selections[..i].iter().any(|p| p.word == s.word))
        {
            return Err("EXR control needs unique memory selections including reset".into());
        }
        Ok(Self {
            wiring,
            state: State {
                registers: [
                    options.boot_source,
                    options.selection_reset,
                    options.mode_reset,
                    options.timing_reset,
                ],
            },
        })
    }
    fn index(address: u64, size: u8) -> Option<usize> {
        (size == 4)
            .then(|| OFFSETS.iter().position(|o| BASE + o == address))
            .flatten()
    }
}
impl Device for SystemControl {
    crate::checkpoint_fields!(state: State);
    fn name(&self) -> &'static str {
        "exr-system-control"
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
            cx.fail("EXR word exceeds 32 bits");
            return;
        };
        if index == 1 {
            let Some(selection) = self.wiring.selections.iter().find(|s| s.word == word) else {
                cx.fail("unsupported EXR memory selection");
                return;
            };
            if let Err(error) = cx.select_memory(self.wiring.decoder, selection.choice) {
                cx.fail(error);
                return;
            }
        }
        self.state.registers[index] = word;
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        Self::index(address, size).is_some()
    }
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"state":self.state,"wiring":self.wiring}))
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if !self
            .wiring
            .selections
            .iter()
            .any(|s| s.word == state.registers[1])
        {
            return Err("unsupported restored EXR memory selection".into());
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_host::{TestHost, read, write};
    #[test]
    fn unknown_memory_training_refuses_before_applying_a_command_or_status() {
        let mut device = MemoryInterface;
        let mut host = TestHost::new();
        let mut cx = host.cx();
        device.write(&mut cx, 0xfffb0008, 4, 0x2000f);
        assert!(cx.refusal().unwrap().contains("reference bus trace"));
        assert_eq!(device.save(), serde_json::Value::Null);
        assert!(host.effects.is_empty());
    }
    #[test]
    fn configuration_is_retained_without_clock_side_effects_or_inspection_mutation() {
        let mut device = SystemControl::new(
            Options {
                boot_source: 4,
                selection_reset: 0,
                mode_reset: 0x01020000,
                timing_reset: 0x01000000,
            },
            Wiring {
                decoder: 0,
                selections: vec![
                    Selection { word: 0, choice: 0 },
                    Selection { word: 1, choice: 1 },
                ],
            },
        )
        .unwrap();
        let mut host = TestHost::new();
        assert_eq!(read(&mut device, &mut host, BASE + 0x204), 0x01020000);
        for (offset, word) in [(0, 0x10004), (0x204, 0x01010000), (0x208, 0x1000000)] {
            write(&mut device, &mut host, BASE + offset, word);
            assert_eq!(read(&mut device, &mut host, BASE + offset), u64::from(word));
        }
        let saved = device.save();
        for _ in 0..4 {
            assert_eq!(device.next_deadline(u64::MAX), None);
            assert_eq!(device.save(), saved);
            device.fingerprint().unwrap();
        }
        assert!(host.effects.is_empty());
        for (address, size) in [(BASE + 4, 4), (BASE + 0x204, 2), (BASE + 0x100, 4)] {
            let mut cx = host.cx();
            device.read(&mut cx, address, size);
            assert!(cx.is_unmodeled());
        }
        let mut invalid = saved.clone();
        invalid["registers"][1] = serde_json::json!(2);
        assert!(device.restore(&invalid).is_err());
        assert_eq!(device.save(), saved);
    }
}
