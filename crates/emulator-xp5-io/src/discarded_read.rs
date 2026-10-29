//! A single word whose reads drain bus traffic and discard the value.
use fujimod_emulator_devices::{Cx, Device};

pub struct DiscardedRead {
    base: u64,
}

impl DiscardedRead {
    pub fn new(base: u64, size: u64) -> Result<Self, String> {
        if size != 4 {
            return Err("discarded-read window must be 4 bytes".into());
        }
        Ok(Self { base })
    }
}

impl Device for DiscardedRead {
    fn name(&self) -> &'static str {
        "discarded-read"
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if address != self.base || address & 3 != 0 || size != 4 {
            cx.unmodeled();
        }
        0
    }

    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
        cx.unmodeled();
    }
}
