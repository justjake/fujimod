//! Micron-compatible serial NOR control registers, independent of array data.
//! Only status, write-enable and enhanced volatile configuration commands are
//! supported; array commands and other register protocols refuse explicitly.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub enhanced_volatile_reset: u8,
    pub write_ticks: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registers {
    options: Options,
    enhanced_volatile: u8,
    write_enable: bool,
    pending: Option<(u64, u8)>,
}

impl Registers {
    pub fn new(options: Options) -> Result<Self, String> {
        if options.write_ticks == 0 {
            return Err("Serial NOR register write latency must be positive".into());
        }
        Ok(Self {
            enhanced_volatile: options.enhanced_volatile_reset,
            options,
            write_enable: false,
            pending: None,
        })
    }
    pub fn advance(&mut self, now: u64) {
        if let Some((due, value)) = self.pending
            && due <= now
        {
            self.enhanced_volatile = value;
            self.pending = None;
        }
    }
    pub fn next_deadline(&self) -> Option<u64> {
        self.pending.map(|(due, _)| due)
    }
    pub fn validate_restore(&self, saved: &Self) -> Result<(), String> {
        if self.options != saved.options || (saved.pending.is_some() && saved.write_enable) {
            return Err("Serial NOR register checkpoint differs or is inconsistent".into());
        }
        Ok(())
    }
    pub fn command(
        &mut self,
        now: u64,
        opcode: u8,
        transmit: &[u8],
        receive: usize,
    ) -> Result<Vec<u8>, String> {
        self.advance(now);
        if self.pending.is_some() && opcode != 0x05 {
            return Err("Serial NOR is busy writing a register".into());
        }
        match (opcode, transmit, receive) {
            (0x05, [], 1) => Ok(vec![
                u8::from(self.pending.is_some()) | (u8::from(self.write_enable) << 1),
            ]),
            (0x06, [], 0) => {
                self.write_enable = true;
                Ok(vec![])
            }
            (0x04, [], 0) => {
                self.write_enable = false;
                Ok(vec![])
            }
            (0x65, [], 1) => Ok(vec![self.enhanced_volatile]),
            (0x61, [value], 0) => {
                if !self.write_enable {
                    return Err("Serial NOR register write requires write-enable".into());
                }
                let due = now
                    .checked_add(self.options.write_ticks)
                    .ok_or("Serial NOR register write deadline overflow")?;
                self.write_enable = false;
                self.pending = Some((due, *value));
                Ok(vec![])
            }
            _ => Err(format!(
                "Unsupported serial NOR command {opcode:#04x}, {} transmitted and {receive} received bytes",
                transmit.len()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn write_enable_busy_deadline_and_configuration_round_trip() {
        let mut device = Registers::new(Options {
            enhanced_volatile_reset: 0xff,
            write_ticks: 7,
        })
        .unwrap();
        assert!(device.command(0, 0x61, &[0xf5], 0).is_err());
        device.command(0, 0x06, &[], 0).unwrap();
        assert_eq!(device.command(0, 0x05, &[], 1).unwrap(), [2]);
        device.command(3, 0x61, &[0xf5], 0).unwrap();
        assert_eq!(device.command(9, 0x05, &[], 1).unwrap(), [1]);
        assert!(device.command(9, 0x65, &[], 1).is_err());
        let saved = serde_json::to_value(&device).unwrap();
        assert_eq!(device.command(10, 0x05, &[], 1).unwrap(), [0]);
        assert_eq!(device.command(10, 0x65, &[], 1).unwrap(), [0xf5]);
        let resumed: Registers = serde_json::from_value(saved).unwrap();
        device.validate_restore(&resumed).unwrap();
        device = resumed;
        assert_eq!(device.next_deadline(), Some(10));
        assert_eq!(device.command(10, 0x65, &[], 1).unwrap(), [0xf5]);
        assert!(device.command(10, 0x03, &[], 1).is_err());
    }
}
