//! An ordered M-profile run lends its remaining instruction budget to the
//! translated block guard. Every callback first takes back the unused entries.
use fujimod_emulator_cpu::{Cpu, ThumbBlockAccounting};
use std::ptr::NonNull;

#[derive(Default)]
pub(super) struct Window {
    record: Option<NonNull<ThumbBlockAccounting>>,
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
        self.record = if enabled {
            cpu.thumb_accounting()
        } else {
            None
        };
        let mut pointer = self.record?;
        // The board owns this thread-affine CPU. Both this call and the
        // operations below execute while stopped or in its serialized callback.
        let record = unsafe { pointer.as_mut() };
        record.accounting.budget = 0;
        record.accounting.enabled = 3;
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

    #[inline]
    pub fn take(&mut self) -> Result<u64, String> {
        if self.granted == 0 {
            return Ok(0);
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
