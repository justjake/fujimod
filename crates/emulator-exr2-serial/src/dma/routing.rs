//! EXR II request selector block at FFF98500.
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const CHANNELS: usize = 8;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Routing {
    pub(super) words: [u32; 2],
    pub(super) active: [bool; CHANNELS],
}
pub type SharedRouting = Arc<Mutex<Routing>>;
impl Routing {
    pub(super) fn selector(&self, channel: usize) -> u8 {
        ((self.words[channel / 4] >> ((channel % 4) * 8)) & 31) as u8
    }
    pub(super) fn write(&mut self, bank: usize, value: u32) -> Result<(), String> {
        if value & !0x1f1f1f1f != 0 {
            return Err("EXR II DMA routing reserved bits are unmodeled".into());
        }
        for lane in 0..4 {
            if self.active[bank * 4 + lane] && (self.words[bank] ^ value) & (31 << (lane * 8)) != 0
            {
                return Err("EXR II DMA request route changed while active".into());
            }
        }
        self.words[bank] = value;
        Ok(())
    }
}

/// Register view of the two four-channel request selector words.
pub struct Block98500 {
    base: u64,
    routing: SharedRouting,
}

#[derive(Deserialize, Serialize)]
struct Snapshot {
    base: u64,
    words: [u32; 2],
}

impl Block98500 {
    pub fn new(base: u64, routing: SharedRouting) -> Result<Self, String> {
        if base & 3 != 0 || base.checked_add(8).is_none() {
            return Err("invalid EXR II DMA routing window".into());
        }
        Ok(Self { base, routing })
    }
    fn bank(&self, address: u64, size: u8) -> Option<usize> {
        match (address.checked_sub(self.base)?, size) {
            (0, 4) => Some(0),
            (4, 4) => Some(1),
            _ => None,
        }
    }
}
impl Device for Block98500 {
    fn name(&self) -> &'static str {
        "exr2-dma-request-routing"
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(
            self.name(),
            &Snapshot {
                base: self.base,
                words: self.routing.lock().unwrap().words,
            },
        )
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let snapshot: Snapshot = saved.decode(self.name())?;
        if snapshot.base != self.base
            || snapshot.words.iter().any(|value| value & !0x1f1f1f1f != 0)
            || !saved.children().is_empty()
            || !saved.blobs().is_empty()
        {
            return Err("invalid EXR II DMA routing checkpoint".into());
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let snapshot: Snapshot = saved.decode(self.name())?;
        self.routing.lock().unwrap().words = snapshot.words;
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.save())
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(bank) = self.bank(address, size) else {
            cx.unmodeled();
            return 0;
        };
        self.routing.lock().unwrap().words[bank] as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(bank) = self.bank(address, size) else {
            cx.unmodeled();
            return;
        };
        if value > u32::MAX as u64 {
            cx.unmodeled();
            return;
        }
        if let Err(error) = self.routing.lock().unwrap().write(bank, value as u32) {
            cx.fail(error);
        }
    }
    fn save(&self) -> Value {
        json!({"base":self.base,"words":self.routing.lock().unwrap().words})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let words: [u32; 2] =
            serde_json::from_value(value["words"].clone()).map_err(|e| e.to_string())?;
        if value["base"] != self.base || words.iter().any(|v| v & !0x1f1f1f1f != 0) {
            return Err("invalid EXR II DMA routing checkpoint".into());
        }
        self.routing.lock().unwrap().words = words;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_restores_selector_words_and_validates_before_mutation() {
        let routing = Arc::new(Mutex::new(Routing::default()));
        let mut block = Block98500::new(0xfff98500, routing.clone()).unwrap();
        routing.lock().unwrap().words = [0x0504_0302, 0x0908_0706];
        let saved = block.checkpoint().unwrap();

        routing.lock().unwrap().words = [0x0101_0101, 0x0202_0202];
        block.restore_checkpoint(&saved).unwrap();
        assert_eq!(routing.lock().unwrap().words, [0x0504_0302, 0x0908_0706]);

        let invalid = Checkpoint::encode(
            block.name(),
            &Snapshot {
                base: 0xfff98500,
                words: [0x2020_2020, 0],
            },
        )
        .unwrap();
        assert!(block.validate_checkpoint(&invalid).is_err());
        assert!(block.restore_checkpoint(&invalid).is_err());
        assert_eq!(routing.lock().unwrap().words, [0x0504_0302, 0x0908_0706]);
        assert_eq!(block.fingerprint().unwrap(), block.save());
    }
}
