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
    /// The board store hook sees only watched pages (code pages) and device
    /// pages; other stores keep the engine's inline path and no longer end
    /// their block. Temporarily disabled while an exclusive reservation needs
    /// every store.
    pub fast_stores: bool,
    /// Board store hook installed for this processor, used to toggle watched
    /// filtering immediately when an exclusive reservation starts or clears.
    pub store_hook: Option<fujimod_emulator_cpu::HookId>,
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
            scan: Scan {
                entries: 0,
                unit: 0,
                last: 0,
                returns_through_lr: false,
            },
        };
        Self {
            entries: vec![empty; Self::SLOTS].into_boxed_slice(),
        }
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
        self.entries[Self::slot(pc, usize::from(len))] = ScanEntry {
            pc,
            len,
            code,
            scan,
        };
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
/// a 16-bit or 32-bit immediate-offset LDR, LDRB or LDRH (into R0-R12 from
/// R0-R12); then 16-bit shift, add, subtract, move, compare and
/// data-processing instructions on low registers, sign and zero extension,
/// and 32-bit data processing with an immediate into R0-R12 or as a
/// comparison; then a 16-bit or 32-bit conditional branch to the load. None
/// of them stores, loads from memory, changes control state or leaves the
/// loop except by falling through the branch.
pub(super) fn poll_period(code: &[u8]) -> Option<u64> {
    let half = |offset: usize| {
        code.get(offset..offset + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    let head = half(0)?;
    let (mut offset, mut count) = if matches!(head & 0xf800, 0x6800 | 0x7800 | 0x8800) {
        (2usize, 1u64)
    } else if matches!(head & 0xfff0, 0xf8d0 | 0xf8b0 | 0xf890) {
        let second = half(2)?;
        let rn = head & 15;
        let rt = second >> 12;
        if rn > 12 || rt > 12 {
            return None;
        }
        (4usize, 1u64)
    } else {
        return None;
    };
    while count < 8 {
        let word = half(offset)?;
        if word >> 11 >= 0x1d {
            let second = half(offset + 2)?;
            if word & 0xf800 == 0xf000
                && second & 0xd000 == 0x8000
                && (word >> 6) & 15 < 14
            {
                let s = u32::from((word >> 10) & 1);
                let j1 = u32::from((second >> 13) & 1);
                let j2 = u32::from((second >> 11) & 1);
                let imm6 = u32::from(word & 0x3f);
                let imm11 = u32::from(second & 0x7ff);
                let raw = (s << 20) | (j2 << 19) | (j1 << 18) | (imm6 << 12) | (imm11 << 1);
                let disp = ((raw as i32) << 11) >> 11;
                let target = offset as i64 + 4 + i64::from(disp);
                return (target == 0).then_some(count + 1);
            }
            let rd = (second >> 8) & 15;
            // TST, TEQ, CMN, CMP: flags only.
            let compares =
                rd == 15 && word & 0x0210 == 0x0010 && matches!((word >> 5) & 15, 0 | 4 | 8 | 13);
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

/// Whether a Thumb instruction is a legal stack PUSH (16-bit or 32-bit) that
/// writes only ordinary registers and LR (never PC or SP). Keeping it terminal
/// lets each watched or device write complete without restarting earlier words.
pub(super) fn is_terminal_push(word: u32) -> bool {
    if word <= u32::from(u16::MAX) {
        let instruction = word as u16;
        return instruction & 0xfe00 == 0xb400 && instruction & 0x01ff != 0;
    }
    let first = (word >> 16) as u16;
    let second = word as u16;
    first == 0xe92d && second & 0xa000 == 0 && second.count_ones() >= 2
}

/// Whether a Thumb instruction is a multiple transfer that is not a stack
/// operation (`LDM`/`STM`, the `0xe880`/`0xe8a0`.. forms where bits 9:7 select
/// increment/decrement). `Some(true)` for a store, whose independent writes
/// make it terminal like a PUSH; `Some(false)` for a load that names no PC,
/// which may fault and be redone. `None` for anything else, including a load
/// that writes PC (an exception return the controller performs).
pub(super) fn multiple_transfer(word: u32) -> Option<bool> {
    if word <= u32::from(u16::MAX) {
        return None;
    }
    let first = (word >> 16) as u16;
    let second = word as u16;
    // `1110 1000/1001 (01|10) W L Rn`: bit 6 clear selects the multiple
    // forms (the dual and table forms set it), bits 8:7 are 01 for increment
    // after and 10 for decrement before, bit 5 is the writeback and bit 4 the
    // load.
    if first & 0xfe40 != 0xe800 || !matches!(first & 0x0180, 0x0080 | 0x0100) {
        return None;
    }
    let rn = first & 15;
    let writeback = first & 0x0020 != 0;
    let load = first & 0x0010 != 0;
    if rn > 12 || second == 0 || (writeback && second & (1 << rn) != 0) {
        return None;
    }
    if load {
        // A PC load is an exception return; SP cannot be written by a
        // multiple load alongside other registers.
        if second & 0xa000 != 0 {
            return None;
        }
        return Some(false);
    }
    Some(true)
}

/// Whether a Thumb instruction is a legal stack POP (16-bit or 32-bit) that
/// excludes PC (and SP in 32-bit form). Because it performs only memory loads
/// and cannot trigger EXC_RETURN, it is safe to run inline in recoverable
/// blocks.
pub(super) fn is_non_pc_pop(word: u32) -> bool {
    if word <= u32::from(u16::MAX) {
        let instruction = word as u16;
        return instruction & 0xff00 == 0xbc00 && instruction & 0x00ff != 0;
    }
    let first = (word >> 16) as u16;
    let second = word as u16;
    first == 0xe8bd && second & 0xa000 == 0 && second.count_ones() >= 2
}

/// Whether a Thumb instruction is a legal stack POP (16-bit or 32-bit) that
/// loads PC (and excludes SP/LR in 32-bit form). Safe to run inline in Thread
/// Mode where EXC_RETURN cannot legally occur; Handler Mode keeps inline
/// accounting disarmed so `block_entered` routes EXC_RETURN to the controller.
pub(super) fn is_pc_pop(word: u32) -> bool {
    if word <= u32::from(u16::MAX) {
        let instruction = word as u16;
        return instruction & 0xff00 == 0xbd00;
    }
    let first = (word >> 16) as u16;
    let second = word as u16;
    first == 0xe8bd && second & 0x8000 != 0 && second & 0x6000 == 0 && second.count_ones() >= 2
}

/// Whether a Thumb instruction is `BX LR` (`0x4770`). Safe to run inline in
/// Thread Mode where EXC_RETURN cannot legally occur.
pub(super) fn is_bx_lr(word: u32) -> bool {
    word == 0x4770
}

/// Whether a Thumb instruction writes a mask or CONTROL register with `MSR`
/// (`0xf380..0xf38f`, `second & 0xff00 == 0x8800`). The engine executes the
/// write itself; it only has to end its block so the next entry rescans the
/// exception masks. `unmasks` already ends the block after it.
pub(super) fn is_mask_msr(word: u32) -> bool {
    word > u32::from(u16::MAX)
        && (word >> 16) as u16 & 0xfff0 == 0xf380
        && word as u16 & 0xff00 == 0x8800
}

/// Whether a Thumb instruction is `CPS` (`0xb660..0xb66f` without the
/// reserved bits). It changes PRIMASK, FAULTMASK or CONTROL, so it must end
/// its block; the engine executes the change itself.
pub(super) fn is_cps(word: u32) -> bool {
    word <= u32::from(u16::MAX) && (word as u16) & 0xffe8 == 0xb660
}

/// Whether a Thumb instruction is a register-only bitfield operation
/// (`SBFX`, `BFI`, `BFC`, `UBFX`). Each writes one ordinary register from
/// another and no flags, so it cannot fault or change control state.
/// `BFC` names `Rn=15`; every other field must be an ordinary register.
pub(super) fn is_bitfield(word: u32) -> bool {
    if word <= u32::from(u16::MAX) {
        return false;
    }
    let first = (word >> 16) as u16;
    let second = word as u16;
    let rn = first & 15;
    let rd = (second >> 8) & 15;
    // The three prefixes are `SBFX` (`0xf340`), `BFI`/`BFC` (`0xf360`, whose
    // `Rn=15` is the cleared-source alias) and `UBFX` (`0xf3c0`). Bit 15 and
    // bit 5 of the second halfword are reserved and must be zero.
    if second & 0x8020 != 0 || rd > 12 {
        return false;
    }
    match first & 0xfff0 {
        // `BFI` from R0-R12, or `BFC` (the `Rn=15` cleared-source alias).
        0xf360 => rn <= 12 || rn == 15,
        0xf340 | 0xf3c0 => rn <= 12,
        _ => false,
    }
}

/// Whether a Thumb instruction is a dual-register transfer (`LDRD`/`STRD`,
/// the `P`/`U`/`W` forms of `0xe900..0xe9ff`). The pair must be two ordinary
/// registers other than the base, which is the encoding's predictability
/// rule. Returns `Some(true)` for a store, whose two independent writes make
/// it terminal the same way a PUSH is.
pub(super) fn dual_transfer(word: u32) -> Option<bool> {
    if word <= u32::from(u16::MAX) {
        return None;
    }
    let first = (word >> 16) as u16;
    let second = word as u16;
    // `1110_100 P U 1 W L Rn`: bit 6 is set for `LDRD`/`STRD`, and clear for
    // the load/store-multiple forms that share the prefix.
    if first & 0xfe40 != 0xe840 {
        return None;
    }
    let rn = first & 15;
    let rt = (second >> 12) & 15;
    let rt2 = (second >> 8) & 15;
    let pre_index = first & 0x0100 != 0;
    let writeback = first & 0x0020 != 0;
    // `Rt2 = 15` is the exclusive space sharing this prefix, and P=0,W=0 is
    // reserved; a writeback base that is also a target is unpredictable.
    if !(rn <= 14
        && rt <= 12
        && rt2 <= 12
        && rt != rt2
        && (pre_index || writeback)
        && !(writeback && (rn == rt || rn == rt2)))
    {
        return None;
    }
    // Bit 20 is `L`: clear for `STRD`.
    Some(first & 0x0010 == 0)
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
            // The fixed-register REV family: a byte/halfword permute of two
            // ordinary low registers, no flags and no memory.
            || matches!(instruction & 0xffc0, 0xba00 | 0xba40 | 0xbac0)
            // NOP and YIELD execute without touching state; WFE, WFI, SEV,
            // SEVL, DBG and the reserved hints stay observed.
            || matches!(instruction, 0xbf00 | 0xbf10)
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
    // Dual-register and multiple loads perform ordinary single accesses and
    // may fault on the new path only to be redone; their stores are terminal
    // (`dual_transfer`, `multiple_transfer`) and the caller decides them.
    if dual_transfer(word) == Some(false) || multiple_transfer(word) == Some(false) {
        return true;
    }
    // Data-synchronization barriers order this processor's own accesses; they
    // store nothing, change no register and cannot fault.
    if word >> 16 == 0xf3bf && matches!(word as u16 & 0xfff0, 0x8f40 | 0x8f50 | 0x8f60) {
        return word as u16 & 0x000f == 0x000f;
    }
    // Register-only bitfield operations. `BFC` clears bits with Rn=15.
    if is_bitfield(word) {
        return true;
    }
    // Native MRS reads mask state from Unicorn's M-profile architectural
    // helper. The controller emulates xPSR-family selectors 1/3/5/7; permit
    // only the exact PRIMASK, BASEPRI, and FAULTMASK selectors here.
    if first == 0xf3ef {
        return rd <= 12 && matches!(second & 0xf0ff, 0x8008 | 0x8009 | 0x8010 | 0x8011 | 0x8013);
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
    if matches!(first & 0xfff0, 0xfa00 | 0xfa10 | 0xfa40 | 0xfa50) && second & 0xf0c0 == 0xf080 {
        let rn = first & 15;
        return (rn <= 12 || rn == 15) && rd <= 12 && second & 15 <= 12;
    }
    if first & 0xfff0 == 0xfa90 && second & 0xf0c0 == 0xf080 {
        return first & 15 == second & 15 && rd <= 12 && second & 15 <= 12;
    }
    // SMULL/UMULL write two distinct ordinary registers.
    if matches!(first & 0xfff0, 0xfb80 | 0xfba0) {
        let rd_lo = second >> 12;
        return second & 0xf0 == 0
            && first & 15 <= 12
            && second & 15 <= 12
            && rd_lo <= 12
            && rd <= 12
            && rd_lo != rd;
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
        // AND/EOR and their flag-only TST/TEQ aliases. ADD and SUB
        // use the same destination restriction for CMN/CMP.
        0 | 4 | 8 | 13 => rn <= 12 && (rd <= 12 || set_flags && rd == 15),
        // BIC uses ordinary registers. ORR/ORN also permit Rn=15
        // for the MOV/MVN immediate aliases.
        1 => rn <= 12 && rd <= 12,
        2 | 3 => (rn <= 12 || rn == 15) && rd <= 12,
        // ADC/ADCS and SBC/SBCS use APSR.C as an input and may update APSR
        // flags, but have no control-state effects with ordinary registers.
        10 | 11 => rn <= 12 && rd <= 12,
        // RSB with an immediate computes `imm - Rn` in one register. Like
        // SUB, its flag-only alias (`Rd=15` with S) is CMP-with-negation.
        14 => rn <= 12 && (rd <= 12 || set_flags && rd == 15),
        _ => false,
    }
}

/// An IT instruction or a register-only member with no branch or memory
/// restart. Unicorn evaluates conditions and preserves ITSTATE at TB exits.
pub(super) fn can_run_in_it(word: u32) -> bool {
    if word <= u32::from(u16::MAX) {
        let instruction = word as u16;
        if instruction & 0xff00 == 0xbf00 {
            return instruction & 15 != 0 && (instruction >> 4) & 15 < 14;
        }
        if matches!(instruction, 0xd000..=0xe7ff) || instruction & 0xf500 == 0xb100 {
            return false;
        }
    } else if word >> 27 == 0b11110 && word & 0x8000 != 0 {
        // The wide branch space also contains system instructions. Leave
        // those observed when conditional, including mask-register reads.
        return false;
    }
    can_run_inline(word)
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
        code = code
            .get(size..)
            .ok_or("M-profile block ends inside an instruction")?;
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
            0xddff, 0xe7ff, 0xb000, 0xb080, 0xa800, 0xafff, 0x46bd, 0x466f, 0xb170, 0xb169, 0xb300,
            0xb37f, 0xb970, 0xb969, 0xbb00, 0xbb7f,
        ] {
            assert!(can_run_inline(instruction), "{instruction:#06x}");
        }
        for instruction in [
            0x4485, 0x4478, 0x4501, 0x4700, 0x4770, 0xb400, 0xbe00, 0xbf30, 0xde00, 0xdf00, 0xe800,
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
        assert!(can_run_inline(0xf041_0101)); // orr r1,r1,#1
        assert!(can_run_inline(0xf021_0007)); // bic r0,r1,#7
        assert!(can_run_inline(0xf3ef_8c09)); // mrs r12,psp
        assert!(!can_run_inline(0xf041_0f01)); // ORR destination PC
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
            0x46bf,      // MOV SP,PC
            0x4677,      // MOV R7,LR
            0xb500,      // PUSH including LR, not CBZ
            0xbd00,      // POP including PC, not CBNZ
            0xf000_1100, // replicated-zero submode 01 is reserved
            0xf000_2100, // replicated-zero submode 10 is reserved
            0xf000_3100, // replicated-zero submode 11 is reserved
            0xf121_0007, // ADC opcode
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
    fn it_classifier_admits_pure_members_without_control_transfers() {
        for word in [0xbf0c, 0xbfb8, 0x2001, 0xf240_0301, 0xfba5_2302] {
            assert!(can_run_in_it(word), "{word:#x}");
        }
        for word in [
            0xbf00,
            0xbff8,
            0xb672,
            0xbf30,
            0x6008,
            0xb430,
            0x4770,
            0xb170,
            0xd001,
            0xf006_fcf7,
            0xf3ef_8110,
            0xf8d0_0004,
        ] {
            assert!(!can_run_in_it(word), "{word:#x}");
        }
    }

    #[test]
    fn new_inline_classes_accept_only_their_pure_or_recoverable_forms() {
        // REV, REV16, REVSH permute two low registers; the hints NOP and
        // YIELD touch no state. The observed hints stay refused.
        for word in [0xba11, 0xba63, 0xbaf5, 0xbf00, 0xbf10, 0xb251, 0xb29d] {
            assert!(can_run_inline(word), "{word:#06x}");
        }
        for word in [0xbf20, 0xbf30, 0xbf40, 0xbf50, 0xbf60, 0xbf70, 0xbf80, 0xbfb0] {
            assert!(!can_run_inline(word), "{word:#06x}");
        }
        // SBFX, BFI, BFC, UBFX write one ordinary register from another and
        // set no flags; SP, PC and the reserved field stay refused.
        for word in [0xf366_0305, 0xf36f_0385, 0xf3c1_0000, 0xf341_0041] {
            assert!(is_bitfield(word), "{word:#010x}");
            assert!(can_run_inline(word), "{word:#010x}");
        }
        for word in [
            0xf366_0d05, // BFI destination SP
            0xf366_0f05, // BFI destination PC
            0xf36f_0f85, // BFC destination PC
            0xf3c1_0d00, // UBFX destination SP
            0xf3c0_0020, // reserved bit 5
            0xf3c1_8000, // reserved bit 15
        ] {
            assert!(!is_bitfield(word), "{word:#010x}");
            assert!(!can_run_inline(word), "{word:#010x}");
        }
        // LDRD/STRD pre-index, post-index and negative forms. A load is
        // inline; a store is terminal so each write completes on its own.
        for (word, store) in [
            (0xe9d2_0102u32, false),
            (0xe952_0102, false),
            (0xe8f2_0102, false),
            (0xe9f2_0102, false),
            (0xe9c2_0102, true),
            (0xe942_0102, true),
            (0xe8e2_0102, true),
            (0xe9e2_0102, true),
        ] {
            assert_eq!(dual_transfer(word), Some(store), "{word:#010x}");
            assert_eq!(can_run_inline(word), !store, "{word:#010x}");
        }
        for word in [
            0xe9d2_0f02, // Rt2 = PC: the exclusive prefix
            0xe9d2_0d02, // Rt2 = SP
            0xe9d2_1102, // repeated pair register
            0xe9d2_0d02, // pair (Rt2) names SP
            0xe9f5_5102, // writeback base equals Rt2
            0xe9e2_2102, // writeback base equals the first target
            0xe8d2_0102, // P=0,W=0 reserved
        ] {
            assert_eq!(dual_transfer(word), None, "{word:#010x}");
            assert!(!can_run_inline(word), "{word:#010x}");
        }
        // MSR to a mask register and CPS end their block; both are decided
        // from the encoding alone and execute in the engine.
        for word in [0xf380_8810u32, 0xf38c_8813, 0xf381_8811] {
            assert!(is_mask_msr(word), "{word:#010x}");
        }
        for word in [0xf380_8010u32, 0xf3ef_8810, 0xf380_9800] {
            assert!(!is_mask_msr(word), "{word:#010x}");
        }
        for word in [0xb660u32, 0xb661, 0xb662, 0xb671, 0xb672] {
            assert!(is_cps(word), "{word:#06x}");
        }
        // Bit 4 is reserved and selects the A-profile-only `cps` form; `b670`
        // is `cpsid i`/`cpsie i` with both the A and I bits set.
        for word in [0xb660u32 | 0x0008, 0xb661_0000] {
            assert!(!is_cps(word), "{word:#06x}");
        }
    }

    #[test]
    fn single_transfer_classifier_accepts_only_single_register_memory_forms() {
        // Register-offset transfers, covering word/byte/halfword and signed
        // byte/halfword loads.
        for instruction in [
            0x5000, 0x5200, 0x5400, 0x5600, 0x5800, 0x5a00, 0x5c00, 0x5e00, 0x5fff,
        ] {
            assert!(is_single_transfer(instruction), "{instruction:#06x}");
        }
        // Immediate word/byte/halfword forms and SP-relative STR/LDR.
        for instruction in [
            0x6000, 0x6800, 0x7000, 0x7800, 0x8000, 0x8800, 0x9000, 0x9800, 0x9fff, 0x4800, 0x4fff,
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
            0xb400,
            0xbc00,
            0xc000,
            0xc800,
            0xde00,
            0xdf00,
            0xf3bf_8f4f,
            0xe850_1f00, // LDREX
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
            assert_eq!(
                count_entries(&mixed[..end]).unwrap(),
                scan(&mixed[..end]).unwrap().entries
            );
        }
        for end in [1, 3, 4, 5, 7] {
            assert!(count_entries(&mixed[..end]).is_err());
        }
    }
    #[test]
    fn poll_period_accepts_only_register_computation_between_a_load_and_its_branch() {
        // ldrh r5,[r2]; and r5,r5,#4; uxth r5,r5; cmp r5,#0; beq .-10
        assert_eq!(
            poll_period(&code(&[0x8815, 0xf005, 0x0504, 0xb2ad, 0x2d00, 0xd0f9])),
            Some(5)
        );
        // ldr r2,[r0]; cmp r2,r3; beq .-4
        assert_eq!(poll_period(&code(&[0x6802, 0x429a, 0xd0fc])), Some(3));
        // ldrb r1,[r0]; tst r1,#0x80; bne .-6 (TST writes no register).
        assert_eq!(
            poll_period(&code(&[0x7801, 0xf011, 0x0f80, 0xd1fb])),
            Some(3)
        );
        // The branch goes elsewhere, or forward.
        assert_eq!(poll_period(&code(&[0x6802, 0x429a, 0xd0fb])), None);
        assert_eq!(poll_period(&code(&[0x6802, 0x429a, 0xd000])), None);
        // Not a load first: str r2,[r0]; a register-offset or literal load.
        assert_eq!(poll_period(&code(&[0x6002, 0x429a, 0xd0fc])), None);
        assert_eq!(poll_period(&code(&[0x5842, 0x429a, 0xd0fc])), None);
        assert_eq!(poll_period(&code(&[0x4a02, 0x429a, 0xd0fc])), None);
        // A store, a second load, a push, a call, a high-register move, an IT
        // instruction, SVC, or CPS in the body.
        for body in [
            0x6001u16, 0x6801, 0xb401, 0x4780, 0x4686, 0xbf08, 0xdf00, 0xb672, 0x4770,
        ] {
            assert_eq!(
                poll_period(&code(&[0x6802, body, 0xd0fc])),
                None,
                "{body:#x}"
            );
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
            assert_eq!(
                poll_period(&code(&[0x6802, first, second, 0xd0fb])),
                None,
                "{first:#x}"
            );
        }
        // movw sp,#0 and add.w pc,r1,#4 write SP and PC; ands.w with Rd 15
        // and no S is not a comparison.
        assert_eq!(poll_period(&code(&[0x6802, 0xf240, 0x0d00, 0xd0fb])), None);
        assert_eq!(poll_period(&code(&[0x6802, 0xf101, 0x0f04, 0xd0fb])), None);
        assert_eq!(poll_period(&code(&[0x6802, 0xf001, 0x0f04, 0xd0fb])), None);
        // Too long, or cut off.
        assert_eq!(
            poll_period(&code(&[
                0x6802, 0x3001, 0x3001, 0x3001, 0x3001, 0x3001, 0x3001, 0xd0f7
            ])),
            Some(8)
        );
        assert_eq!(
            poll_period(&code(&[
                0x6802, 0x3001, 0x3001, 0x3001, 0x3001, 0x3001, 0x3001, 0x3001, 0xd0f6
            ])),
            None
        );
        assert_eq!(poll_period(&code(&[0x6802, 0x429a])), None);
        assert_eq!(poll_period(&[]), None);
    }
    #[test]
    fn poll_period_accepts_thumb2_loads_and_conditional_branches() {
        // ldr.w r2,[r0,#0xc]; tst.w r2,#4; beq .-8
        assert_eq!(
            poll_period(&code(&[0xf8d0, 0x200c, 0xf012, 0x0f04, 0xd0fa])),
            Some(3)
        );
        // ldrh.w r5,[r2,#0x10]; and.w r5,r5,#4; cmp r5,#0; bne.w .-10
        assert_eq!(
            poll_period(&code(&[0xf8b2, 0x5010, 0xf005, 0x0504, 0x2d00, 0xf47f, 0xaff9])),
            Some(4)
        );
        // ldrb.w r1,[r0,#1]; tst.w r1,#0x80; beq.w .-8
        assert_eq!(
            poll_period(&code(&[0xf890, 0x1001, 0xf011, 0x0f80, 0xf43f, 0xaffa])),
            Some(3)
        );
        // Wrong target, forward, unconditional B.W, or reserved condition space.
        assert_eq!(
            poll_period(&code(&[0xf890, 0x1001, 0xf011, 0x0f80, 0xf43f, 0xaff9])),
            None
        );
        assert_eq!(
            poll_period(&code(&[0xf890, 0x1001, 0xf011, 0x0f80, 0xf000, 0x8002])),
            None
        );
        assert_eq!(
            poll_period(&code(&[0xf890, 0x1001, 0xf011, 0x0f80, 0xf7ff, 0xbffa])),
            None
        );
        assert_eq!(
            poll_period(&code(&[0xf890, 0x1001, 0xf380, 0x8811])),
            None
        );
        // Disallowed head-load base/destination registers (SP, LR, PC).
        for (first, second) in [
            (0xf8df, 0x200c), // Rn = PC (literal)
            (0xf8dd, 0x200c), // Rn = SP
            (0xf8de, 0x200c), // Rn = LR
            (0xf8d0, 0xd00c), // Rt = SP
            (0xf8d0, 0xe00c), // Rt = LR
            (0xf8d0, 0xf00c), // Rt = PC
        ] {
            assert_eq!(
                poll_period(&code(&[first, second, 0xf012, 0x0f04, 0xd0fa])),
                None,
                "{first:#06x} {second:#06x}"
            );
        }
        assert_eq!(poll_period(&code(&[0xf8d0])), None);
    }
    #[test]
    fn push_and_non_pc_pop_classifiers_enforce_safe_register_lists() {
        for word in [0xb401, 0xb430, 0xb500, 0xb5f0, 0xe92d_0030, 0xe92d_41f0] {
            assert!(is_terminal_push(word), "{word:#010x}");
        }
        for word in [
            0xb400,      // empty PUSH
            0xbc30,      // POP
            0xbd30,      // POP {..., pc}
            0xe92d_0010, // single-register PUSH.W
            0xe92d_2030, // PUSH.W with SP bit set
            0xe92d_8030, // PUSH.W with PC bit set
            0xe8bd_0030, // POP.W
        ] {
            assert!(!is_terminal_push(word), "{word:#010x}");
        }

        for word in [0xbc01, 0xbc30, 0xbcff, 0xe8bd_0030, 0xe8bd_4030] {
            assert!(is_non_pc_pop(word), "{word:#010x}");
        }
        for word in [
            0xbc00,      // empty POP
            0xbd00,      // POP {pc}
            0xbd30,      // POP {r4, r5, pc}
            0xb430,      // PUSH
            0xe8bd_0010, // single-register POP.W
            0xe8bd_2030, // POP.W with SP bit set
            0xe8bd_8030, // POP.W with PC bit set
            0xe8bd_c030, // POP.W with LR + PC
            0xe92d_0030, // PUSH.W
        ] {
            assert!(!is_non_pc_pop(word), "{word:#010x}");
        }
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
        assert_eq!(
            parts(&[0xf8dd, 0x4004, 0xe8bd, 0x0030, 0x4770]),
            (3, 2, 8, true)
        );
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
