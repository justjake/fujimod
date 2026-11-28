//! A single word whose reads drain bus traffic and discard the value.
use fujimod_emulator_devices::bus::FastReadWord;
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::{Cx, Device};
use std::sync::{Arc, atomic::AtomicU32};

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
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), &())
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        saved.decode(self.name())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)
    }
    fn name(&self) -> &'static str {
        "discarded-read"
    }

    fn fast_reads(&self, window_base: u64, window_size: u64) -> Vec<FastReadWord> {
        if self.base & 3 == 0 && window_base == self.base && window_size == 4 {
            vec![FastReadWord::atomic_u32(0, Arc::new(AtomicU32::new(0)))]
        } else {
            Vec::new()
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::bus::FastReadValue;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    use std::sync::atomic::Ordering;

    #[test]
    fn fast_reads_publish_zero_only_for_exact_aligned_window() {
        let base = 0xff90_1000;
        let mut device = DiscardedRead::new(base, 4).unwrap();
        let words = device.fast_reads(base, 4);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].offset, 0);
        let FastReadValue::AtomicU32(atom) = &words[0].value else {
            panic!("expected AtomicU32");
        };
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        assert_eq!(u64::from(atom.load(Ordering::Acquire)), device.read(&mut cx, base, 4));
        assert!(!cx.is_unmodeled());

        assert!(device.fast_reads(base + 4, 4).is_empty());
        assert!(device.fast_reads(base, 8).is_empty());

        let unaligned = DiscardedRead::new(base + 2, 4).unwrap();
        assert!(unaligned.fast_reads(base + 2, 4).is_empty());

        let checkpoint = device.checkpoint().unwrap();
        device.restore_checkpoint(&checkpoint).unwrap();
        assert_eq!(atom.load(Ordering::Acquire), 0);
    }
}
