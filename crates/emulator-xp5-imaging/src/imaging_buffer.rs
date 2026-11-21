//! XP5 imaging input-buffer (IBUF) register page: a bank of plain 32-bit words and one ready bit.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const BASE: u64 = 0xff945000;

/// Latency of the functional ready handshake; silicon timing is unknown.
const READY_LATENCY_NS: u64 = 100_000;

/// Words the stock buffer setters, getters and start/stop routines access, one 32-bit
/// load or store each: control (+0x00, bit 0 is the ready bit), +0x04 and +0x08, the size pairs
/// +0x0c/+0x10, +0x14/+0x18 and +0x20/+0x24 (the setters refuse a zero or negative dimension
/// before storing), mode +0x1c, enable +0x40 (bit 0), the address words +0x44 to +0x5c, +0x60
/// and +0x70, and the request words +0x68 and +0x6c.
fn known(offset: u64) -> bool {
    offset % 4 == 0 && matches!(offset, 0..=0x24 | 0x40..=0x70)
}

/// Plain word bank. Every word reads back what was last written, from zero at reset. Bit 0 of
/// +0x00 is the one behavior: the stock stop sequence clears it and then polls until it reads
/// set, so a write that leaves it clear sets it again after a functional latency.
#[derive(Default, Serialize, Deserialize)]
pub struct Buffer {
    words: BTreeMap<u64, u32>,
    #[serde(default)]
    generation: u64,
    #[serde(default)]
    started: bool,
    deadline: Option<u64>,
}
impl Device for Buffer {
    fn name(&self) -> &'static str {
        "imaging-buffer"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(BASE);
        self.advance(cx);
        if size != 4 || !known(offset) {
            cx.unmodeled();
            return 0;
        }
        let word = self.words.get(&offset).copied().unwrap_or(0);
        // Before the first control write the buffer is idle, which reads as ready.
        let ready = u32::from(offset == 0 && !self.started);
        u64::from(word | ready)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(BASE);
        self.advance(cx);
        if size != 4 || !known(offset) {
            cx.unmodeled();
            return;
        }
        let value = value as u32;
        if offset == 0 {
            self.started = true;
            self.generation = self.generation.wrapping_add(1);
            self.deadline = None;
            if value & 1 == 0 {
                let at = cx.now() + READY_LATENCY_NS;
                self.deadline = Some(at);
                cx.schedule(at, self.generation);
            }
        }
        self.words.insert(offset, value);
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token == self.generation {
            self.advance(cx);
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.deadline
    }
    fujimod_emulator_devices::checkpoint_fields!(words: BTreeMap<u64, u32>, generation: u64, started: bool, deadline: Option<u64>);
    fn save(&self) -> Value {
        serde_json::to_value(self).unwrap()
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        *self = serde_json::from_value(value.clone())
            .map_err(|e| format!("input buffer checkpoint: {e}"))?;
        Ok(())
    }
}
impl Buffer {
    fn advance(&mut self, cx: &Cx<'_>) {
        if self.deadline.is_some_and(|at| at <= cx.now()) {
            self.deadline = None;
            *self.words.entry(0).or_default() |= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    fn write(b: &mut Buffer, host: &mut RecordingHost, o: u64, v: u64) {
        b.write(&mut Cx::new(host, 0, 0, 0), BASE + o, 4, v);
    }
    fn read(b: &mut Buffer, host: &mut RecordingHost, o: u64) -> u64 {
        b.read(&mut Cx::new(host, 0, 0, 0), BASE + o, 4)
    }
    #[test]
    fn size_words_read_back_unchanged() {
        let mut b = Buffer::default();
        let mut host = RecordingHost::default();
        for (o, v) in [(0x14, 0x0e1c_1234), (0x18, 0x0e1c_0abc), (0x44, 0xdead_beef), (0x5c, 7), (0x6c, 1)] {
            write(&mut b, &mut host, o, v);
            assert_eq!(read(&mut b, &mut host, o), v);
        }
    }
    #[test]
    fn the_control_ready_bit_returns_after_a_clearing_write() {
        let mut b = Buffer::default();
        let mut host = RecordingHost::default();
        assert_eq!(read(&mut b, &mut host, 0) & 1, 1);
        write(&mut b, &mut host, 0, 0);
        assert_eq!(read(&mut b, &mut host, 0) & 1, 0);
        host.now = READY_LATENCY_NS - 1;
        assert_eq!(read(&mut b, &mut host, 0) & 1, 0);
        host.now = READY_LATENCY_NS;
        assert_eq!(read(&mut b, &mut host, 0) & 1, 1);
        write(&mut b, &mut host, 0, 1);
        assert_eq!(read(&mut b, &mut host, 0), 1);
    }
    #[test]
    fn an_unlisted_word_is_unmodeled() {
        assert!(!known(0x28) && !known(0x74) && !known(0x2) && !known(0x1000));
        assert!(known(0x70) && known(0x24) && known(0x40));
    }
}
