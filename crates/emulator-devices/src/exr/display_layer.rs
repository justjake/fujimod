//! EXR Processor Pro display-layer register block (two instances, 0xFFF23000 and 0xFFF27000). Words +0x00..+0xFC latch. Bit 16 of +0x00 is a self-completing reset: writing it 0 starts the reset and it reads 1 again after the declared reset time (driver 0x88a0e8 writes 0x10000, clears bit 16 and waits for it). The camera's RAM shadow at 0xa436ec holds +0x00 = 0x10100 and a 640x480 window for instance 1. Scanout, layers and interrupts are not modeled.
use crate::{Cx, Device};
use serde::{Deserialize, Serialize};

/// Register window size of one display-layer instance.
pub const WINDOW: u64 = 0x100;
const WORDS: usize = WINDOW as usize / 4;
/// Self-completing reset bit of word +0x00.
const RESET: u32 = 1 << 16;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub base: u64,
    pub reset_ticks: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    #[serde(with = "crate::checkpoint::array")]
    registers: [u32; WORDS],
    due: Option<u64>,
}

/// One display-layer register window.
pub struct DisplayLayer {
    options: Options,
    state: State,
}

impl DisplayLayer {
    pub fn new(options: Options) -> Result<Self, String> {
        if options.base & 0xfff != 0 {
            return Err(format!(
                "EXR display-layer base {:#x} is not 0x1000-aligned",
                options.base
            ));
        }
        if options.reset_ticks == 0 {
            return Err("EXR display-layer reset needs positive time".into());
        }
        Ok(Self {
            options,
            state: State {
                registers: [0; WORDS],
                due: None,
            },
        })
    }

    fn index(&self, address: u64, size: u8) -> Option<usize> {
        let offset = address.checked_sub(self.options.base)?;
        if size != 4 || !offset.is_multiple_of(4) || offset >= WINDOW {
            return None;
        }
        Some((offset / 4) as usize)
    }
}

impl Device for DisplayLayer {
    crate::checkpoint_fields!(state: State);

    fn name(&self) -> &'static str {
        "exr-display-layer"
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match self.index(address, size) {
            Some(index) => self.state.registers[index].into(),
            None => {
                cx.unmodeled();
                0
            }
        }
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(index) = self.index(address, size) else {
            cx.unmodeled();
            return;
        };
        let Ok(word) = u32::try_from(value) else {
            cx.fail("EXR display-layer word exceeds 32 bits");
            return;
        };
        let due = if index == 0 && word & RESET == 0 {
            let Some(due) = cx.now().checked_add(self.options.reset_ticks) else {
                cx.fail("EXR display-layer reset deadline overflow");
                return;
            };
            Some(due)
        } else {
            None
        };
        self.state.registers[index] = word;
        if index == 0 {
            self.state.due = due;
        }
    }

    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.due
    }

    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.state.due.is_some_and(|due| due <= cx.now()) {
            self.state.registers[0] |= RESET;
            self.state.due = None;
        }
    }

    fn pollable(&self, address: u64, size: u8) -> bool {
        self.index(address, size).is_some()
    }

    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn save(&self) -> serde_json::Value {
        serde_json::to_value(&self.state).unwrap()
    }

    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state.due.is_some_and(|_| state.registers[0] & RESET != 0) {
            return Err("invalid EXR display-layer checkpoint".into());
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_host::{TestHost, advance, read, write};

    const BASE: u64 = 0xfff27000;

    #[test]
    fn reset_bit_completes_after_the_declared_time() {
        let mut model = DisplayLayer::new(Options {
            base: BASE,
            reset_ticks: 10,
        })
        .unwrap();
        let mut host = TestHost::new();
        host.now = 100;
        write(&mut model, &mut host, BASE, 0x1_0000);
        assert_eq!(read(&mut model, &mut host, BASE), 0x1_0000);
        write(&mut model, &mut host, BASE, 0);
        assert_eq!(read(&mut model, &mut host, BASE), 0);
        assert_eq!(model.next_deadline(100), Some(110));
        advance(&mut model, &mut host, 109);
        assert_eq!(read(&mut model, &mut host, BASE), 0);
        advance(&mut model, &mut host, 110);
        assert_eq!(read(&mut model, &mut host, BASE), 0x1_0000);
        assert_eq!(model.next_deadline(110), None);
        write(&mut model, &mut host, BASE + 0x78, 0x1234_5678);
        assert_eq!(read(&mut model, &mut host, BASE + 0x78), 0x1234_5678);
        assert!(!model.pollable(BASE + 0x100, 4));
        let mut cx = host.cx();
        assert_eq!(model.read(&mut cx, BASE + 0x100, 4), 0);
        assert!(cx.is_unmodeled());
    }

    #[test]
    fn options_are_validated() {
        assert!(
            DisplayLayer::new(Options {
                base: 0xfff27004,
                reset_ticks: 10,
            })
            .is_err()
        );
        assert!(
            DisplayLayer::new(Options {
                base: BASE,
                reset_ticks: 0,
            })
            .is_err()
        );
    }
}
