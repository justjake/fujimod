//! Power-controller wake countdown and SoC supply-hold timing.
//!
//! The owner delivers register writes and hold levels at virtual ticks. A
//! pending wake tells the board owner to reconstruct the powered system while
//! retaining its external media and the controller's persistent state.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Supply {
    On,
    Off,
    Wake,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WakeAlarm {
    ticks_per_second: u64,
    hold_release_ticks: u64,
    seconds: [u8; 2],
    packet: Option<[u8; 16]>,
    alarm_due: Option<u64>,
    release_due: Option<u64>,
    supply: Supply,
    now: u64,
}
impl WakeAlarm {
    pub fn new(ticks_per_second: u64, hold_release_ticks: u64) -> Result<Self, String> {
        if ticks_per_second == 0 || hold_release_ticks == 0 {
            return Err("power timing must be positive".into());
        }
        Ok(Self {
            ticks_per_second,
            hold_release_ticks,
            seconds: [0; 2],
            packet: None,
            alarm_due: None,
            release_due: None,
            supply: Supply::On,
            now: 0,
        })
    }
    /// The controller's persistent packet, when its complete register bank is wired.
    pub fn packet(&self) -> Option<[u8; 16]> {
        self.packet
    }
    pub fn initialize_packet(&mut self, packet: [u8; 16]) -> Result<(), String> {
        if self.packet.is_some() {
            return Err("power packet already initialized".into());
        }
        self.packet = Some(packet);
        Ok(())
    }
    pub fn write_packet(&mut self, at: u64, index: usize, value: u8) -> Result<(), String> {
        self.advance(at)?;
        let packet = self.packet.as_mut().ok_or("power packet is not wired")?;
        let byte = packet
            .get_mut(index)
            .ok_or("power packet offset out of range")?;
        *byte = value;
        Ok(())
    }
    pub fn supply(&self) -> Supply {
        self.supply
    }
    pub fn next_deadline(&self) -> Option<u64> {
        [self.alarm_due, self.release_due]
            .into_iter()
            .flatten()
            .min()
    }
    /// Updates the countdown registers; other registers belong to the part's
    /// register bank. The enabled countdown starts at this write's tick.
    pub fn write(&mut self, at: u64, register: u8, value: u8) -> Result<(), String> {
        self.advance(at)?;
        match register {
            0x69 => self.seconds[0] = value,
            0x6a => self.seconds[1] = value,
            0x68 if value == 0 => self.alarm_due = None,
            0x68 if value == 1 => {
                let seconds = u64::from(u16::from_le_bytes(self.seconds));
                if seconds == 0 {
                    return Err("zero wake countdown is unsupported".into());
                }
                let delay = seconds
                    .checked_mul(self.ticks_per_second)
                    .ok_or("wake countdown overflow")?;
                self.alarm_due = Some(at.checked_add(delay).ok_or("wake deadline overflow")?);
            }
            0x68 => return Err("unknown wake enable".into()),
            _ => return Err("not a wake register".into()),
        }
        Ok(())
    }
    pub fn hold(&mut self, at: u64, asserted: bool) -> Result<(), String> {
        self.advance(at)?;
        if self.supply != Supply::On {
            return Err("power hold changed while supply is off".into());
        }
        if asserted {
            self.release_due = None;
        } else if self.release_due.is_none() {
            self.release_due = Some(
                at.checked_add(self.hold_release_ticks)
                    .ok_or("power release overflow")?,
            );
        }
        Ok(())
    }
    /// Process release and alarm in deadline order, including a large jump.
    pub fn advance(&mut self, at: u64) -> Result<(), String> {
        if at < self.now {
            return Err("power time moved backward".into());
        }
        while let Some(due) = self.next_deadline() {
            if due > at {
                break;
            }
            if self.release_due == Some(due) {
                self.release_due = None;
                self.supply = Supply::Off;
            }
            if self.alarm_due == Some(due) {
                self.alarm_due = None;
                if self.supply == Supply::Off {
                    self.supply = Supply::Wake;
                }
            }
        }
        self.now = at;
        Ok(())
    }
    pub fn acknowledge_wake(&mut self) -> Result<(), String> {
        if self.supply != Supply::Wake {
            return Err("no power wake is pending".into());
        }
        self.supply = Supply::On;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn alarm_and_hold_produce_one_wake_on_virtual_time() {
        let mut p = WakeAlarm::new(12_000_000, 100_000).unwrap();
        p.write(20, 0x69, 1).unwrap();
        p.write(21, 0x6a, 0).unwrap();
        p.write(22, 0x68, 1).unwrap();
        p.hold(30, false).unwrap();
        p.advance(100_029).unwrap();
        assert_eq!(p.supply(), Supply::On);
        p.advance(100_030).unwrap();
        assert_eq!(p.supply(), Supply::Off);
        p.advance(12_000_021).unwrap();
        assert_eq!(p.supply(), Supply::Off);
        p.advance(12_000_022).unwrap();
        assert_eq!(p.supply(), Supply::Wake);
        p.acknowledge_wake().unwrap();
        assert_eq!(p.supply(), Supply::On);
        assert_eq!(p.next_deadline(), None);
    }
    #[test]
    fn hold_reassertion_and_disabled_alarm_do_not_restart() {
        let mut p = WakeAlarm::new(100, 10).unwrap();
        p.write(0, 0x69, 1).unwrap();
        p.write(0, 0x68, 1).unwrap();
        p.hold(1, false).unwrap();
        p.hold(9, true).unwrap();
        p.write(10, 0x68, 0).unwrap();
        p.advance(200).unwrap();
        assert_eq!(p.supply(), Supply::On);
        p.hold(200, false).unwrap();
        p.advance(500).unwrap();
        assert_eq!(p.supply(), Supply::Off);
        assert!(p.acknowledge_wake().is_err());
    }
    #[test]
    fn expired_alarm_and_invalid_inputs_do_not_fabricate_wake() {
        let mut p = WakeAlarm::new(10, 20).unwrap();
        assert!(p.write(0, 0x68, 1).is_err());
        p.write(0, 0x69, 1).unwrap();
        p.write(0, 0x68, 1).unwrap();
        p.hold(0, false).unwrap();
        p.advance(20).unwrap();
        assert_eq!(p.supply(), Supply::Off);
        assert!(p.advance(19).is_err());
    }
}
