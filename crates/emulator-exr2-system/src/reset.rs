//! Reset configuration words at FFFB2004..14. Their retained values do not
//! imply an unobserved reset action. Processor reset/release has separate
//! explicit board wiring. The final configuration word is retained for
//! UART transport setup.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
pub const BASE: u64 = 0xfffb2000;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {}
pub struct Wiring;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    registers: [u32; 5],
}
pub struct Reset {
    state: State,
}
impl Reset {
    pub fn new(_: Options, _: Wiring) -> Result<Self, String> {
        Ok(Self {
            state: State { registers: [0; 5] },
        })
    }
    fn index(address: u64, size: u8) -> Option<usize> {
        let offset = address.checked_sub(BASE)?;
        (size == 4 && (4..=0x14).contains(&offset) && offset.is_multiple_of(4))
            .then(|| (offset / 4 - 1) as usize)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, read, write};
    #[test]
    fn reset_words_retain_guest_configuration_and_do_not_claim_cpu_or_clock_effects() {
        let mut host = TestHost::new();
        let mut reset = Reset::new(Options {}, Wiring).unwrap();
        write(&mut reset, &mut host, BASE + 0x14, 0x120);
        assert_eq!(read(&mut reset, &mut host, BASE + 0x14), 0x120);
        let saved = reset.save();
        for (address, size) in [(BASE, 4), (BASE + 0x18, 4), (BASE + 4, 2), (0xfff5102c, 2)] {
            let mut cx = host.cx();
            reset.read(&mut cx, address, size);
            assert!(cx.is_unmodeled());
        }
        reset.restore(&saved).unwrap();
        assert_eq!(reset.save(), saved);
        assert_eq!(reset.next_deadline(0), None);
        assert!(host.effects.is_empty());
    }
}
impl Device for Reset {
    fujimod_emulator_devices::checkpoint_fields!(state: State);

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "reset-control"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match Self::index(address, size) {
            Some(i) => self.state.registers[i].into(),
            None => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(i) = Self::index(address, size) else {
            cx.unmodeled();
            return;
        };
        let Ok(word) = u32::try_from(value) else {
            cx.fail("reset word exceeds 32 bits");
            return;
        };
        self.state.registers[i] = word;
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
