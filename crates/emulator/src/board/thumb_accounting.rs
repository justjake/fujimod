//! An ordered M-profile run lends its remaining instruction budget to the
//! translated block guard. Every callback first takes back the unused entries.
use super::code_shadow;
use fujimod_emulator_cpu::{Cpu, THUMB_GRANT_ENDS_RUN, ThumbBlockAccounting, ThumbBlockRecoveryAccounting};
use std::ptr::NonNull;

#[derive(Default)]
pub(super) struct Window {
    record: Option<NonNull<ThumbBlockAccounting>>,
    recovery: Option<NonNull<ThumbBlockRecoveryAccounting>>,
    exception_status: Option<NonNull<u32>>,
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
        self.exception_status = self.recovery.and_then(|_| cpu.thumb_exception_status());
        self.clear_pages();
        let mut pointer = self.record?;
        // The board owns this thread-affine CPU. Both this call and the
        // operations below execute while stopped or in its serialized callback.
        let record = unsafe { pointer.as_mut() };
        record.accounting.budget = 0;
        record.accounting.enabled = if self.recovery.is_some() { 4 } else { 3 };
        Some(Slice(pointer))
    }

    /// Clear all armed Thumb pages (low-1 MiB bitmap and single-page fallback)
    /// at the start of a slice or on code-coherence flushes.
    pub fn clear_pages(&mut self) {
        if let Some(mut status) = self.exception_status {
            unsafe { *status.as_mut() = 0 };
        }
        if let Some(mut recovery) = self.recovery {
            let record = unsafe { recovery.as_mut() };
            record.thumb_page_bitmap = [0; 16];
            record.thumb_accounting.thumb_page = u64::MAX;
        } else if let Some(mut pointer) = self.record {
            let record = unsafe { pointer.as_mut() };
            record.thumb_page = u64::MAX;
        }
    }

    #[inline]
    pub fn arm(
        &mut self,
        pc: u64,
        remaining: u64,
        active_exception: u16,
        watches_translated_bytes: bool,
        ends_run: bool,
    ) {
        if let Some(mut recovery) = self.recovery {
            self.granted = remaining.min(i64::MAX as u64) as i64;
            let record = unsafe { recovery.as_mut() };
            record.thumb_handler_mode = u64::from(active_exception != 0);
            if let Some(mut status) = self.exception_status {
                let flags = if ends_run { THUMB_GRANT_ENDS_RUN } else { 0 };
                unsafe { *status.as_mut() = (u32::from(active_exception) + 1) | flags };
            }
            record.thumb_accounting.thumb_page = pc & !0x3ff;
            let page = pc >> 10;
            if page < 1024 {
                // The shadow proved this entire host page. Reuse that proof
                // across its Thumb subpages only when stores to previously
                // translated bytes are reported without address watches.
                const SHADOW_SUBPAGES: u64 = code_shadow::PAGE >> 10;
                let bits = if watches_translated_bytes {
                    ((1 << SHADOW_SUBPAGES) - 1) << ((page & 63) & !(SHADOW_SUBPAGES - 1))
                } else {
                    1 << (page & 63)
                };
                record.thumb_page_bitmap[(page >> 6) as usize] |= bits;
            }
            record.thumb_accounting.accounting.budget = self.granted;
        } else if let Some(mut pointer) = self.record.filter(|_| active_exception == 0) {
            self.granted = remaining.min(i64::MAX as u64) as i64;
            let record = unsafe { pointer.as_mut() };
            record.thumb_page = pc & !0x3ff;
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
        if let Some(mut status) = self.exception_status {
            unsafe { *status.as_mut() = 0 };
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_cpu::BlockAccounting;

    fn zeroed_recovery() -> ThumbBlockRecoveryAccounting {
        ThumbBlockRecoveryAccounting {
            thumb_accounting: ThumbBlockAccounting {
                accounting: BlockAccounting {
                    budget: 0,
                    head: 0,
                    ring: std::ptr::null_mut(),
                    mask: 0,
                    enabled: 0,
                    pause_requested: std::ptr::null(),
                },
                thumb_page: 0,
            },
            last_block_start: 0,
            last_block_remaining_budget: 0,
            last_block_bytes: 0,
            last_block_icount: 0,
            thumb_page_bitmap: [0; 16],
            thumb_handler_mode: 0,
        }
    }

    #[test]
    fn arm_accumulates_low_1mib_pages_and_clear_pages_resets_them() {
        let mut raw = zeroed_recovery();
        let recovery = NonNull::from(&mut raw);
        let mut window = Window {
            record: Some(recovery.cast()),
            recovery: Some(recovery),
            exception_status: None,
            granted: 0,
            entries: 0,
        };

        // Arm 1 KiB pages including boundary bits 0, 63, 64, and 1023.
        for pc in [0x0000_0004u64, 0x0000_fc10, 0x0001_0004, 0x000f_fffe] {
            window.arm(pc, 100, 0, false, false);
        }
        assert_eq!(raw.thumb_page_bitmap[0], (1u64 << 0) | (1u64 << 63));
        assert_eq!(raw.thumb_page_bitmap[1], 1u64 << 0);
        for word in &raw.thumb_page_bitmap[2..15] {
            assert_eq!(*word, 0);
        }
        assert_eq!(raw.thumb_page_bitmap[15], 1u64 << 63);
        assert_eq!(raw.thumb_accounting.thumb_page, 0x000f_fc00);
        assert_eq!(raw.thumb_accounting.accounting.budget, 100);

        // Addresses at or above 1 MiB update thumb_page (1 KiB aligned) without touching the bitmap.
        window.arm(0x0010_04bc, 50, 11, false, false);
        assert_eq!(raw.thumb_handler_mode, 1);
        assert_eq!(raw.thumb_accounting.thumb_page, 0x0010_0400);
        assert_eq!(raw.thumb_page_bitmap[0], (1u64 << 0) | (1u64 << 63));
        assert_eq!(raw.thumb_page_bitmap[1], 1u64 << 0);
        assert_eq!(raw.thumb_page_bitmap[15], 1u64 << 63);

        window.clear_pages();
        assert_eq!(raw.thumb_page_bitmap, [0; 16]);
        assert_eq!(raw.thumb_accounting.thumb_page, u64::MAX);
    }

    #[test]
    fn code_store_notifications_admit_the_verified_shadow_page_only() {
        let mut raw = zeroed_recovery();
        let recovery = NonNull::from(&mut raw);
        let mut window = Window {
            record: Some(recovery.cast()),
            recovery: Some(recovery),
            exception_status: None,
            granted: 0,
            entries: 0,
        };

        window.arm(0x000f_f811, 100, 0, true, false);
        window.arm(0x0000_0100, 99, 0, true, false);
        assert_eq!(raw.thumb_page_bitmap[0], 0xf);
        assert_eq!(raw.thumb_page_bitmap[15], 0xf << 60);
        assert!(raw.thumb_page_bitmap[1..15].iter().all(|&word| word == 0));

        window.clear_pages();
        assert_eq!(raw.thumb_page_bitmap, [0; 16]);
    }

}
