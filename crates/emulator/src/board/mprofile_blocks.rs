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
    // Every instruction so far is 16 bits wide and outside the group that
    // can write a high register (ADD/MOV with high registers, BX, BLX).
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
            keeps_lr &= size == 2 && word & 0xfc00 != 0x4400;
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
    fn scan_counts_entries_and_stops_at_observed_and_unmasking_instructions() {
        // adds; movw (32-bit); adds
        assert_eq!(parts(&[0x3001, 0xf240, 0x0001, 0x3001]), (3, 3, 6, false));
        // adds; bx lr: the BX starts its own block unless LR allows it.
        assert_eq!(parts(&[0x3001, 0x4770]), (2, 1, 2, true));
        // mov lr,r0; bx lr and movw; bx lr: LR may have changed in the block.
        assert_eq!(parts(&[0x4686, 0x4770]), (2, 1, 2, false));
        assert_eq!(parts(&[0xf240, 0x0001, 0x4770]), (2, 1, 4, false));
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
