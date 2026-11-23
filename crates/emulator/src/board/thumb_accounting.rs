//! An ordered M-profile run lends its remaining instruction budget to the
//! translated block guard. Every callback first takes back the unused entries.
use fujimod_emulator_cpu::{Cpu, ThumbBlockAccounting, ThumbBlockRecoveryAccounting};
use std::ptr::NonNull;

#[derive(Default)]
pub(super) struct Window {
    record: Option<NonNull<ThumbBlockAccounting>>,
    recovery: Option<NonNull<ThumbBlockRecoveryAccounting>>,
    granted: i64,
    pub entries: u64,
}

/// Disable the engine path on every exit from the ordered slice, including errors.
pub(super) struct Slice(NonNull<ThumbBlockAccounting>);

impl Drop for Slice {
    fn drop(&mut self) {
        // The CPU outlives this guard and is stopped.
        let record = unsafe { self.0.as_mut() };
        record.accounting.enabled = 0;
        record.accounting.budget = 0;
    }
}

impl Window {
    pub fn begin(&mut self, cpu: &dyn Cpu, enabled: bool) -> Option<Slice> {
        self.granted = 0;
        self.recovery = if enabled {
            cpu.thumb_recovery_accounting()
        } else {
            None
        };
        self.record = if let Some(mut pointer) = self.recovery {
            // The extension begins with the existing Thumb accounting record.
            let record = unsafe { pointer.as_mut() };
            record.last_block_bytes = 0;
            Some(pointer.cast())
        } else if enabled {
            cpu.thumb_accounting()
        } else {
            None
        };
        let mut pointer = self.record?;
        // The board owns this thread-affine CPU. Both this call and the
        // operations below execute while stopped or in its serialized callback.
        let record = unsafe { pointer.as_mut() };
        record.accounting.budget = 0;
        record.accounting.enabled = if self.recovery.is_some() { 4 } else { 3 };
        Some(Slice(pointer))
    }

    #[inline]
    pub fn arm(&mut self, pc: u64, remaining: u64) {
        if let Some(mut pointer) = self.record {
            self.granted = remaining.min(i64::MAX as u64) as i64;
            let record = unsafe { pointer.as_mut() };
            record.thumb_page = pc & !0xfff;
            record.accounting.budget = self.granted;
        }
    }

    /// The most recent block charged since this window was armed. The
    /// translated guard writes this before executing the first instruction.
    #[inline]
    pub fn charged_block(&self) -> Option<(u64, u32, u64)> {
        if self.granted == 0 {
            return None;
        }
        let record = unsafe { self.recovery?.as_ref() };
        let budget = record.thumb_accounting.accounting.budget;
        (record.last_block_bytes != 0
            && record.last_block_remaining_budget == budget
            && budget < self.granted)
            .then_some((
                record.last_block_start,
                record.last_block_bytes,
                u64::from(record.last_block_icount),
            ))
    }

    /// The transfer restarted or faulted before the rest of its block ran.
    /// The following settlement spends only the executed part of the grant.
    pub fn refund_tail(&mut self, tail: u64) -> Result<(), String> {
        let mut pointer = self.recovery.ok_or("Thumb block has no recovery record")?;
        let record = unsafe { pointer.as_mut() };
        let remaining = record.thumb_accounting.accounting.budget;
        record.thumb_accounting.accounting.budget = i64::try_from(tail)
            .ok()
            .and_then(|n| remaining.checked_add(n))
            .filter(|n| *n <= self.granted)
            .ok_or("invalid translated Thumb block tail")?;
        Ok(())
    }

    #[inline]
    pub fn take(&mut self) -> Result<u64, String> {
        if self.granted == 0 {
            return Ok(0);
        }
        if let Some(mut pointer) = self.recovery {
            unsafe { pointer.as_mut() }.last_block_bytes = 0;
        }
        let record = unsafe { self.record.as_mut().unwrap().as_mut() };
        let granted = std::mem::take(&mut self.granted);
        let remaining = std::mem::take(&mut record.accounting.budget);
        if !(0..=granted).contains(&remaining) {
            return Err("invalid remaining translated Thumb instruction budget".into());
        }
        let entered = (granted - remaining) as u64;
        self.entries += entered;
        Ok(entered)
    }
}
