//! XP5 input-capture banks.
//! Only disabled/reset and idle readback are supported. Active capture
//! configuration refuses; no samples, cause bits or deadlines are invented.
//! The bank control word at +0xa0 holds eight bits that the firmware sets and
//! clears one at a time by read-modify-write; the model stores and returns
//! them without attaching any effect.
use fujimod_emulator_devices::{dma::{self, Direction}, Cx, Device};
use serde_json::{json, Value};

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

/// Bank control word; the firmware addresses bits 0..7 of it.
const CONTROL: u64 = 0xa0;
const CONTROL_BITS: u32 = 0xff;

pub struct InputCapture {
    base: u64,
    channels: u64,
    /// +0xa0. Reset value zero is assumed, like the per-channel words.
    control: u32,
}
impl InputCapture {
    pub fn new(base: u64) -> Result<Self, String> {
        let channels = match base {
            0xff928000 => 5,
            0xff929000 => 3,
            _ => return Err("invalid input-capture bank".into()),
        };
        Ok(Self { base, channels, control: 0 })
    }
    fn readable(&self, address: u64, size: u8) -> bool {
        let offset = address.wrapping_sub(self.base);
        size == 4
            && (offset == CONTROL
                || offset / 0x20 < self.channels && matches!(offset % 0x20, 0 | 4 | 0xc | 0x10))
    }
    fn validate_control(&self, control: &u32) -> Result<(), String> {
        if control & !CONTROL_BITS != 0 {
            return Err("input-capture control has bits above 7".into());
        }
        Ok(())
    }
}
impl Device for InputCapture {
    fujimod_emulator_devices::checkpoint_state!(control: u32, Self::validate_control);
    fn name(&self) -> &'static str {
        "input-capture"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if !self.readable(address, size) {
            cx.unmodeled();
            return 0;
        }
        if address.wrapping_sub(self.base) == CONTROL {
            return u64::from(self.control);
        }
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(self.base);
        if size == 4 && offset == CONTROL && value & !u64::from(CONTROL_BITS) == 0 {
            self.control = value as u32;
            return;
        }
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
    fn save(&self) -> Value {
        json!({ "control": self.control })
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        let control = state
            .get("control")
            .and_then(Value::as_u64)
            .and_then(|control| u32::try_from(control).ok())
            .ok_or("invalid input-capture state")?;
        self.validate_control(&control)?;
        self.control = control;
        Ok(())
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
            // Bank 0's first word past its channels is the control word.
            capture.read(&mut cx, base + channels * 0x20 + 4, 4);
            assert!(cx.is_unmodeled());
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            capture.write(&mut cx, base, 4, 1);
            assert!(cx.is_unmodeled());
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            capture.read(&mut cx, base, 8);
            assert!(cx.is_unmodeled());
            assert_eq!(capture.next_deadline(0), None);
            assert_eq!(capture.save(), json!({ "control": 0 }));
        }
        assert!(host.events.is_empty());
        assert!(host.irqs.is_empty());
    }
    #[test]
    fn bank_control_bits_read_back_and_checkpoint() {
        let mut host = RecordingHost::default();
        for base in [0xff928000, 0xff929000] {
            let mut capture = InputCapture::new(base).unwrap();
            let control = base + CONTROL;
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            assert_eq!(capture.read(&mut cx, control, 4), 0);
            // Set bit 7 and bit 0, then clear bit 7, as single-bit
            // read-modify-writes.
            for bit in [7, 0] {
                let value = capture.read(&mut cx, control, 4) | 1 << bit;
                capture.write(&mut cx, control, 4, value);
            }
            let value = capture.read(&mut cx, control, 4) & !(1 << 7);
            capture.write(&mut cx, control, 4, value);
            assert_eq!(capture.read(&mut cx, control, 4), 1);
            assert!(capture.pollable(control, 4));
            assert_eq!(cx.refusal(), None);
            assert!(!cx.is_unmodeled());

            let saved = capture.checkpoint().unwrap();
            let mut restored = InputCapture::new(base).unwrap();
            restored.restore_checkpoint(&saved).unwrap();
            assert_eq!(restored.read(&mut cx, control, 4), 1);
            assert_eq!(capture.save(), json!({ "control": 1 }));
            restored.restore(&json!({ "control": 0x80 })).unwrap();
            assert_eq!(restored.read(&mut cx, control, 4), 0x80);
            assert!(restored.restore(&json!({ "control": 0x100 })).is_err());

            for (address, size, value) in
                [(control, 4, 0x100), (control, 2, 1), (control + 4, 4, 0), (control - 4, 4, 0)]
            {
                let mut cx = Cx::new(&mut host, 0, 0, 0);
                capture.write(&mut cx, address, size, value);
                assert!(cx.is_unmodeled(), "{address:#x}/{size}={value:#x}");
            }
            for (address, size) in [(control, 8), (control + 4, 4)] {
                let mut cx = Cx::new(&mut host, 0, 0, 0);
                capture.read(&mut cx, address, size);
                assert!(cx.is_unmodeled(), "{address:#x}/{size}");
            }
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            assert_eq!(capture.read(&mut cx, control, 4), 1);
        }
        assert!(host.events.is_empty());
        assert!(host.irqs.is_empty());
    }
}
