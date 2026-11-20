//! Block execution of an M-profile processor.
//!
//! The per-instruction observer costs a callback for every instruction. In
//! block mode the engine has no instruction hook and no instruction counter.
//! The board observes each translated block when it is entered and does there,
//! for the block's first instruction, exactly what the instruction observer
//! does: device catch-up, interrupt pins, exception entry, and the M-profile
//! controller's emulated encodings. It then admits as many of the block's
//! instructions as may run without another observation.
//!
//! Between two instructions of an admitted run nothing the observer depends
//! on can change:
//! - a device event is due no earlier than the admitted entry count (the
//!   admission is capped by the timeline's budget to the next deadline);
//! - a store or a device access is the last instruction of its block (the
//!   engine restarts a hooked access as a block of its own), so its effects
//!   are seen at the next block entry;
//! - an instruction the controller may emulate starts its own block, and an
//!   instruction that can unmask an exception ends its block (`scan`).
//!
//! Entries are accounted when the next observation happens: at the next block
//! entry, at a restart (only the entered prefix), in the store hook, in a
//! device access, and when the run ends. No clock or counter is read between.
//!
//! All of this is host-only. The entry counts, the instants of device events
//! and the instructions at which exceptions are entered are those of the
//! per-instruction observer.

/// One M-profile processor's block execution state.
#[derive(Default)]
pub(super) struct Blocks {
    /// The engine is set up for block execution (hooks and translations).
    pub active: bool,
    /// Instruction entries this run may still begin.
    pub room: u64,
    /// The admitted block now executing: address, bytes, entries, and the
    /// address of its last instruction.
    pub pending: Option<(u64, u32, u64, u64)>,
    /// The board store hook sees only watched pages (code pages, a live
    /// exclusive granule) and device pages; other stores keep the engine's
    /// inline path and no longer end their block.
    pub fast_stores: bool,
    /// Store watches were declared on this engine. From then on the engine
    /// detects stores into translated code only on watched pages, so every
    /// page code is entered from must be watched, in every mode.
    pub watching: bool,
    pub watched: std::collections::HashSet<u64>,
    pub last_watched: Option<u64>,
    /// The observer changed the PC: the run stopped and must resume at once.
    pub resume: bool,
    /// Host-only counters: observed block entries, admitted entries, and
    /// blocks retranslated to a shorter admission.
    pub observed: u64,
    pub admitted: u64,
    pub shortened: u64,
    /// The last device read of this run, while it may repeat as a poll loop
    /// (`Poll`); whether such loops are fast-forwarded; and the reads left
    /// out since the access callback last collected them.
    pub poll: Option<Poll>,
    pub poll_enabled: bool,
    pub skipped_reads: u64,
    /// Host-only counters: fast-forwards and the reads they left out.
    pub poll_skips: u64,
    pub poll_reads: u64,
    /// Why: bookkeeping not settled, an instruction the observer must see,
    /// the run's budget, the peer's turn, a deadline.
    pub shortened_by: [u64; 5],
}

/// A device read that may be the head of a poll loop: a load from a
/// register whose value only device events change (`Device::poll_stable`),
/// followed by instructions that only compute in registers, and a
/// conditional branch back to the load (`poll_period`).
///
/// When the processor comes back to the same read after exactly one period,
/// with no exception entry, store or other device access in between, reads
/// the same value, has the same registers as at the read before, and no
/// exception is ready, then the round that follows is the round that just
/// ended: the loop has no other input. It repeats unchanged until a device
/// event, another processor, or the end of the run. The board accounts whole
/// rounds up to that bound at once and leaves their reads out.
#[derive(Clone, Copy)]
pub(super) struct Poll {
    pub pc: u64,
    pub address: u64,
    pub size: u8,
    /// Instruction entries of one round, when the code at `pc` is a poll loop.
    pub period: Option<u64>,
    /// The value the last read returned and the processor's entry count then.
    pub value: u64,
    pub instructions: u64,
    /// R0-R14 and the status register at the last read that ended a round.
    pub registers: Option<[u64; 16]>,
}

/// The number of instructions of the poll loop that starts with the load at
/// the start of `code`, or `None` when the code is anything else. Accepted:
/// a 16-bit LDR, LDRB or LDRH with an immediate offset; then 16-bit shift,
/// add, subtract, move, compare and data-processing instructions on low
/// registers, sign and zero extension, and 32-bit data processing with an
/// immediate into R0-R12 or as a comparison; then a 16-bit conditional
/// branch to the load. None of them stores, loads from memory, changes
/// control state or leaves the loop except by falling through the branch.
pub(super) fn poll_period(code: &[u8]) -> Option<u64> {
    let half = |offset: usize| {
        code.get(offset..offset + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    if !matches!(half(0)? & 0xf800, 0x6800 | 0x7800 | 0x8800) {
        return None;
    }
    let (mut offset, mut count) = (2usize, 1u64);
    while count < 8 {
        let word = half(offset)?;
        if word >> 11 >= 0x1d {
            let second = half(offset + 2)?;
            let rd = (second >> 8) & 15;
            // TST, TEQ, CMN, CMP: flags only.
            let compares = rd == 15
                && word & 0x0210 == 0x0010
                && matches!((word >> 5) & 15, 0 | 4 | 8 | 13);
            if word & 0xf800 != 0xf000 || second & 0x8000 != 0 || !(rd <= 12 || compares) {
                return None;
            }
            offset += 4;
        } else {
            match word {
                0x0000..=0x43ff | 0xb200..=0xb2ff => offset += 2,
                0xd000..=0xddff => {
                    let target = offset as i64 + 4 + i64::from(word as u8 as i8) * 2;
                    return (target == 0).then_some(count + 1);
                }
                _ => return None,
            }
        }
        count += 1;
    }
    None
}

/// Whether the controller's observer may emulate, enter or refuse an
/// instruction with this first halfword (`Profile::before`). Such an
/// instruction starts its own block.
pub(super) fn observed(word: u16) -> bool {
    matches!(word, 0xbf30 | 0xbf20 | 0xf3ef | 0xf3bf)
        || word & 0xff00 == 0xdf00
        || word & 0xff87 == 0x4700
        || matches!(word & 0xfff0, 0xe850 | 0xe840 | 0xe8d0 | 0xe8c0)
}

/// Whether an instruction with this first halfword can change which
/// exceptions are masked: CPS and MSR (PRIMASK, FAULTMASK, BASEPRI,
/// CONTROL). A pending exception may become ready after it, so it ends its
/// block.
fn unmasks(word: u16) -> bool {
    word & 0xffe8 == 0xb660 || word & 0xfff0 == 0xf380
}

/// Whether the controller's observer may do more than let this instruction
/// execute (`Profile::before`): emulate it, enter or return from an
/// exception, wait, or refuse it. `second` is the second halfword of a
/// 32-bit instruction. BX is not decided here.
fn emulated(word: u16, second: u16) -> bool {
    use fujimod_emulator_cpu::arm::thumb::exclusive;
    matches!(word, 0xbf30 | 0xbf20)
        || word & 0xff00 == 0xdf00
        || word == 0xf3ef && matches!(second & 255, 1 | 3 | 5 | 7)
        || (matches!(word & 0xfff0, 0xe850 | 0xe840 | 0xe8d0 | 0xe8c0) || word == 0xf3bf)
            && !matches!(exclusive::decode(word, second), Ok(None))
}

/// Whether an instruction can write LR. Exact for 16-bit instructions. For
/// a 32-bit instruction this is a superset: every encoding names a core
/// register it writes in bits 3..0 of the first halfword (base register with
/// writeback) or in bits 15..12 or 11..8 of the second (Rt, RdLo, Rd, Rt2,
/// RdHi), or in the register list of a load multiple (bit 14).
fn may_write_lr(word: u16, second: u16, size: usize) -> bool {
    if size == 2 {
        // ADD and MOV with high registers name Rd in bits 7 and 2..0; BLX
        // writes LR. No other 16-bit instruction can: POP loads R0-R7 and PC.
        return match word & 0xff00 {
            0x4400 | 0x4600 => (word & 7) | ((word >> 4) & 8) == 14,
            0x4700 => word & 0x80 != 0,
            _ => false,
        };
    }
    word & 15 == 14
        || (second >> 12) & 15 == 14
        || (second >> 8) & 15 == 14
        || word & 0xfe40 == 0xe800 && second & 0x4000 != 0
}

/// A translated Thumb block as the observer admits it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Scan {
    /// Instruction entries in the block.
    pub entries: u64,
    /// How many of them, from the first, may run without another
    /// observation.
    pub unit: u64,
    /// Offset of the last instruction.
    pub last: u64,
    /// The admission stops before a `bx lr` that every earlier instruction
    /// of the block leaves LR alone for. If LR at the block's entry is not an
    /// exception return value, that BX is an ordinary branch and the whole
    /// block may run.
    pub returns_through_lr: bool,
}

/// Scan a translated Thumb block.
pub(super) fn scan(code: &[u8]) -> Result<Scan, String> {
    let (mut offset, mut entries, mut unit, mut last) = (0, 0u64, None, 0);
    // No instruction so far can write LR (`may_write_lr`).
    let mut keeps_lr = true;
    let mut returns_through_lr = false;
    while offset < code.len() {
        last = offset as u64;
        let Some(halfword) = code.get(offset..offset + 2) else {
            return Err("M-profile block ends inside an instruction".into());
        };
        let word = u16::from_le_bytes([halfword[0], halfword[1]]);
        let size = if word >> 11 >= 0x1d { 4 } else { 2 };
        if offset + size > code.len() {
            return Err("M-profile block ends inside an instruction".into());
        }
        let second = if size == 4 {
            u16::from_le_bytes([code[offset + 2], code[offset + 3]])
        } else {
            0
        };
        if unit.is_none() {
            let exchange = word & 0xff87 == 0x4700;
            if entries != 0 && (exchange || emulated(word, second)) {
                unit = Some(entries);
                returns_through_lr = word == 0x4770 && keeps_lr;
            } else if unmasks(word) {
                unit = Some(entries + 1);
            }
            keeps_lr &= !may_write_lr(word, second, size);
        }
        entries += 1;
        offset += size;
    }
    if entries == 0 {
        return Err("empty M-profile block".into());
    }
    Ok(Scan {
        entries,
        unit: unit.unwrap_or(entries),
        last,
        returns_through_lr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn code(words: &[u16]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }
    fn parts(words: &[u16]) -> (u64, u64, u64, bool) {
        let scan = scan(&code(words)).unwrap();
        (scan.entries, scan.unit, scan.last, scan.returns_through_lr)
    }
    #[test]
    fn poll_period_accepts_only_register_computation_between_a_load_and_its_branch() {
        // ldrh r5,[r2]; and r5,r5,#4; uxth r5,r5; cmp r5,#0; beq .-10
        assert_eq!(poll_period(&code(&[0x8815, 0xf005, 0x0504, 0xb2ad, 0x2d00, 0xd0f9])), Some(5));
        // ldr r2,[r0]; cmp r2,r3; beq .-4
        assert_eq!(poll_period(&code(&[0x6802, 0x429a, 0xd0fc])), Some(3));
        // ldrb r1,[r0]; tst r1,#0x80; bne .-6 (TST writes no register).
        assert_eq!(poll_period(&code(&[0x7801, 0xf011, 0x0f80, 0xd1fb])), Some(3));
        // The branch goes elsewhere, or forward.
        assert_eq!(poll_period(&code(&[0x6802, 0x429a, 0xd0fb])), None);
        assert_eq!(poll_period(&code(&[0x6802, 0x429a, 0xd000])), None);
        // Not a load first: str r2,[r0]; a register-offset or literal load.
        assert_eq!(poll_period(&code(&[0x6002, 0x429a, 0xd0fc])), None);
        assert_eq!(poll_period(&code(&[0x5842, 0x429a, 0xd0fc])), None);
        assert_eq!(poll_period(&code(&[0x4a02, 0x429a, 0xd0fc])), None);
        // A store, a second load, a push, a call, a high-register move, an IT
        // instruction, SVC, or CPS in the body.
        for body in [0x6001u16, 0x6801, 0xb401, 0x4780, 0x4686, 0xbf08, 0xdf00, 0xb672, 0x4770] {
            assert_eq!(poll_period(&code(&[0x6802, body, 0xd0fc])), None, "{body:#x}");
        }
        // 32-bit instructions that are not data processing with an immediate:
        // str.w r1,[r0]; ldr.w r1,[r0]; and.w r1,r1,r2; bl; msr basepri,r0.
        for (first, second) in [
            (0xf8c0u16, 0x1000u16),
            (0xf8d0, 0x1000),
            (0xea01, 0x0102),
            (0xf000, 0xf800),
            (0xf380, 0x8811),
        ] {
            assert_eq!(poll_period(&code(&[0x6802, first, second, 0xd0fb])), None, "{first:#x}");
        }
        // movw sp,#0 and add.w pc,r1,#4 write SP and PC; ands.w with Rd 15
        // and no S is not a comparison.
        assert_eq!(poll_period(&code(&[0x6802, 0xf240, 0x0d00, 0xd0fb])), None);
        assert_eq!(poll_period(&code(&[0x6802, 0xf101, 0x0f04, 0xd0fb])), None);
        assert_eq!(poll_period(&code(&[0x6802, 0xf001, 0x0f04, 0xd0fb])), None);
        // Too long, or cut off.
        assert_eq!(poll_period(&code(&[0x6802, 0x3001, 0x3001, 0x3001, 0x3001, 0x3001, 0x3001, 0xd0f7])), Some(8));
        assert_eq!(poll_period(&code(&[0x6802, 0x3001, 0x3001, 0x3001, 0x3001, 0x3001, 0x3001, 0x3001, 0xd0f6])), None);
        assert_eq!(poll_period(&code(&[0x6802, 0x429a])), None);
        assert_eq!(poll_period(&[]), None);
    }
    #[test]
    fn scan_counts_entries_and_stops_at_observed_and_unmasking_instructions() {
        // adds; movw (32-bit); adds
        assert_eq!(parts(&[0x3001, 0xf240, 0x0001, 0x3001]), (3, 3, 6, false));
        // adds; bx lr: the BX starts its own block unless LR allows it.
        assert_eq!(parts(&[0x3001, 0x4770]), (2, 1, 2, true));
        // mov lr,r0; bx lr and movw; bx lr: LR may have changed in the block.
        assert_eq!(parts(&[0x4686, 0x4770]), (2, 1, 2, false));
        // movw lr,#1; bx lr: LR changed in the block.
        assert_eq!(parts(&[0xf240, 0x0e01, 0x4770]), (2, 1, 4, false));
        // movw r0,#1; bx lr and mov sp,r7; pop {r4,r5,r7}; bx lr: LR is kept.
        assert_eq!(parts(&[0xf240, 0x0001, 0x4770]), (2, 1, 4, true));
        assert_eq!(parts(&[0x46bd, 0xbcb0, 0x4770]), (3, 2, 4, true));
        // add lr,r0; ldr.w lr,[sp],#4; pop.w {r4,lr}; ldrd r0,lr,[sp]: LR may change.
        assert_eq!(parts(&[0x4486, 0x4770]), (2, 1, 2, false));
        assert_eq!(parts(&[0xf85d, 0xeb04, 0x4770]), (2, 1, 4, false));
        assert_eq!(parts(&[0xe8bd, 0x4010, 0x4770]), (2, 1, 4, false));
        assert_eq!(parts(&[0xe9dd, 0x0e00, 0x4770]), (2, 1, 4, false));
        // ldr.w r4,[sp,#4]; pop.w {r4,r5}; bx lr: LR is kept.
        assert_eq!(parts(&[0xf8dd, 0x4004, 0xe8bd, 0x0030, 0x4770]), (3, 2, 8, true));
        // umull r0,lr,r1,r2 (RdHi) and ldr r0,[lr],#4 (writeback).
        assert_eq!(parts(&[0xfba1, 0x0e02, 0x4770]), (2, 1, 4, false));
        assert_eq!(parts(&[0xf85e, 0x0b04, 0x4770]), (2, 1, 4, false));
        // adds; bx r3: only LR is checked at the block's entry.
        assert_eq!(parts(&[0x3001, 0x4718]), (2, 1, 2, false));
        // bx lr first: it belongs to this block's observation.
        assert_eq!(parts(&[0x4770, 0x3001]), (2, 2, 2, false));
        // adds; cpsie i; adds: the block ends after CPS.
        assert_eq!(parts(&[0x3001, 0xb662, 0x3001]), (3, 2, 4, false));
        // msr basepri, r0 (32-bit) first; adds.
        assert_eq!(parts(&[0xf380, 0x8811, 0x3001]), (2, 1, 4, false));
        // wfi after two instructions; svc after one.
        assert_eq!(parts(&[0x3001, 0x3001, 0xbf30]), (3, 2, 4, false));
        assert_eq!(parts(&[0x3001, 0xdf00]), (2, 1, 2, false));
        // ldrex in second place; tbb and dsb are not exclusive instructions.
        assert_eq!(parts(&[0x3001, 0xe850, 0x1f00]), (2, 1, 2, false));
        assert_eq!(parts(&[0x3001, 0xe8df, 0xf000]), (2, 2, 2, false));
        assert_eq!(parts(&[0x3001, 0xf3bf, 0x8f4f, 0x3001]), (3, 3, 6, false));
        assert_eq!(parts(&[0x3001, 0xf3bf, 0x8f2f, 0x3001]), (3, 1, 6, false));
        // mrs r1,primask executes; mrs r2,ipsr is emulated.
        assert_eq!(parts(&[0x3001, 0xf3ef, 0x8110, 0x3001]), (3, 3, 6, false));
        assert_eq!(parts(&[0x3001, 0xf3ef, 0x8205, 0x3001]), (3, 1, 6, false));
        // A second halfword that looks observed is not an instruction.
        assert_eq!(parts(&[0xf240, 0x4770, 0x3001]), (2, 2, 4, false));
        assert!(scan(&code(&[0xf240])).is_err());
        assert!(scan(&[0x01]).is_err());
        assert!(scan(&[]).is_err());
    }
}
