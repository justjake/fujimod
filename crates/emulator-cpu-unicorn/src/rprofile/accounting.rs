//! Budgeted ARM block accounting over the pinned Unicorn accelerator ABI.
//! This is instruction progress, independent of devices, guest clocks and
//! diagnostic trace storage. A restart returns the unentered block tail;
//! an observed final instruction remains debited and is counted by its hook.
use fujimod_emulator_cpu::BlockAccounting;
use std::ptr::NonNull;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub address: u64,
    pub instructions: u64,
}
impl Span {
    pub fn tail(self, pc: u64, entered: bool) -> u64 {
        let Some(end) = self
            .instructions
            .checked_mul(4)
            .and_then(|n| self.address.checked_add(n))
        else {
            return 0;
        };
        if pc < self.address || pc >= end || !pc.is_multiple_of(4) {
            return 0;
        }
        (end - pc) / 4 - u64::from(entered)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub instructions: u64,
    pub budget: i64,
}

/// A temporary view used only while stopped or inside a serialized callback.
pub struct Accounting {
    record: NonNull<BlockAccounting>,
}
impl Accounting {
    /// # Safety
    /// The record and its initialized power-of-two instruction ring must remain
    /// allocated for this view's lifetime. No accelerator, callback or host
    /// operation may access them concurrently with these operations.
    pub unsafe fn from_raw(record: NonNull<BlockAccounting>) -> Self {
        Self { record }
    }

    #[inline]
    pub fn counters(&self) -> (i64, u64) {
        let r = unsafe { self.record.as_ref() };
        (r.budget, r.head)
    }
    #[inline]
    pub fn span(&self, index: u64) -> Span {
        let r = unsafe { self.record.as_ref() };
        let value = unsafe { *r.ring.add((index & r.mask) as usize) };
        Span {
            address: value >> 32,
            instructions: value & 0xffff_ffff,
        }
    }
    #[inline]
    pub fn newest_since(&self, traced: u64) -> Option<Span> {
        let head = self.counters().1;
        (head != 0 && head != traced).then(|| self.span(head - 1))
    }
    #[inline]
    pub fn reset(&mut self) {
        unsafe {
            self.record.as_mut().budget = 0;
        }
    }

    #[inline]
    pub fn settle(&mut self, synced: i64, tail: u64, give_back: bool) -> Result<Progress, String> {
        let tail = i64::try_from(tail).map_err(|_| "block tail exceeds accounting range")?;
        let r = unsafe { self.record.as_mut() };
        let instructions = synced
            .checked_sub(r.budget)
            .and_then(|n| n.checked_sub(tail))
            .filter(|n| *n >= 0)
            .ok_or_else(|| "budgeted instruction accounting went negative".to_string())?;
        if give_back {
            r.budget = r.budget.checked_add(tail).ok_or("block budget overflow")?;
        }
        Ok(Progress {
            instructions: instructions as u64,
            budget: r.budget,
        })
    }

    /// Admit this whole block and allow at most `room` entries from its first
    /// instruction. Returns the baseline for the next settlement.
    #[inline]
    pub fn admit(&mut self, span: Span, room: u64) -> Result<i64, String> {
        self.admit_inner(span, room, false, true)
    }

    /// Admit a whole block even when it crosses `room`, without recording a
    /// span in the exact-accounting ring. The next block boundary observes the
    /// negative remaining budget and stops the run.
    #[inline]
    pub fn admit_coarse(&mut self, span: Span, room: u64) -> Result<i64, String> {
        self.admit_inner(span, room, true, false)
    }

    fn admit_inner(
        &mut self,
        span: Span,
        room: u64,
        allow_overshoot: bool,
        record: bool,
    ) -> Result<i64, String> {
        if span.instructions == 0
            || span.instructions > u32::MAX.into()
            || !allow_overshoot && span.instructions > room
            || !span.address.is_multiple_of(4)
            || span
                .address
                .checked_add(span.instructions * 4)
                .is_none_or(|end| end > 1 << 32)
        {
            return Err("invalid ARM block span".into());
        }
        let room = room.min(i64::MAX as u64 / 2) as i64;
        let r = unsafe { self.record.as_mut() };
        if record {
            let head = r
                .head
                .checked_add(1)
                .ok_or("block accounting ring head exhausted")?;
            unsafe {
                *r.ring.add((r.head & r.mask) as usize) = span.address << 32 | span.instructions;
            }
            r.head = head;
        }
        r.budget = room - span.instructions as i64;
        Ok(room)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(ring: &mut [u64; 4]) -> BlockAccounting {
        BlockAccounting {
            budget: 0,
            head: 0,
            ring: ring.as_mut_ptr(),
            mask: 3,
            enabled: 1,
            pause_requested: std::ptr::null(),
        }
    }
    #[test]
    fn restart_counts_only_the_entered_prefix_and_returns_the_unentered_tail() {
        let mut ring = [0; 4];
        let mut r = record(&mut ring);
        let mut a = unsafe { Accounting::from_raw(NonNull::from(&mut r)) };
        let span = Span {
            address: 0x1000,
            instructions: 10,
        };
        let synced = a.admit(span, 100).unwrap();
        assert_eq!(a.newest_since(0), Some(span));
        let tail = span.tail(0x1010, false);
        assert_eq!(tail, 6);
        let progress = a.settle(synced, tail, true).unwrap();
        assert_eq!(
            progress,
            Progress {
                instructions: 4,
                budget: 96
            }
        );
        assert_eq!(a.settle(progress.budget, 0, true).unwrap().instructions, 0);
        a.reset();
        let synced = a
            .admit(
                Span {
                    address: 0x1010,
                    instructions: 1,
                },
                1,
            )
            .unwrap();
        assert_eq!(a.settle(synced, 0, true).unwrap().instructions, 1);
    }
    #[test]
    fn coarse_admission_keeps_the_full_block_without_a_ring_entry() {
        let mut ring = [0; 4];
        let mut r = record(&mut ring);
        let mut a = unsafe { Accounting::from_raw(NonNull::from(&mut r)) };
        let span = Span {
            address: 0x1000,
            instructions: 10,
        };
        let synced = a.admit_coarse(span, 1).unwrap();
        assert_eq!(synced, 1);
        assert_eq!(a.counters(), (-9, 0));
        assert_eq!(a.settle(synced, 0, true).unwrap().instructions, 10);
        assert_eq!(a.counters(), (-9, 0));
        assert_eq!(ring, [0; 4]);
    }
    #[test]
    fn an_observed_last_instruction_keeps_its_debit_and_ring_wrap_keeps_the_newest_span() {
        let mut ring = [0; 4];
        let mut r = record(&mut ring);
        let mut a = unsafe { Accounting::from_raw(NonNull::from(&mut r)) };
        let synced = a
            .admit(
                Span {
                    address: 0x1000,
                    instructions: 3,
                },
                100,
            )
            .unwrap();
        assert_eq!(
            a.settle(synced, 1, false).unwrap(),
            Progress {
                instructions: 2,
                budget: 97
            }
        );
        // The instruction hook counts the final entry. Further accelerator
        // debits therefore begin at 97, without giving that entry back.
        assert_eq!(a.settle(97, 0, true).unwrap().instructions, 0);
        for index in 0..8 {
            a.admit(
                Span {
                    address: 0x2000 + index * 4,
                    instructions: 1,
                },
                10,
            )
            .unwrap();
        }
        assert_eq!(a.counters().1, 9);
        assert_eq!(a.newest_since(9), None);
        assert_eq!(
            a.newest_since(8),
            Some(Span {
                address: 0x201c,
                instructions: 1
            })
        );
    }
    #[test]
    fn invalid_spans_and_negative_progress_refuse_before_changing_the_record() {
        let mut ring = [0; 4];
        let mut r = record(&mut ring);
        let mut a = unsafe { Accounting::from_raw(NonNull::from(&mut r)) };
        for span in [
            Span {
                address: 0x1002,
                instructions: 1,
            },
            Span {
                address: 0xffff_fffc,
                instructions: 2,
            },
            Span {
                address: 0,
                instructions: 0,
            },
        ] {
            assert!(a.admit(span, 10).is_err());
            assert_eq!(a.counters(), (0, 0));
        }
        assert!(a.settle(0, 1, true).is_err());
        assert!(
            a.admit(
                Span {
                    address: 0x1000,
                    instructions: 1
                },
                0
            )
            .is_err()
        );
        assert_eq!(a.counters(), (0, 0));
        assert_eq!(
            Span {
                address: 0x1000,
                instructions: 3
            }
            .tail(0x1008, true),
            0
        );
    }
}
