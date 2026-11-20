//! Poll loops of an AArch64 processor.
//!
//! Firmware waits for a device by reading a status register in a loop. When
//! the register is one whose value only device events change
//! (`Device::poll_stable`) and the loop only computes in registers, every
//! round is the round before it until such an event. The board then accounts
//! whole rounds at once and leaves their reads out. `mprofile_blocks::Poll`
//! is the same idea for an M-profile processor in block execution.
//!
//! What makes a round provably identical to the one before it:
//! - two consecutive device accesses of the processor are reads of the same
//!   register at the same PC, a fixed number of instruction entries apart,
//!   with no other callback of that processor in between (so the board took
//!   no action and delivered no interrupt between them);
//! - both reads return the same value, and every general register, SP and
//!   PSTATE are the same at both;
//! - no interrupt pin of the processor is raised;
//! - every instruction the processor can reach from the read within that
//!   number of entries only computes in registers or branches (`round_is_pure`),
//!   so the round has no input but the registers and the value read.
//!
//! All of this is host-only: the entry counts, the instants of device events
//! and the processor state are those of executing every round.

/// The last device read of a processor's run, as a poll loop candidate.
#[derive(Clone, Copy)]
pub(super) struct Poll {
    pub pc: u64,
    pub address: u64,
    pub size: u8,
    /// The register is `poll_stable`.
    pub stable: bool,
    /// The value the read returned and the processor's entry count then.
    pub value: u64,
    pub instructions: u64,
    /// X0-X30, SP and PSTATE at the last read that ended a round.
    pub registers: Option<[u64; 33]>,
    /// The result of `round_is_pure` for a round length and a link register.
    pub proven: Option<(u64, u64, bool)>,
}

/// Host-only state of the fast-forward, per processor where it says so.
pub(super) struct Polls {
    pub last: Vec<Option<Poll>>,
    /// The processor's current run may fast-forward poll loops.
    pub active: Vec<bool>,
    /// The entry count at which the processor's current run must end.
    pub run_end: Vec<u64>,
    /// Reads left out since the access callback last collected them.
    pub skipped_reads: u64,
    /// Counters: fast-forwards and the reads they left out.
    pub skips: Vec<u64>,
    pub reads: Vec<u64>,
    /// Counters: rounds that came back unchanged but are not provably pure,
    /// and the last such (pc, entries, link register).
    pub refused: Vec<u64>,
    pub refused_at: Vec<Option<(u64, u64, u64)>>,
}
impl Polls {
    pub(super) fn new(processors: usize) -> Self {
        Self {
            last: vec![None; processors],
            active: vec![false; processors],
            run_end: vec![0; processors],
            skipped_reads: 0,
            skips: vec![0; processors],
            reads: vec![0; processors],
            refused: vec![0; processors],
            refused_at: vec![None; processors],
        }
    }
}

/// The longest round, in instruction entries, that is examined.
pub(super) const LONGEST_ROUND: u64 = 24;

fn branch_target(address: u64, field: u32, bits: u32) -> u64 {
    let offset = (((field << (32 - bits)) as i32) >> (32 - bits)) as i64 * 4;
    address.wrapping_add(offset as u64)
}

/// Whether a processor that enters the load at `pc` and comes back to it
/// exactly `steps` instruction entries later, with link register `lr` at the
/// load, can only have executed register computation and branches in
/// between. `read` returns the instruction at an address.
///
/// Every path of `steps` instructions from the load is followed, both ways
/// at each conditional branch. Accepted: at the load, LDR, LDRH or LDRB with
/// an unsigned offset; after it, data processing with an immediate or with
/// registers, NOP, B, BL, B.cond, CBZ, CBNZ, TBZ, TBNZ, and RET through X30
/// while the path has not written X30 except by BL. Any other instruction
/// on any path, an unreadable address, or no path that ends at the load
/// refuses the round. A path that reaches the load early is not a round
/// (the load would have been another access) and is dropped.
pub(super) fn round_is_pure(
    read: &mut dyn FnMut(u64) -> Option<u32>,
    pc: u64,
    steps: u64,
    lr: u64,
) -> bool {
    if !(2..=LONGEST_ROUND).contains(&steps) {
        return false;
    }
    let Some(load) = read(pc) else {
        return false;
    };
    if load & 0x3fc0_0000 != 0x3940_0000 {
        return false;
    }
    let link = (load & 31 != 30).then_some(lr);
    let mut paths = vec![(pc.wrapping_add(4), 1, link)];
    let (mut visited, mut returns) = (0, false);
    while let Some((address, step, link)) = paths.pop() {
        if step == steps {
            returns |= address == pc;
            continue;
        }
        if address == pc {
            continue;
        }
        visited += 1;
        if visited > 512 {
            return false;
        }
        let Some(word) = read(address) else {
            return false;
        };
        let next = address.wrapping_add(4);
        let rd = word & 31;
        if word == 0xd503_201f {
            paths.push((next, step + 1, link));
        } else if word & 0xfc00_0000 == 0x1400_0000 {
            paths.push((branch_target(address, word & 0x03ff_ffff, 26), step + 1, link));
        } else if word & 0xfc00_0000 == 0x9400_0000 {
            let target = branch_target(address, word & 0x03ff_ffff, 26);
            paths.push((target, step + 1, Some(next)));
        } else if word & 0xff00_0010 == 0x5400_0000 || word & 0x7e00_0000 == 0x3400_0000 {
            paths.push((next, step + 1, link));
            paths.push((branch_target(address, (word >> 5) & 0x7ffff, 19), step + 1, link));
        } else if word & 0x7e00_0000 == 0x3600_0000 {
            paths.push((next, step + 1, link));
            paths.push((branch_target(address, (word >> 5) & 0x3fff, 14), step + 1, link));
        } else if word & 0xffff_fc1f == 0xd65f_0000 {
            match link {
                Some(target) if (word >> 5) & 31 == 30 => paths.push((target, step + 1, link)),
                _ => return false,
            }
        } else if word & 0x1c00_0000 == 0x1000_0000 || word & 0x0e00_0000 == 0x0a00_0000 {
            paths.push((next, step + 1, if rd == 30 { None } else { link }));
        } else {
            return false;
        }
    }
    returns
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pure(code: &[(u64, u32)], pc: u64, steps: u64, lr: u64) -> bool {
        let mut read = |address: u64| code.iter().find(|(a, _)| *a == address).map(|(_, w)| *w);
        round_is_pure(&mut read, pc, steps, lr)
    }
    const LDR_W0_X0: u32 = 0xb940_0000;
    const RET: u32 = 0xd65f_03c0;
    #[test]
    fn a_round_through_a_read_helper_is_pure() {
        // 0x100: mov x0,x19; bl 0x200; tst w0,#1; b.eq 0x100; mov w0,#0; nop.
        // 0x200: ldr w0,[x0]; ret
        let code = [
            (0x100, 0xaa13_03e0),
            (0x104, 0x9400_003f),
            (0x108, 0x7200_001f),
            (0x10c, 0x54ff_ffa0),
            (0x110, 0x5280_0000),
            (0x114, 0xd503_201f),
            (0x200, LDR_W0_X0),
            (0x204, RET),
        ];
        // ldr, ret, tst, b.eq, mov, bl: six entries, back at the load.
        assert!(pure(&code, 0x200, 6, 0x108));
        // Another length does not end at the load.
        assert!(!pure(&code, 0x200, 5, 0x108));
        assert!(!pure(&code, 0x200, 7, 0x108));
        // The link register points elsewhere: unreadable.
        assert!(!pure(&code, 0x200, 6, 0x300));
        // The way out of the loop is followed for the rest of the round too:
        // a store there (str w0,[x1]) refuses it.
        let mut stores = code;
        stores[5].1 = 0xb900_0020;
        assert!(!pure(&stores, 0x200, 6, 0x108));
        assert!(!pure(&code, 0x200, 1, 0x108));
        assert!(!pure(&code, 0x200, LONGEST_ROUND + 1, 0x108));
    }
    #[test]
    fn anything_but_register_computation_on_any_path_refuses_the_round() {
        let round = |body: u32| {
            // 0x100: ldr w0,[x1]; <body>; cbz w0,0x100
            pure(&[(0x100, 0xb940_0020), (0x104, body), (0x108, 0x34ff_ffc0)], 0x100, 3, 0)
        };
        // and w0,w0,#1; add x2,x2,#1; cmp w0,w3; csel; nop; adr.
        for body in [0x1200_0000u32, 0x9100_0442, 0x6b03_001f, 0x9a82_1020, 0xd503_201f, 0x1000_0002] {
            assert!(round(body), "{body:#x}");
        }
        // str w0,[x1]; ldr w2,[x1]; stp; mrs; msr daifset; svc; br x2; blr x2;
        // ldxr; fmov; dmb; wfi; ret x2; udf.
        for body in [
            0xb900_0020u32,
            0xb940_0022,
            0xa900_0be1,
            0xd53b_4200,
            0xd503_42df,
            0xd400_0001,
            0xd61f_0040,
            0xd63f_0040,
            0xc85f_7c20,
            0x1e27_0000,
            0xd503_3fbf,
            0xd503_207f,
            0xd65f_0040,
            0x0000_0000,
        ] {
            assert!(!round(body), "{body:#x}");
        }
        // The first instruction must be a plain load: ldr w0,[x1,x2]; ldp; str.
        for load in [0xb862_6820u32, 0xa940_0820, 0xb900_0020] {
            assert!(!pure(&[(0x100, load), (0x104, 0x34ff_ffe0)], 0x100, 2, 0), "{load:#x}");
        }
        // The other way of a branch inside the round meets a store.
        // 0x100: ldr w0,[x1]; tbnz w0,#0,0x10c; str w0,[x1]; cbz w0,0x100
        let code = [
            (0x100, 0xb940_0020),
            (0x104, 0x3700_0040),
            (0x108, 0xb900_0020),
            (0x10c, 0x34ff_ffa0),
        ];
        assert!(!pure(&code, 0x100, 3, 0));
        // RET after the path wrote X30 by other means: mov x30,x2; ret.
        let code = [(0x100, 0xb940_0020), (0x104, 0xaa02_03fe), (0x108, RET)];
        assert!(!pure(&code, 0x100, 3, 0x100));
        // The load writes X30 itself.
        assert!(!pure(&[(0x100, 0xb940_003e), (0x104, RET)], 0x100, 2, 0x100));
        assert!(pure(&[(0x100, 0xb940_0020), (0x104, RET)], 0x100, 2, 0x100));
    }
}
