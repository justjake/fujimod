//! XP5 input-capture banks.
//! Only disabled/reset and idle readback are supported. Active capture
//! configuration refuses; no samples, cause bits or deadlines are invented.
use fujimod_emulator_devices::{dma::{self, Direction}, Cx, Device};

pub struct TransmitSink;
impl dma::Port for TransmitSink {
    fn request(&self, direction: Direction, _burst: bool) -> Option<u64> {
        (direction == Direction::Transmit).then_some(0)
    }
    fn receive(&mut self, _now: u64, _bytes: u8) -> Result<u64, String> {
        Err("input-capture transmit sink cannot receive".into())
    }
    fn transmit(&mut self, _now: u64, _bytes: u8, _value: u64) -> Result<(), String> {
        // Immediate acceptance with no downstream effect is a session assumption.
        Ok(())
    }
}

pub struct InputCapture {
    base: u64,
    channels: u64,
}
impl InputCapture {
    pub fn new(base: u64) -> Result<Self, String> {
        let channels = match base {
            0xff928000 => 5,
            0xff929000 => 3,
            _ => return Err("invalid input-capture bank".into()),
        };
        Ok(Self { base, channels })
    }
    fn readable(&self, address: u64, size: u8) -> bool {
        let offset = address.wrapping_sub(self.base);
        size == 4 && offset / 0x20 < self.channels && matches!(offset % 0x20, 0 | 4 | 0xc | 0x10)
    }
}
impl Device for InputCapture {
    fn name(&self) -> &'static str {
        "input-capture"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if !self.readable(address, size) {
            cx.unmodeled();
        }
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(self.base);
        if size != 4 || offset / 0x20 >= self.channels {
            cx.unmodeled();
            return;
        }
        match offset % 0x20 {
            0 if value == 0 => {}
            0xc if value & !0x10001 == 0 => {}
            _ => cx.unmodeled(),
        }
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        self.readable(address, size)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    #[test]
    fn bank_bounds_idle_readback_and_active_capture_refusal() {
        let mut host = RecordingHost::default();
        for (base, channels) in [(0xff928000, 5), (0xff929000, 3)] {
            let mut capture = InputCapture::new(base).unwrap();
            for channel in 0..channels {
                let mut cx = Cx::new(&mut host, 4, 0, 0);
                let at = base + channel * 0x20;
                capture.write(&mut cx, at, 4, 0);
                capture.write(&mut cx, at + 0xc, 4, 0x10001);
                for offset in [0, 4, 0xc, 0x10] {
                    assert_eq!(capture.read(&mut cx, at + offset, 4), 0);
                }
                assert_eq!(cx.refusal(), None);
            }
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            capture.read(&mut cx, base + channels * 0x20, 4);
            assert!(cx.is_unmodeled());
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            capture.write(&mut cx, base, 4, 1);
            assert!(cx.is_unmodeled());
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            capture.read(&mut cx, base, 8);
            assert!(cx.is_unmodeled());
            assert_eq!(capture.next_deadline(0), None);
            assert!(capture.save().is_null());
            capture.restore(&serde_json::Value::Null).unwrap();
        }
        assert!(host.events.is_empty());
        assert!(host.irqs.is_empty());
    }
}
