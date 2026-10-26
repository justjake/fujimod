//! Boot-source latch and FFF98008 memory selection. The meanings of selection
//! words are supplied by board wiring: the old 1=RAM/2=storage interpretation
//! is not a measured address decoder. No CPU state or code bytes are inputs.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};

pub const BASE: u64 = 0xfff98000;
const OFFSETS: [u64; 5] = [0, 8, 0x100, 0x104, 0x208];
#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub boot_source: u32,
    pub selection_reset: u32,
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
    registers: [u32; 5],
}
pub struct Block98000 {
    wiring: Wiring,
    state: State,
}
impl Block98000 {
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
            return Err("boot control requires unique selections including its reset word".into());
        }
        Ok(Self {
            wiring,
            state: State {
                registers: [options.boot_source, options.selection_reset, 0, 0, 0],
            },
        })
    }
    fn index(address: u64, size: u8) -> Option<usize> {
        if size != 4 {
            return None;
        }
        OFFSETS
            .iter()
            .position(|o| BASE.checked_add(*o) == Some(address))
    }
}
impl Device for Block98000 {
    fujimod_emulator_devices::checkpoint_fields!(state: State);

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(serde_json::json!({"state":self.state,"wiring":self.wiring}))
    }

    fn name(&self) -> &'static str {
        "block-98000"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match Self::index(address, size) {
            Some(index) => self.state.registers[index] as u64,
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
            cx.fail("boot control word exceeds 32 bits");
            return;
        };
        if index == 1 {
            let Some(selection) = self.wiring.selections.iter().find(|s| s.word == word) else {
                cx.fail(format!("unsupported memory selection word {word:#x}"));
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
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(&self.state).expect("boot control state")
    }
    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if !self
            .wiring
            .selections
            .iter()
            .any(|s| s.word == state.registers[1])
        {
            return Err("unsupported restored memory selection".into());
        }
        self.state = state;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{Host, Record};
    #[derive(Default)]
    struct Output {
        selections: Vec<(usize, usize)>,
        refuse: bool,
    }
    impl Host for Output {
        fn now(&self) -> u64 {
            0
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            panic!("no boot deadlines")
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            panic!("no boot interrupts")
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            panic!("no boot interrupts")
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            panic!("no boot DMA")
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            panic!("no boot DMA")
        }
        fn record(&mut self, _: Record) {
            panic!("no boot records")
        }
        fn select_memory(&mut self, decoder: usize, choice: usize) -> Result<(), String> {
            if self.refuse {
                return Err("unconnected selection".into());
            }
            self.selections.push((decoder, choice));
            Ok(())
        }
    }
    fn control() -> Block98000 {
        Block98000::new(
            Options {
                boot_source: 3,
                selection_reset: 0,
            },
            Wiring {
                decoder: 7,
                selections: vec![
                    Selection { word: 0, choice: 2 },
                    Selection { word: 1, choice: 4 },
                    Selection { word: 2, choice: 4 },
                ],
            },
        )
        .unwrap()
    }
    #[test]
    fn only_declared_words_select_a_connected_output_and_failed_writes_preserve_state() {
        let mut d = control();
        let mut host = Output::default();
        for (word, choice) in [(1, 4), (2, 4), (0, 2)] {
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            d.write(&mut cx, BASE + 8, 4, word);
            assert_eq!(cx.refusal(), None);
            assert_eq!(d.read(&mut cx, BASE + 8, 4), word);
            assert_eq!(host.selections.last(), Some(&(7, choice)));
        }
        let before = d.save();
        for word in [3, 1u64 << 32] {
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            d.write(&mut cx, BASE + 8, 4, word);
            assert!(cx.refusal().is_some());
            assert_eq!(d.save(), before);
        }
        host.refuse = true;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        d.write(&mut cx, BASE + 8, 4, 1);
        assert_eq!(cx.refusal(), Some("unconnected selection"));
        assert_eq!(d.save(), before);
    }
    #[test]
    fn register_ownership_state_and_deadline_inspection_are_observational() {
        let mut d = control();
        let mut host = Output::default();
        for (offset, value) in [(0, 3), (0x100, 19), (0x104, 20), (0x208, 21)] {
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            d.write(&mut cx, BASE + offset, 4, value);
            assert_eq!(cx.refusal(), None);
            assert_eq!(d.read(&mut cx, BASE + offset, 4), value);
        }
        for (address, size) in [
            (BASE + 4, 4),
            (BASE + 8, 2),
            (0xfff5002c, 2),
            (0xfff5102c, 2),
        ] {
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            d.read(&mut cx, address, size);
            assert!(cx.is_unmodeled());
        }
        let before = d.save();
        d.restore(&before).unwrap();
        assert_eq!(d.next_deadline(u64::MAX), None);
        assert_eq!(d.save(), before);
        assert!(host.selections.is_empty());
        let mut bad = before.clone();
        bad["registers"][1] = serde_json::json!(99);
        assert!(d.restore(&bad).is_err());
        assert_eq!(d.save(), before);
        for selections in [
            vec![],
            vec![
                Selection { word: 0, choice: 0 },
                Selection { word: 0, choice: 1 },
            ],
        ] {
            assert!(
                Block98000::new(
                    Options {
                        boot_source: 0,
                        selection_reset: 0
                    },
                    Wiring {
                        decoder: 0,
                        selections
                    }
                )
                .is_err()
            );
        }
    }
}
