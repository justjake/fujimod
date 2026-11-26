//! XP5 imaging input-buffer (IBUF) register page: a bank of plain 32-bit words and one ready bit,
//! with an optional transfer completion (start +0x40 bit 0, interrupt 169).
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
    offset.is_multiple_of(4) && matches!(offset, 0..=0x24 | 0x40..=0x70)
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
    /// Transfer completion state; unused unless the manifest sets `completion_us`.
    #[serde(default)]
    transfer: Transfer,
    /// Device configuration from the manifest, not guest state: the functional transfer time.
    #[serde(skip)]
    completion_ns: Option<u64>,
}

/// INTID of the IBUF completion (X-T5 4.31 handler 0x138a928).
const COMPLETION_IRQ: u32 = 169;
const START: u64 = 0x40;
const IRQ_ENABLE: u64 = 0x68;
const PENDING: u64 = 0x6c;

/// The transfer a start bit runs. X-T5 4.31 movie live view re-arms IBUF once a frame (+0x40
/// bit 0, 0x138b538) and waits for the completion interrupt, which its handler 0x138a928 clears by
/// writing 1 to +0x6c, before the movie job reports done. The transfer time is functional.
#[derive(Default, Clone, PartialEq, Serialize, Deserialize)]
struct Transfer {
    pending: bool,
    irq_enable: bool,
    completed: u64,
    generation: u64,
    deadline: Option<u64>,
}
impl Buffer {
    /// Complete each +0x40 bit 0 start after `completion_ns`: the start bit clears, +0x6c bit 0
    /// latches and interrupt 169 is high while +0x68 bit 0 enables it. Without this option the
    /// start, enable and request words are plain words.
    pub fn with_completion(mut self, completion_ns: u64) -> Result<Self, String> {
        if completion_ns == 0 {
            return Err("invalid input-buffer completion time".into());
        }
        self.completion_ns = Some(completion_ns);
        Ok(self)
    }
    fn publish(&self, cx: &mut Cx<'_>) {
        cx.set_irq(
            COMPLETION_IRQ,
            self.transfer.pending && self.transfer.irq_enable,
        );
    }
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
        if self.completion_ns.is_some() {
            match offset {
                IRQ_ENABLE => return u64::from(self.transfer.irq_enable),
                PENDING => return u64::from(self.transfer.pending),
                _ => {}
            }
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
        if let Some(completion_ns) = self.completion_ns {
            match offset {
                // The reset handshake abandons a running transfer and its request.
                0 if value & 1 == 0 => {
                    self.transfer.deadline = None;
                    self.transfer.generation = self.transfer.generation.wrapping_add(1);
                    self.transfer.pending = false;
                    self.publish(cx);
                }
                START if value & 1 != 0 && self.transfer.deadline.is_none() => {
                    // The start can arrive while the reset handshake is still running; the
                    // completion then follows the end of the reset.
                    let from = self.deadline.unwrap_or(0).max(cx.now());
                    let Some(at) = from.checked_add(completion_ns) else {
                        cx.fail("input-buffer completion deadline overflow");
                        return;
                    };
                    self.transfer.generation = self.transfer.generation.wrapping_add(1);
                    self.transfer.deadline = Some(at);
                    cx.schedule(at, self.transfer.generation | (1 << 63));
                }
                IRQ_ENABLE => {
                    self.transfer.irq_enable = value & 1 != 0;
                    self.publish(cx);
                    return;
                }
                PENDING => {
                    self.transfer.pending &= value & 1 == 0;
                    self.publish(cx);
                    return;
                }
                _ => {}
            }
        }
        self.words.insert(offset, value);
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token == self.generation || token == self.transfer.generation | (1 << 63) {
            self.advance(cx);
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.deadline
            .into_iter()
            .chain(self.transfer.deadline)
            .min()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        Buffer::advance(self, cx);
    }
    fujimod_emulator_devices::checkpoint_fields!(words: BTreeMap<u64, u32>, generation: u64, started: bool, deadline: Option<u64>, #[serde(default)] transfer: Transfer);
    fn save(&self) -> Value {
        serde_json::to_value(self).unwrap()
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let completion_ns = self.completion_ns;
        *self = serde_json::from_value(value.clone())
            .map_err(|e| format!("input buffer checkpoint: {e}"))?;
        self.completion_ns = completion_ns;
        Ok(())
    }
}
impl Buffer {
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.deadline.is_some_and(|at| at <= cx.now()) {
            self.deadline = None;
            *self.words.entry(0).or_default() |= 1;
        }
        if self.transfer.deadline.is_some_and(|at| at <= cx.now()) {
            self.transfer.deadline = None;
            *self.words.entry(START).or_default() &= !1;
            self.transfer.completed += 1;
            self.transfer.pending = true;
            self.publish(cx);
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
        for (o, v) in [
            (0x14, 0x0e1c_1234),
            (0x18, 0x0e1c_0abc),
            (0x44, 0xdead_beef),
            (0x5c, 7),
            (0x6c, 1),
        ] {
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
    fn a_start_completes_once_with_a_level_interrupt_cleared_by_the_request_word() {
        assert!(Buffer::default().with_completion(0).is_err());
        let mut b = Buffer::default().with_completion(8_000_000).unwrap();
        let mut host = RecordingHost::default();
        // Open (X-T5 0x138b9e0): clear +0x6c, reset handshake on +0x00, destination, start.
        write(&mut b, &mut host, 0x6c, 1);
        write(&mut b, &mut host, 0x04, 0x1234);
        write(&mut b, &mut host, 0x00, 0);
        assert_eq!(read(&mut b, &mut host, 0), 0);
        host.now = READY_LATENCY_NS;
        assert_eq!(read(&mut b, &mut host, 0), 1);
        assert_eq!(read(&mut b, &mut host, 0x04), 0x1234);
        write(&mut b, &mut host, 0x50, 0x8e83_a350);
        write(&mut b, &mut host, 0x68, 1);
        let v = read(&mut b, &mut host, 0x40);
        write(&mut b, &mut host, 0x40, v | 1);
        assert_eq!(b.next_deadline(host.now), Some(8_100_000));
        assert!(!host.irqs.iter().any(|(i, on)| *i == 169 && *on));
        host.now = 8_100_000;
        Device::advance(&mut b, &mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(host.irqs.last(), Some(&(169, true)));
        assert_eq!(b.next_deadline(host.now), None);
        assert_eq!(read(&mut b, &mut host, 0x40), 0);
        assert_eq!(read(&mut b, &mut host, 0x6c), 1);
        // The handler writes 1 to +0x6c, then the next frame re-arms the transfer.
        write(&mut b, &mut host, 0x6c, 1);
        assert_eq!(host.irqs.last(), Some(&(169, false)));
        write(&mut b, &mut host, 0x40, 1);
        // Without +0x68 the completion latches but the line stays low.
        write(&mut b, &mut host, 0x68, 0);
        assert_eq!(b.next_deadline(host.now), Some(16_100_000));
        let saved = b.checkpoint().unwrap();
        let mut restored = Buffer::default().with_completion(8_000_000).unwrap();
        restored.restore_checkpoint(&saved).unwrap();
        host.now = 16_100_000;
        Device::advance(&mut restored, &mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(read(&mut restored, &mut host, 0x6c), 1);
        assert_eq!(restored.save()["transfer"]["completed"], 2);
        assert_eq!(host.irqs.last(), Some(&(169, false)));
    }
    #[test]
    fn a_start_arriving_during_the_reset_handshake_completes_after_it() {
        let mut b = Buffer::default().with_completion(1_000).unwrap();
        let mut host = RecordingHost::default();
        write(&mut b, &mut host, 0x00, 0);
        write(&mut b, &mut host, 0x40, 1);
        assert_eq!(b.next_deadline(0), Some(READY_LATENCY_NS));
        host.now = READY_LATENCY_NS;
        Device::advance(&mut b, &mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(b.next_deadline(host.now), Some(READY_LATENCY_NS + 1_000));
        // A later reset abandons the transfer.
        write(&mut b, &mut host, 0x00, 0);
        assert_eq!(b.next_deadline(host.now), Some(2 * READY_LATENCY_NS));
    }
    #[test]
    fn without_the_option_the_start_and_request_words_stay_plain() {
        let mut b = Buffer::default();
        let mut host = RecordingHost::default();
        write(&mut b, &mut host, 0x40, 1);
        write(&mut b, &mut host, 0x68, 1);
        assert_eq!(b.next_deadline(0), None);
        assert_eq!(read(&mut b, &mut host, 0x40), 1);
        assert!(host.irqs.is_empty());
    }
    #[test]
    fn an_unlisted_word_is_unmodeled() {
        assert!(!known(0x28) && !known(0x74) && !known(0x2) && !known(0x1000));
        assert!(known(0x70) && known(0x24) && known(0x40));
    }
}
