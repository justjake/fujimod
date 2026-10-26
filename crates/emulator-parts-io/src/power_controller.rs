//! Four-byte, MSB-first power-controller protocol with explicit checksum and
//! register effects.
//! This is an emulated register file, not an identified external PMIC.
use serde::{Deserialize, Serialize};

impl super::serial::Endpoint for PowerController {
    fn name(&self) -> &'static str {
        "power-controller"
    }
    fn validate(&self, frame: super::serial::Frame) -> Result<(), String> {
        if frame.bits != 8 || frame.mode != 3 {
            Err("power controller requires an 8-bit mode3 frame".into())
        } else {
            Ok(())
        }
    }
    fn reset_transaction(&mut self) {
        self.reset_transfer();
    }
    fn transfer(&mut self, frame: super::serial::Frame, now_ns: u64) -> u16 {
        self.advance(now_ns);
        self.exchange(frame.word as u8).into()
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap()
    }
    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let decoded: Self = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if decoded.registers.len() != 128 || decoded.position >= 4 {
            return Err("invalid power controller checkpoint".into());
        }
        *self = decoded;
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Rtc {
    seconds: u32,
    anchor_ns: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PowerController {
    registers: Vec<u16>,
    request: [u8; 4],
    position: usize,
    reply: u16,
    pub bad_checksums: u64,
    #[serde(default)]
    rtc: Option<Rtc>,
    #[serde(default)]
    now_ns: u64,
}
impl Default for PowerController {
    fn default() -> Self {
        Self::new()
    }
}
impl PowerController {
    pub fn new() -> Self {
        let mut registers = vec![0; 128];
        registers[0] = 0x200; // Default active power state; bit 10 is clear.
        registers[3] = 600; // Declared battery-voltage sample.
        registers[7] = 500; // Declared thermistor sample.
        registers[0xe] = 1; // Declared installed-battery state.
        Self {
            registers,
            request: [0; 4],
            position: 0,
            reply: 0,
            bad_checksums: 0,
            rtc: None,
            now_ns: 0,
        }
    }
    pub fn reset_transfer(&mut self) {
        self.position = 0;
    }
    /// Seconds since 2000-01-01, advanced by the VM clock. Invalid/unset
    /// profiles retain bit1 clear until a calendar is committed.
    pub fn configure_rtc(&mut self, seconds: u32, valid: bool) {
        self.rtc = Some(Rtc {
            seconds,
            anchor_ns: self.now_ns,
        });
        self.registers[0x27] = (self.registers[0x27] & !2) | if valid { 2 } else { 0 };
        self.latch_rtc();
    }
    pub fn advance(&mut self, now_ns: u64) {
        self.now_ns = now_ns;
    }
    fn latch_rtc(&mut self) {
        if let Some(rtc) = &self.rtc {
            let seconds = rtc
                .seconds
                .wrapping_add((self.now_ns.saturating_sub(rtc.anchor_ns) / 1_000_000_000) as u32);
            self.registers[0x23] = seconds as u16;
            self.registers[0x24] = (seconds >> 16) as u16;
        }
    }
    pub fn register(&self, address: u8) -> u16 {
        self.registers[(address & 0x7f) as usize]
    }
    pub fn set_register(&mut self, address: u8, value: u16) {
        let address = (address & 0x7f) as usize;
        self.registers[address] = value;
        if address == 0x27 && value & 0x200 != 0 {
            let seconds = self.registers[0x1d] as u32 | ((self.registers[0x1e] as u32) << 16);
            self.rtc = Some(Rtc {
                seconds,
                anchor_ns: self.now_ns,
            });
            self.latch_rtc();
        }
    }
    pub fn exchange(&mut self, byte: u8) -> u8 {
        let n = self.position;
        self.request[n] = byte;
        let command = self.request[0];
        let write = command & 0x80 != 0;
        if n == 0 {
            // Reading the low half latches both halves, preventing a rollover
            // between transfers from tearing the value.
            if !write && command & 0x7f == 0x23 {
                self.latch_rtc();
            }
            self.reply = self.register(command);
        }
        let response = match n {
            0 => command,
            1 => (self.reply >> 8) as u8,
            2 => self.reply as u8,
            3 => {
                let expected = !(command
                    .wrapping_add(self.request[1])
                    .wrapping_add(self.request[2]));
                if byte != expected {
                    self.bad_checksums += 1;
                    // Deliberately fail the driver's checksum comparison.
                    if write {
                        !byte
                    } else {
                        !command
                            .wrapping_add((self.reply >> 8) as u8)
                            .wrapping_add(self.reply as u8)
                            ^ 1
                    }
                } else if write {
                    self.set_register(
                        command,
                        u16::from_be_bytes([self.request[1], self.request[2]]),
                    );
                    expected
                } else {
                    !command
                        .wrapping_add((self.reply >> 8) as u8)
                        .wrapping_add(self.reply as u8)
                }
            }
            _ => unreachable!(),
        };
        self.position = (n + 1) & 3;
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn byte_order_checksum_and_rejected_write() {
        let mut m = PowerController::new();
        let reply: Vec<_> = [3, 0, 0, !3].map(|b| m.exchange(b)).into();
        assert_eq!(reply, [3, 2, 88, !93]);
        assert_eq!([0x92, 0x12, 0x34, !0xd8].map(|b| m.exchange(b))[3], !0xd8);
        assert_eq!(m.register(0x12), 0x1234);
        let _ = [0x92, 0xab, 0xcd, 0].map(|b| m.exchange(b));
        assert_eq!(m.register(0x12), 0x1234);
        assert_eq!(m.bad_checksums, 1);
    }
    #[test]
    fn rtc_commit_rollover_and_checkpoint() {
        fn read(m: &mut PowerController, r: u8) -> u16 {
            let b = [r, 0, 0, !r].map(|v| m.exchange(v));
            u16::from_be_bytes([b[1], b[2]])
        }
        fn write(m: &mut PowerController, r: u8, value: u16) {
            let cmd = r | 0x80;
            let [hi, lo] = value.to_be_bytes();
            for byte in [cmd, hi, lo, !cmd.wrapping_add(hi).wrapping_add(lo)] {
                m.exchange(byte);
            }
        }
        let mut m = PowerController::new();
        m.configure_rtc(0xffff, false);
        assert_eq!(read(&mut m, 0x27) & 2, 0);
        assert_eq!(read(&mut m, 0x23), 0xffff);
        m.advance(1_000_000_000);
        assert_eq!(read(&mut m, 0x24), 0); // same latched read, despite rollover
        assert_eq!(read(&mut m, 0x23), 0);
        assert_eq!(read(&mut m, 0x24), 1);
        write(&mut m, 0x1d, 0x1234);
        write(&mut m, 0x1e, 0x5678);
        assert_eq!(read(&mut m, 0x23), 0); // staging has no effect
        write(&mut m, 0x27, 0x202);
        assert_eq!(read(&mut m, 0x23), 0x1234);
        assert_eq!(read(&mut m, 0x24), 0x5678);
        assert_eq!(read(&mut m, 0x27) & 2, 2);
        let saved = serde_json::to_value(&m).unwrap();
        let mut restored: PowerController = serde_json::from_value(saved).unwrap();
        restored.advance(3_000_000_000);
        assert_eq!(read(&mut restored, 0x23), 0x1236);
        assert_eq!(read(&mut restored, 0x24), 0x5678);
        write(&mut restored, 0x27, 3); // retained sleep packet, no RTC commit
        assert_eq!(read(&mut restored, 0x23), 0x1236);
    }
    #[test]
    fn rtc_uses_only_elapsed_guest_time_after_restore() {
        let mut m = PowerController::new();
        m.configure_rtc(700, true);
        m.advance(1_000_000_000);
        m.set_register(0x1d, 0x1234);
        m.set_register(0x1e, 0x5678);
        m.set_register(0x27, 0x202);
        let saved = serde_json::to_value(&m).unwrap();
        let mut restored: PowerController = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(serde_json::to_value(&restored).unwrap(), saved);
        restored.advance(1_000_000_000 + 5_000_000_000);
        restored.latch_rtc();
        let value = restored.register(0x23) as u32 | ((restored.register(0x24) as u32) << 16);
        assert_eq!(value, 0x56781234 + 5);
        assert_eq!(restored.register(0x27) & 2, 2);
    }
}

/// Battery-gauge endpoint on I2C1, address 0x36. Register 4 holds a declared
/// charge word read most-significant byte first. No chip identity or behavior
/// for unmodeled registers is implied; I2C controller behavior is separate.
#[derive(Clone, Serialize, Deserialize)]
pub struct FuelGauge {
    registers: Vec<u8>,
    known: Vec<bool>,
}
impl Default for FuelGauge {
    fn default() -> Self {
        Self::new()
    }
}
impl FuelGauge {
    pub const ADDRESS: u8 = 0x36;
    pub fn new() -> Self {
        let mut s = Self {
            registers: vec![0; 256],
            known: vec![false; 256],
        };
        // Declared Q8.8 charge sample.
        s.measurement(4, 0x5f00);
        // Declared retained OCV sample; no physical calibration is implied.
        s.measurement(0xe, 0xd000);
        s
    }
    fn measurement(&mut self, register: u8, value: u16) {
        let n = register as usize;
        self.registers[n..n + 2].copy_from_slice(&value.to_be_bytes());
        self.known[n..n + 2].fill(true);
    }
    pub fn read(&self, register: u8) -> Result<u8, String> {
        let n = register as usize;
        if self.known[n] {
            Ok(self.registers[n])
        } else {
            Err(format!("unmodeled battery-gauge register {register:#x}"))
        }
    }
    pub fn write(&mut self, register: u8, value: u8) -> Result<(), String> {
        // SOC is read-only in this model; reject writes to its two bytes.
        if matches!(register, 4 | 5) {
            return Err("unmodeled battery-gauge SOC write".into());
        }
        self.registers[register as usize] = value;
        self.known[register as usize] = true;
        Ok(())
    }
    /// Read sequential big-endian bytes starting at the selected register.
    /// Unmodeled registers remain unsupported by `read`.
    pub fn read_bytes(&self, register: u8, length: usize) -> Result<Vec<u8>, String> {
        if length > 256 {
            return Err("battery-gauge transfer exceeds register file".into());
        }
        (0..length)
            .map(|n| self.read(register.wrapping_add(n as u8)))
            .collect()
    }
    pub fn write_bytes(&mut self, register: u8, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > 256 {
            return Err("battery-gauge transfer exceeds register file".into());
        }
        if (0..bytes.len()).any(|n| matches!(register.wrapping_add(n as u8), 4 | 5)) {
            return Err("unmodeled battery-gauge SOC write".into());
        }
        for (n, value) in bytes.iter().enumerate() {
            self.write(register.wrapping_add(n as u8), *value)?;
        }
        Ok(())
    }
}
