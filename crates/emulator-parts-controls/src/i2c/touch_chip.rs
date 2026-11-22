//! Byte-level I2C endpoint for a touch chip with 16-bit register addresses.
//!
//! The chip answers one seven-bit address. A write transaction starts with the
//! two register-address bytes (high byte first); further bytes are data for the
//! register at that pointer, which advances per byte. A read starts at the
//! pointer left by the last write. One register range returns the eleven-byte
//! two-contact report, produced by [`TouchController`]; one register returns a
//! heartbeat counter that changes on every read. Only configuration registers
//! named in the profile accept writes and nothing else is readable, so any other
//! access refuses without effect.
//!
//! The model consumes bytes, input calls and virtual time only. The owner
//! raises the input-ready interrupt named by the profile whenever
//! [`TouchChip::advance`] reports a ready edge.
use crate::touch_controller::TouchController;
use fujimod_emulator_devices::parts::touch_input::{ContactInput, Phase};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Length of one report, as produced by [`TouchController`].
pub const REPORT_BYTES: u16 = 11;

/// Construction data; each body supplies its own values.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Seven-bit bus address.
    pub address: u8,
    /// First register of the report.
    pub report_register: u16,
    /// Register that returns a counter changing on each read.
    pub heartbeat_register: u16,
    /// Register that gates reporting: nonzero enables, zero disables.
    pub mode_register: u16,
    /// Registers that accept writes, including the mode register.
    pub writable_registers: Vec<u16>,
    /// Slot IDs reported for contacts 0 and 1.
    pub contact_ids: [u8; 2],
    /// Interrupt line the owner raises for each input-ready edge.
    pub input_ready_irq: u32,
    /// Host virtual ticks per second; a positive multiple of 100.
    pub ticks_per_second: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct TouchChip {
    profile: Profile,
    controller: TouchController,
    registers: BTreeMap<u16, u8>,
    enabled: bool,
    initial_ready_pending: bool,
    heartbeat: u8,
    current: Option<[u8; 11]>,
    reads: u64,
    phase: TransferPhase,
    pointer: Option<u16>,
}

/// Position within the current bus transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
enum TransferPhase {
    Idle,
    Read,
    AddressHigh,
    AddressLow(u8),
    Data,
}

impl TouchChip {
    pub fn new(profile: Profile) -> Result<Self, String> {
        let controller = TouchController::new(profile.ticks_per_second)?
            .with_contact_ids(profile.contact_ids)?;
        if !(8..=0x77).contains(&profile.address)
            || !profile.writable_registers.contains(&profile.mode_register)
        {
            return Err("Invalid touch chip address or mode register".into());
        }
        let report_end = u32::from(profile.report_register) + u32::from(REPORT_BYTES);
        let overlaps = |register: u16| {
            (profile.report_register..).take(usize::from(REPORT_BYTES)).any(|r| r == register)
        };
        if report_end > 0x1_0000
            || overlaps(profile.heartbeat_register)
            || profile.writable_registers.iter().any(|r| overlaps(*r))
            || profile.writable_registers.contains(&profile.heartbeat_register)
        {
            return Err("Touch chip register ranges overlap".into());
        }
        Ok(Self {
            profile,
            controller,
            registers: BTreeMap::new(),
            enabled: false,
            initial_ready_pending: true,
            heartbeat: 0,
            current: None,
            reads: 0,
            phase: TransferPhase::Idle,
            pointer: None,
        })
    }

    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// Value last written to a configuration register.
    pub fn register(&self, register: u16) -> Option<u8> {
        self.registers.get(&register).copied()
    }

    /// Ticks between reports; an input should be spaced at least this far from
    /// a following input of the same contact for a tap to read as one.
    pub fn touch_period(&self) -> u64 {
        self.controller.period()
    }

    /// Check a contact script without queueing it.
    pub fn admit_touch(&self, now: u64, script: &[ContactInput]) -> Result<(), String> {
        self.controller.clone().inject(now, script)
    }

    /// Queue a contact script at absolute ticks.
    pub fn touch(&mut self, now: u64, script: &[ContactInput]) -> Result<(), String> {
        self.controller.inject(now, script)
    }

    pub fn press(&mut self, now: u64, x: u16, y: u16) -> Result<(), String> {
        self.input(now, Phase::Down, x, y)
    }

    pub fn move_to(&mut self, now: u64, x: u16, y: u16) -> Result<(), String> {
        self.input(now, Phase::Move, x, y)
    }

    pub fn release(&mut self, now: u64, x: u16, y: u16) -> Result<(), String> {
        self.input(now, Phase::Up, x, y)
    }

    fn input(&mut self, now: u64, phase: Phase, x: u16, y: u16) -> Result<(), String> {
        self.controller.inject(now, &[ContactInput { tick: now, contact: 0, phase, x, y }])
    }

    /// Earliest tick at which [`TouchChip::advance`] has work.
    pub fn next_deadline(&self, now: u64) -> Option<u64> {
        if self.initial_ready_pending {
            return Some(now);
        }
        self.controller.next_event_tick(now, self.reads, self.enabled)
    }

    /// Run due input edges. Returns true when the input-ready interrupt should
    /// be raised: once at power-up, then once per report made available.
    pub fn advance(&mut self, now: u64) -> Result<bool, String> {
        let mut ready = std::mem::take(&mut self.initial_ready_pending);
        if let Some(frame) = self.controller.tick(now, self.reads, self.enabled)? {
            self.current = Some(frame);
            ready = true;
        }
        Ok(ready)
    }

    /// Address phase; true when this chip acknowledges.
    pub fn address(&mut self, byte: u8) -> bool {
        let ack = byte >> 1 == self.profile.address;
        self.phase = match (ack, byte & 1) {
            (false, _) => TransferPhase::Idle,
            (true, 0) => TransferPhase::AddressHigh,
            (true, _) => TransferPhase::Read,
        };
        ack
    }

    /// Check one write byte without effect.
    pub fn check_write(&self) -> Result<(), String> {
        match self.phase {
            TransferPhase::AddressHigh | TransferPhase::AddressLow(_) => Ok(()),
            TransferPhase::Data => {
                let pointer = self.pointer.ok_or("Touch chip register pointer missing")?;
                if self.profile.writable_registers.contains(&pointer) {
                    Ok(())
                } else {
                    Err(format!("Touch chip register {pointer:#06x} is not writable"))
                }
            }
            TransferPhase::Idle | TransferPhase::Read => {
                Err("Touch chip write outside a write transaction".into())
            }
        }
    }

    pub fn write(&mut self, byte: u8) -> Result<(), String> {
        self.check_write()?;
        match self.phase {
            TransferPhase::AddressHigh => self.phase = TransferPhase::AddressLow(byte),
            TransferPhase::AddressLow(high) => {
                self.pointer = Some(u16::from(high) << 8 | u16::from(byte));
                self.phase = TransferPhase::Data;
            }
            _ => {
                let pointer = self.pointer.expect("checked pointer");
                self.registers.insert(pointer, byte);
                if pointer == self.profile.mode_register {
                    self.enabled = byte != 0;
                }
                self.pointer = Some(pointer.wrapping_add(1));
            }
        }
        Ok(())
    }

    /// The next read byte, without effect.
    pub fn peek_read(&self) -> Result<u8, String> {
        let pointer = match (self.phase, self.pointer) {
            (TransferPhase::Read, Some(pointer)) => pointer,
            _ => return Err("Touch chip read without a register pointer".into()),
        };
        if let Some(index) = self.report_index(pointer) {
            Ok(self.current.map_or(0, |frame| frame[index]))
        } else if pointer == self.profile.heartbeat_register {
            Ok(self.heartbeat.wrapping_add(1))
        } else {
            Err(format!("Touch chip register {pointer:#06x} is not readable"))
        }
    }

    /// Check the next read byte without effect.
    pub fn check_read(&self) -> Result<(), String> {
        self.peek_read().map(|_| ())
    }

    fn report_index(&self, pointer: u16) -> Option<usize> {
        pointer
            .checked_sub(self.profile.report_register)
            .map(usize::from)
            .filter(|index| *index < usize::from(REPORT_BYTES))
    }

    pub fn read(&mut self) -> Result<u8, String> {
        let byte = self.peek_read()?;
        let pointer = self.pointer.expect("checked pointer");
        self.pointer = Some(pointer.wrapping_add(1));
        let Some(index) = self.report_index(pointer) else {
            self.heartbeat = byte;
            return Ok(byte);
        };
        if index == usize::from(REPORT_BYTES) - 1 && self.current.take().is_some() {
            self.reads += 1;
        }
        Ok(byte)
    }

    pub fn stop(&mut self) {
        self.phase = TransferPhase::Idle;
    }
}

#[cfg(test)]
mod tests;
