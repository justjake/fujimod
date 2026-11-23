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
    pub inline: super::thumb_accounting::Window,
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
    // Host-only counts of actual callback invocations, including retries.
    pub instruction_callbacks: u64,
    pub block_callbacks: u64,
    pub restart_callbacks: u64,
    pub store_callbacks: u64,
    pub mmio_callbacks: u64,
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
    /// Scans of blocks entered before (`ScanCache`).
    pub scans: ScanCache,
}

/// Host-only: the scans of recently entered blocks, by address and length. An entry
/// holds the code it was made from and answers only for the same bytes, so
/// rewritten code is scanned again. A scan is a function of the bytes alone.
pub(super) struct ScanCache {
    entries: Box<[ScanEntry]>,
}
#[derive(Clone, Copy)]
struct ScanEntry {
    pc: u64,
    len: u8,
    code: [u8; ScanCache::BYTES],
    scan: Scan,
}
impl Default for ScanCache {
    fn default() -> Self {
        let empty = ScanEntry {
            pc: u64::MAX,
            len: 0,
            code: [0; Self::BYTES],
            scan: Scan { entries: 0, unit: 0, last: 0, returns_through_lr: false },
        };
        Self { entries: vec![empty; Self::SLOTS].into_boxed_slice() }
    }
}
impl ScanCache {
    const BYTES: usize = 44;
    const SLOTS: usize = 4096;
    #[inline]
    fn slot(pc: u64, len: usize) -> usize {
        // A shortened admission and the full translation at its PC can
        // both be hot. Keep them in separate slots.
        ((pc >> 1) as usize ^ len.wrapping_mul(0x9e37)) & (Self::SLOTS - 1)
    }
    /// The scan of the block at `pc` when it was made from exactly `code`.
    #[inline]
    pub fn get(&self, pc: u64, code: &[u8]) -> Option<Scan> {
        let entry = &self.entries[Self::slot(pc, code.len())];
        (entry.pc == pc && entry.code[..usize::from(entry.len)] == *code).then_some(entry.scan)
    }
    /// The bytes of a block short enough to keep.
    #[inline]
    pub fn key(code: &[u8]) -> Option<(u8, [u8; Self::BYTES])> {
        if code.is_empty() || code.len() > Self::BYTES {
            return None;
        }
        let mut kept = [0; Self::BYTES];
        kept[..code.len()].copy_from_slice(code);
        Some((code.len() as u8, kept))
    }
    #[inline]
    pub fn put(&mut self, pc: u64, (len, code): (u8, [u8; Self::BYTES]), scan: Scan) {
        self.entries[Self::slot(pc, usize::from(len))] = ScanEntry { pc, len, code, scan };
    }
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

fn thumb_size(word: u16) -> usize {
    if word >> 11 >= 0x1d { 4 } else { 2 }
}

/// Whether a Thumb instruction transfers one register to or from memory using
/// an ordinary single-register addressing mode. Literal reads are admitted
/// only for legal general-register destinations. Register-offset transfers
/// require a legal shift and ordinary GPRs. Multiple transfers and stack
/// push/pop encodings are excluded.
pub(super) fn is_single_transfer(word: u32) -> bool {
    if word <= u32::from(u16::MAX) {
        let instruction = word as u16;
        return matches!(instruction & 0xf000, 0x5000 | 0x8000 | 0x9000)
            || instruction & 0xe000 == 0x6000
            // PC-relative LDR (literal), with the destination encoded in
            // R0-R7. The word read is recoverable on the new path.
            || instruction & 0xf800 == 0x4800;
    }

    let first = (word >> 16) as u16;
    let second = word as u16;
    let operation = first & 0xfff0;
    if matches!(
        operation,
        0xf800 | 0xf810 | 0xf820 | 0xf830 | 0xf840 | 0xf850 | 0xf910 | 0xf930
    ) {
        let rn = first & 15;
        let rt = second >> 12;
        let rm = second & 15;
        if second & 0x0fc0 == 0 {
            // Register-offset forms have no writeback. Bits 5:4 encode the
            // legal LSL amount, including the scaled halfword table scan.
            return rt <= 12 && rn < 15 && rm <= 12;
        }
        // Immediate-indexed forms set bit 11, then use P/U/W in bits 10:8
        // and imm8 in bits 7:0. P=0,W=0 is unprivileged or reserved on these
        // encodings, so admit only pre-index and post-index.
        let pre_index = second & 0x0400 != 0;
        let writeback = second & 0x0100 != 0;
        if second & 0x0800 == 0
            || (!pre_index && !writeback)
            || rn > 12
            || rt > 14
            || rt == 13
            || (writeback && rn == rt)
        {
            return false;
        }
        return true;
    }
    if !matches!(
        operation,
        0xf8c0 | 0xf8d0 | 0xf880 | 0xf890 | 0xf8a0 | 0xf8b0 | 0xf990 | 0xf9b0
    ) {
        return false;
    }
    let rn = first & 15;
    let rt = second >> 12;
    // Keep transfers to R0-R12. Rn=PC is legal only for the immediate word
    // LDR literal form; all other bases may be R0-R14, including SP and LR.
    rt <= 12 && (rn < 15 || operation == 0xf8d0)
}

/// Whether a Thumb instruction is safe to execute without an observation.
/// A 16-bit instruction is zero-extended; for 32-bit instructions, the first
/// halfword is in bits 31..16. Accepted operations use general registers,
/// limited SP arithmetic/moves, APSR flags, mask-register reads, or ordinary
/// immediate branches.
pub(super) fn can_run_inline(word: u32) -> bool {
    if word <= u32::from(u16::MAX) {
        let instruction = word as u16;
        return matches!(instruction, 0x0000..=0x43ff | 0xb200..=0xb2ff)
            // High-register ADD, CMP and MOV, provided neither operand or
            // destination is SP or PC. BX/BLX (operation 3) are excluded.
            || (instruction & 0xfc00 == 0x4400
                && (instruction >> 8) & 3 != 3
                && {
                    let rd = ((instruction >> 4) & 8) | (instruction & 7);
                    let rm = (instruction >> 3) & 15;
                    let low_register_cmp = (instruction >> 8) & 3 == 1 && rd < 8 && rm < 8;
                    if (instruction >> 8) & 3 == 1 {
                        rd < 15 && rm < 15 && !low_register_cmp
                    } else {
                        rd < 13 && rm < 13
                    }
                })
            // CBZ/CBNZ are immediate, non-linking branches with no flag or
            // control-state effects.
            || instruction & 0xf500 == 0xb100
            // ADD/SUB SP,#imm and ADD Rd,SP,#imm are register-only and cannot
            // fault.
            || instruction & 0xff00 == 0xb000
            || instruction & 0xf800 == 0xa800
            // High-register MOV can move to/from SP without touching PC.
            || (instruction & 0xfc00 == 0x4400
                && (instruction >> 8) & 3 == 2
                && {
                    let rd = ((instruction >> 4) & 8) | (instruction & 7);
                    let rm = (instruction >> 3) & 15;
                    rd != 15 && rm != 15 && rd <= 13 && rm <= 13
                })
            // Conditional and unconditional immediate branches do not link.
            || matches!(instruction, 0xd000..=0xddff | 0xe000..=0xe7ff);
    }

    let first = (word >> 16) as u16;
    let second = word as u16;
    let rd = (second >> 8) & 15;
    // Native MRS reads mask state from Unicorn's M-profile architectural
    // helper. The controller emulates xPSR-family selectors 1/3/5/7; permit
    // only the exact PRIMASK, BASEPRI, and FAULTMASK selectors here.
    if first == 0xf3ef {
        return rd <= 12 && matches!(second & 0xf0ff, 0x8010 | 0x8011 | 0x8013);
    }
    // MOVW/MOVT (A7.7.80, A7.7.84). Their immediate fields have no
    // reserved values; limit the destination to ordinary R0-R12.
    if matches!(first & 0xfbf0, 0xf240 | 0xf2c0) {
        return second & 0x8000 == 0 && rd <= 12;
    }
    // Data processing with an immediate shift of Rm. Rn=15 is legal
    // only for the MOV/shift and MVN aliases, and Rd=15 only for TST/TEQ.
    if first & 0xff00 == 0xea00 {
        let rn = first & 15;
        let rm = second & 15;
        let set_flags = first & 0x10 != 0;
        if second & 0x8000 != 0 || rm > 12 {
            return false;
        }
        return match first & 0xffe0 {
            0xea00 | 0xea80 => rn <= 12 && (rd <= 12 || set_flags && rd == 15),
            0xea20 => rn <= 12 && rd <= 12,
            0xea40 | 0xea60 => (rn <= 12 || rn == 15) && rd <= 12,
            _ => false,
        };
    }
    // Thumb-2 immediate branches. BL writes LR and ends the block. A
    // conditional branch has a real condition in bits 9:6. BLX and
    // system encodings in the reserved condition space remain observed.
    if first & 0xf800 == 0xf000 && second & 0x8000 != 0 {
        return match second & 0xd000 {
            0x9000 | 0xd000 => true,
            0x8000 => (first >> 6) & 15 < 14,
            _ => false,
        };
    }
    // Byte/halfword extension with an optional rotation. Rn=15 is
    // the extend-only alias. Also admit the fixed-register REV family.
    if matches!(first & 0xfff0, 0xfa00 | 0xfa10 | 0xfa40 | 0xfa50)
        && second & 0xf0c0 == 0xf080
    {
        let rn = first & 15;
        return (rn <= 12 || rn == 15) && rd <= 12 && second & 15 <= 12;
    }
    if first & 0xfff0 == 0xfa90 && second & 0xf0c0 == 0xf080 {
        return first & 15 == second & 15 && rd <= 12 && second & 15 <= 12;
    }
    // SMULL/UMULL write two distinct ordinary registers.
    if matches!(first & 0xfff0, 0xfb80 | 0xfba0) {
        let rd_lo = second >> 12;
        return second & 0xf0 == 0 && first & 15 <= 12 && second & 15 <= 12
            && rd_lo <= 12 && rd <= 12 && rd_lo != rd;
    }
    // Register-specified LSL, LSR, ASR and ROR. The shift count
    // and destination are ordinary registers.
    if first & 0xff80 == 0xfa00 {
        return second & 0xf0f0 == 0xf000 && first & 15 <= 12 && second & 15 <= 12 && rd <= 12;
    }
    // MUL is the Ra=15 alias of MLA. MLS has a distinct operation
    // field and requires an actual accumulator register.
    if first & 0xfff0 == 0xfb00 {
        let ra = second >> 12;
        let valid_operation = match second & 0xf0 {
            0 => ra <= 12 || ra == 15,
            0x10 => ra <= 12,
            _ => false,
        };
        return valid_operation && first & 15 <= 12 && second & 15 <= 12 && rd <= 12;
    }
    // ADD/ADC/SBC/SUB (shifted register), with a register-controlled pure arithmetic
    // result. Exclude SP and PC in all three register fields and reserved bit
    // 15 of the second halfword. Every immediate shift amount/type is defined.
    if matches!(first & 0xffe0, 0xeb00 | 0xeb40 | 0xeb60 | 0xeba0) {
        let rn = first & 15;
        let rm = second & 15;
        return second & 0x8000 == 0 && rn <= 12 && rd <= 12 && rm <= 12;
    }
    // ThumbExpandImm uses i:imm3:imm8. The top five bits identify this
    // instruction family; bit 10 is the immediate's high bit, not an opcode
    // bit. Bit 15 of the second halfword is reserved, while bits 14:12 are
    // imm3.
    if first & 0xf800 != 0xf000 || second & 0x8000 != 0 {
        return false;
    }
    let imm12 = ((u32::from(first >> 10) & 1) << 11)
        | ((u32::from(second >> 12) & 7) << 8)
        | u32::from(second & 0xff);
    // ThumbExpandImm's replicated-byte submodes reserve zero. All rotate
    // submodes are valid because they prepend the implicit leading one.
    if imm12 >> 10 == 0 && (imm12 >> 8) & 3 != 0 && imm12 & 0xff == 0 {
        return false;
    }
    let opcode = (first >> 5) & 0x1f;
    let set_flags = first & 0x10 != 0;
    let rn = first & 15;
    let rd = (second >> 8) & 15;
    match opcode {
        // AND/ANDS, including the flag-only TST alias.
        0 => rn <= 12 && (rd <= 12 || (set_flags && rd == 15)),
        // ORR/ORRS with Rn=PC is MOV/MOVS immediate. No other ORR form is
        // admitted here, and the destination must remain a general register.
        2 => rn == 15 && rd <= 12,
        // ADD/ADDS and SUB/SUBS. Rd=PC is accepted only for the flag-only
        // CMN/CMP aliases; SP and PC are never arithmetic operands.
        8 | 13 => rn <= 12 && (rd <= 12 || (set_flags && rd == 15)),
        // ADC/ADCS and SBC/SBCS use APSR.C as an input and may update APSR
        // flags, but have no control-state effects with ordinary registers.
        10 | 11 => rn <= 12 && rd <= 12,
        _ => false,
    }
}

/// Count the executed instruction prefix reported by a memory-hook restart.
/// Admission constraints on those already executed entries need no rescan.
pub(super) fn count_entries(mut code: &[u8]) -> Result<u64, String> {
    let mut entries = 0;
    while !code.is_empty() {
        let Some(&[first, second, ..]) = code.get(..2) else {
            return Err("M-profile block ends inside an instruction".into());
        };
        let size = thumb_size(u16::from_le_bytes([first, second]));
        code = code.get(size..).ok_or("M-profile block ends inside an instruction")?;
        entries += 1;
    }
    Ok(entries)
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
        let size = thumb_size(word);
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
    #[test]
    fn inline_classifier_accepts_only_register_alu_and_immediate_branches() {
        for instruction in [
            0x0001, 0x180a, 0x4008, 0x42a1, 0xb2ad, 0x4400, 0x4481, 0x45b0, 0x4605, 0x46b1, 0xd001,
            0xddff, 0xe7ff, 0xb000, 0xb080, 0xa800, 0xafff, 0x46bd, 0x466f, 0xb170, 0xb169,
            0xb300, 0xb37f, 0xb970, 0xb969, 0xbb00, 0xbb7f,
        ] {
            assert!(can_run_inline(instruction), "{instruction:#06x}");
        }
        for instruction in [
            0x4485, 0x4478, 0x4501, 0x4700, 0x4770, 0xb400, 0xbe00, 0xbf00, 0xde00, 0xdf00,
            0xe800,
        ] {
            assert!(!can_run_inline(instruction), "{instruction:#06x}");
        }

        // The first halfword of a 32-bit encoding occupies the upper half.
        assert!(can_run_inline(0xf000_0107)); // and.w r1,r0,#7
        assert!(can_run_inline(0xf000_1101)); // and.w with byte replicated at 0/16
        assert!(can_run_inline(0xf000_2101)); // and.w with byte replicated at 8/24
        assert!(can_run_inline(0xf000_3101)); // and.w with byte replicated four times
        assert!(can_run_inline(0xf500_0101)); // add.w with a rotated immediate
        assert!(can_run_inline(0xf1b1_4f00)); // cmp.w r1,#0x80000000
        assert!(can_run_inline(0xf011_0504)); // ands.w r5,r1,#4
        assert!(can_run_inline(0xf101_0007)); // add.w r0,r1,#7
        assert!(can_run_inline(0xf111_0007)); // adds.w r0,r1,#7
        assert!(can_run_inline(0xf1a1_0007)); // sub.w r0,r1,#7
        assert!(can_run_inline(0xf1b1_0007)); // subs.w r0,r1,#7
        assert!(can_run_inline(0xf1b1_0f07)); // cmp r1,#7
        assert!(can_run_inline(0xf04f_0007)); // mov.w r0,#7
        assert!(can_run_inline(0xf05f_0cff)); // movs.w r12,#255
        assert!(can_run_inline(0xf173_36ff)); // sbcs.w r6,r3,#255
        assert!(can_run_inline(0xf179_3cff)); // sbcs.w r12,r9,#255
        assert!(can_run_inline(0xf240_0001)); // movw r0,#1
        assert!(can_run_inline(0xf2c0_0001)); // movt r0,#1
        assert!(can_run_inline(0xf640_0c01)); // movw r12 with i=1
        assert!(can_run_inline(0xf6c0_0c01)); // movt r12 with i=1
        assert!(can_run_inline(0xea4f_4410)); // shift alias with Rn=15
        assert!(can_run_inline(0xea11_0f02)); // TST, no PC write
        assert!(can_run_inline(0xea91_0f02)); // TEQ, no PC write
        assert!(!can_run_inline(0xea0f_0002)); // AND reads PC
        assert!(!can_run_inline(0xea01_0f02)); // no TST alias without S
        assert!(!can_run_inline(0xea4f_440f)); // shifted operand PC
        assert!(can_run_inline(0xeb41_71e2)); // adc.w r1,r1,r2,asr #31
        assert!(can_run_inline(0xfba5_2302)); // umull r2,r3,r5,r2
        assert!(can_run_inline(0xfb85_2302)); // smull r2,r3,r5,r2
        assert!(can_run_inline(0xfa5f_f389)); // uxtb.w r3,r9
        assert!(can_run_inline(0xfa93_f3a3)); // rbit r3,r3
        assert!(!can_run_inline(0xfba5_2202)); // repeated destination
        assert!(!can_run_inline(0xfa93_f3a4)); // inconsistent REV operands
        assert!(can_run_inline(0xfb04_f303)); // mul r3,r4,r3
        assert!(can_run_inline(0xfb03_4401)); // mla r4,r3,r1,r4
        assert!(can_run_inline(0xfb03_4411)); // mls r4,r3,r1,r4
        assert!(can_run_inline(0xfa03_f101)); // lsl.w r1,r3,r1
        assert!(can_run_inline(0x45b6)); // cmp lr,r6
        assert!(!can_run_inline(0x45b7 | 0x78)); // cmp with PC
        assert!(!can_run_inline(0xfb03_f411)); // MLS with no accumulator
        assert!(!can_run_inline(0xfa03_f10f)); // PC shift count
        assert!(can_run_inline(0xf006_fcf7)); // BL
        assert!(can_run_inline(0xf000_81a6)); // BEQ.W
        assert!(can_run_inline(0xf000_b940)); // B.W
        assert!(!can_run_inline(0xf000_e802)); // BLX
        assert!(!can_run_inline(0xf380_8100)); // reserved condition / system
        assert!(!can_run_inline(0xfb04_f30f)); // MUL source PC
        assert!(can_run_inline(0xeb01_0002)); // add.w r0,r1,r2
        assert!(can_run_inline(0xeb11_1042)); // adds.w r0,r1,r2,lsl #5
        assert!(can_run_inline(0xeba1_0002)); // sub.w r0,r1,r2
        assert!(can_run_inline(0xebb1_1042)); // subs.w r0,r1,r2,lsl #5
        assert!(can_run_inline(0xf3ef_8110)); // mrs r1,primask
        assert!(can_run_inline(0xf3ef_8211)); // mrs r2,basepri
        assert!(can_run_inline(0xf3ef_8313)); // mrs r3,faultmask
        for instruction in [
            0xf101_0d07, // destination SP
            0xf101_0f07, // PC destination without compare semantics
            0xf10d_0007, // source SP
            0xf10f_0007, // source PC / ADR alias
            0xf1ad_0007, // SUB source SP
            0xf1af_0007, // SUB source PC
            0xf1a1_0f07, // PC destination without CMP flag semantics
            0xf1bd_0f07, // CMP source SP
            0xf1bf_0f07, // CMP source PC
            0xf173_3fff, // SBCS destination PC
            0xf17d_36ff, // SBCS source SP
            0xf17f_36ff, // SBCS source PC
            0xf240_0d01, // MOVW destination SP
            0xf240_0f01, // MOVW destination PC
            0xf2c0_0d01, // MOVT destination SP
            0xf2c0_0f01, // MOVT destination PC
            0xf240_8001, // reserved bit 15
            0xf250_0001, // undefined MOVW-neighbor opcode
            0xeb0d_0002, // ADD source SP
            0xeb0f_0002, // ADD source PC
            0xeb01_0d02, // ADD destination SP
            0xeb01_0f02, // ADD destination PC
            0xeb01_000d, // ADD shifted source SP
            0xeb01_000f, // ADD shifted source PC
            0xeb01_8002, // reserved second-halfword bit
            0xeba1_000d, // SUB shifted source SP
            0xf3ef_8112, // BASEPRI_MAX not included in the native read subset
            0xf3ef_8114, // CONTROL not included in the native read subset
            0xf3ef_8d10, // MRS destination SP
            0xf3ef_8f10, // MRS destination PC
            0xf3ef_9110, // reserved MRS prefix bits
            0xf3ee_8110, // not the MRS mask-register encoding
            0xf3ef_8001, // emulated xPSR selector
            0xf3ef_8003, // emulated xPSR selector
            0xf3ef_8005, // emulated xPSR selector
            0xf3ef_8007, // emulated xPSR selector
            0xf3ef_8000, // APSR selector outside the emulated xPSR subset
            0x46bf, // MOV SP,PC
            0x4677, // MOV R7,LR
            0xb500, // PUSH including LR, not CBZ
            0xbd00, // POP including PC, not CBNZ
            0xf000_1100, // replicated-zero submode 01 is reserved
            0xf000_2100, // replicated-zero submode 10 is reserved
            0xf000_3100, // replicated-zero submode 11 is reserved
            0xf121_0007, // ADC opcode
            0xf021_0007, // BIC opcode
            0xf04f_0f07, // MOV destination PC
            0xf04d_0007, // MOV alias with SP in Rn field
        ] {
            assert!(!can_run_inline(instruction), "{instruction:#010x}");
        }
        assert!(can_run_inline(0xf000_f800)); // BL
        assert!(!can_run_inline(0xf8c0_1000)); // str.w
        assert!(!can_run_inline(0x0001_0001)); // unrecognized prefix
    }
    #[test]
    fn single_transfer_classifier_accepts_only_single_register_memory_forms() {
        // Register-offset transfers, covering word/byte/halfword and signed
        // byte/halfword loads.
        for instruction in [
            0x5000, 0x5200, 0x5400, 0x5600, 0x5800, 0x5a00, 0x5c00, 0x5e00,
            0x5fff,
        ] {
            assert!(is_single_transfer(instruction), "{instruction:#06x}");
        }
        // Immediate word/byte/halfword forms and SP-relative STR/LDR.
        for instruction in [
            0x6000, 0x6800, 0x7000, 0x7800, 0x8000, 0x8800, 0x9000, 0x9800,
            0x9fff, 0x4800, 0x4fff,
        ] {
            assert!(is_single_transfer(instruction), "{instruction:#06x}");
        }
        // Positive-offset Thumb-2 LDR/STR word, byte and halfword, signed
        // byte/halfword loads, and a legal PC-relative word literal read.
        for instruction in [
            0xf8c1_0004,
            0xf8dd_200c,
            0xf885_40ff,
            0xf896_7004,
            0xf8a6_5006,
            0xf8b3_2008,
            0xf992_1003,
            0xf9b4_3004,
            0xf8df_0004,
        ] {
            assert!(is_single_transfer(instruction), "{instruction:#010x}");
        }
        // T32 immediate-indexed transfers, including the sampled negative
        // pre-index load and post-index load into LR.
        for instruction in [
            0xf853_3c04, // LDR r3,[r3,#-4]
            0xf85c_eb04, // LDR lr,[r12],#4
            0xf841_0f04, // STR r0,[r1,#4]!
            0xf851_0f04, // LDR r0,[r1,#4]!
            0xf812_3b08, // LDRB r3,[r2],#8
            0xf82a_2d04, // LDRH r2,[r10,#-4]
            0xf83a_3c13, // LDRH r3,[r10,#-19]
            0xf843_3c04, // STR r3,[r3,#-4]
            0xf84c_eb04, // STR lr,[r12],#4
        ] {
            assert!(is_single_transfer(instruction), "{instruction:#010x}");
        }
        // T32 register-offset forms have one transfer and no base writeback.
        // Include the sampled LDRH.W r3,[r10,r3,LSL #1] table-scan form.
        for instruction in [
            0xf800_3012, // STRB r3,[r0,r2]
            0xf810_3012, // LDRB r3,[r0,r2]
            0xf820_3012, // STRH r3,[r0,r2]
            0xf830_3013, // LDRH r3,[r0,r3,LSL #1]
            0xf83a_3013, // LDRH r3,[r10,r3,LSL #1]
            0xf840_3012, // STR r3,[r0,r2]
            0xf850_3012, // LDR r3,[r0,r2]
            0xf910_3012, // LDRSB r3,[r0,r2]
            0xf930_3012, // LDRSH r3,[r0,r2]
        ] {
            assert!(is_single_transfer(instruction), "{instruction:#010x}");
        }
        // PUSH/POP, LDM/STM, undefined and special encodings, and unsupported
        // 32-bit memory or exclusive operations have different effects.
        for instruction in [
            0xb400, 0xbc00, 0xc000, 0xc800, 0xde00, 0xdf00, 0xf3bf_8f4f,
            0xe850_1f00, // LDREX
            0xf841_0f04, // STR with writeback
            0xf851_0f04, // LDR with writeback
            0xf8e1_0004, // translation variant
            0xf8c1_f004, // STR from PC
            0xf8d1_d004, // LDR to SP
            0xf8d1_f004, // LDR to PC
            0xf8cf_0004, // STR with PC base
            0xf8df_d004, // literal LDR to SP
            0xf8df_f004, // literal LDR to PC
            0xf83a_d013, // register-offset LDRH to SP
            0xf83a_f013, // register-offset LDRH to PC
            0xf83f_3013, // register-offset LDRH from PC base
            0xf83a_301f, // register-offset LDRH with PC offset
            0xf853_3404, // bit 11 must identify the indexed immediate form
            0xf853_3a04, // invalid P=0,W=0 indexed mode
            0xf853_dc04, // indexed load to SP
            0xf853_fc04, // indexed load to PC
            0xf85d_eb04, // indexed base SP
            0xf85f_eb04, // indexed base PC
            0xf853_3d04, // indexed load with base writeback and Rt=Rn
            0x0001_6000,
            0xffff_5000,
        ] {
            assert!(!is_single_transfer(instruction), "{instruction:#010x}");
        }
    }
    fn parts(words: &[u16]) -> (u64, u64, u64, bool) {
        let scan = scan(&code(words)).unwrap();
        (scan.entries, scan.unit, scan.last, scan.returns_through_lr)
    }
    #[test]
    fn restart_count_uses_the_same_thumb_widths_as_admission() {
        let mixed = code(&[0x3001, 0xf240, 0x4770, 0x4770]);
        for end in [2, 6, 8] {
            assert_eq!(count_entries(&mixed[..end]).unwrap(), scan(&mixed[..end]).unwrap().entries);
        }
        for end in [1, 3, 4, 5, 7] {
            assert!(count_entries(&mixed[..end]).is_err());
        }
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
    #[test]
    fn the_scan_cache_answers_only_for_the_bytes_it_was_given() {
        let mut cache = ScanCache::default();
        let first = code(&[0x3001, 0x4770]);
        let scanned = scan(&first).unwrap();
        assert_eq!(cache.get(0x100, &first), None);
        cache.put(0x100, ScanCache::key(&first).unwrap(), scanned);
        assert_eq!(cache.get(0x100, &first), Some(scanned));
        // Other bytes, a shorter or longer block, another address.
        assert_eq!(cache.get(0x100, &code(&[0x3001, 0x4718])), None);
        assert_eq!(cache.get(0x100, &first[..2]), None);
        assert_eq!(cache.get(0x100, &code(&[0x3001, 0x4770, 0x3001])), None);
        assert_eq!(cache.get(0x102, &first), None);
        // Full and shortened translations of the same code keep both scans.
        let short = &first[..2];
        let short_scan = scan(short).unwrap();
        cache.put(0x100, ScanCache::key(short).unwrap(), short_scan);
        assert_eq!(cache.get(0x100, &first), Some(scanned));
        assert_eq!(cache.get(0x100, short), Some(short_scan));
        // Equal-length translations can still collide and replace each other.
        let other = 0x100 + (ScanCache::SLOTS as u64) * 2;
        cache.put(other, ScanCache::key(&first).unwrap(), scanned);
        assert_eq!(cache.get(0x100, &first), None);
        assert_eq!(cache.get(other, &first), Some(scanned));
        assert_eq!(cache.get(0x100, short), Some(short_scan));
        // Empty and long blocks are not kept.
        assert!(ScanCache::key(&[]).is_none());
        assert!(ScanCache::key(&[0; ScanCache::BYTES + 1]).is_none());
        assert!(ScanCache::key(&[0; ScanCache::BYTES]).is_some());
    }
}
