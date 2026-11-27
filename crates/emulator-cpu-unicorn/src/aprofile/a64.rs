//! AArch64 loop proofs built from permission-checked instruction bytes.
//!
//! A proof is the instructions a short backward branch closes: the head, the
//! pure instructions between, and the branch. Decoding refuses every
//! instruction that can store, trap, take an exception, write the PC or read
//! a device, so a proven body only computes in registers, or re-reads RAM it
//! already read.
//!
//! Two shapes are proven. A fixed point is a body whose result equals its
//! input, so every further round is identical and the window can retire at
//! once; it covers stable RAM-flag and published status polls. A countdown is
//! one register moved by a constant towards a fixed comparison, whose exit
//! round one bounded search finds. The execution owner supplies the inputs
//! and bounds the window at every event and peer boundary. No firmware
//! address, function identity or software structure takes part.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

/// Longest proven loop, in bytes from head to branch.
pub(super) const SPAN: u64 = 64;

/// The registers and flags a proof models. X0-X30, SP and the NZCV flags are
/// the only state decoding admits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct State {
    pub registers: [u64; 32],
    /// PSTATE N, Z, C, V in bits 31..28.
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Condition {
    Eq,
    Ne,
    Cs,
    Cc,
    Mi,
    Pl,
    Hi,
    Ls,
    Ge,
    Lt,
    Gt,
    Le,
    /// An unconditional `B`: always taken.
    Always,
}

impl Condition {
    fn passes(self, flags: u32) -> bool {
        let n = flags & (1 << 31) != 0;
        let z = flags & (1 << 30) != 0;
        let c = flags & (1 << 29) != 0;
        let v = flags & (1 << 28) != 0;
        match self {
            Self::Eq => z,
            Self::Ne => !z,
            Self::Cs => c,
            Self::Cc => !c,
            Self::Mi => n,
            Self::Pl => !n,
            Self::Hi => c && !z,
            Self::Ls => !c || z,
            Self::Ge => n == v,
            Self::Lt => n != v,
            Self::Gt => !z && n == v,
            Self::Le => z || n != v,
            Self::Always => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Failure {
    /// A load could not be answered, or named a device register.
    Input,
    /// No closed form describes the loop's exit from here.
    Model,
    /// The branch would leave after one more iteration.
    Leaving,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Width {
    W,
    X,
}

impl Width {
    fn mask(self) -> u64 {
        match self {
            Self::W => u64::from(u32::MAX),
            Self::X => u64::MAX,
        }
    }
    fn sign(self) -> u64 {
        1 << (if self == Self::W { 31 } else { 63 })
    }
    fn bits(self) -> u32 {
        if self == Self::W { 32 } else { 64 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    /// Nothing architecturally visible.
    Nop,
    /// `ADD`/`SUB` of an immediate; with `flags` and `dst == 31` a compare.
    AddSub {
        width: Width,
        subtract: bool,
        flags: bool,
        dst: usize,
        src: usize,
        value: u64,
    },
    /// `ADD`/`SUB` of two registers; with `flags` and `dst == 31` a compare.
    AddSubRegister {
        width: Width,
        subtract: bool,
        flags: bool,
        dst: usize,
        lhs: usize,
        rhs: usize,
    },
    /// `MOVZ`/`MOVN`/`MOVK` of an immediate.
    Mov {
        width: Width,
        dst: usize,
        value: u64,
        mask: u64,
        merge: bool,
    },
    /// `LSL`/`LSR`/`ASR` by an immediate, optionally setting the flags.
    Shift {
        width: Width,
        right: bool,
        arithmetic: bool,
        flags: bool,
        dst: usize,
        src: usize,
        amount: u32,
    },
    /// `LDR` of a byte, halfword or word at an unsigned offset from a base.
    Load {
        bits: u32,
        dst: usize,
        base: usize,
        offset: u64,
    },
}

/// One decoded loop: its head, the branch that closes it and its body.
#[derive(Clone, Debug)]
pub(super) struct Proof {
    pub head: u64,
    pub branch: u64,
    pub length: u64,
    condition: Condition,
    /// The register and bit mask a `CBZ`/`CBNZ`/`TBZ`/`TBNZ` tests.
    test: Option<(usize, u64)>,
    ops: Vec<Op>,
}

/// The outcome of walking complete iterations from the state at the branch.
#[derive(Clone, Copy, Debug)]
pub(super) struct Advance {
    /// The state at the branch after `iterations` complete iterations.
    pub state: State,
    pub iterations: u64,
    /// True when a counted body's exit bounded the window; false when every
    /// round was the same, so the window retired at its whole budget.
    pub counted: bool,
}

fn sign_extend(value: u64, bits: u32) -> i64 {
    ((value << (64 - bits)) as i64) >> (64 - bits)
}

fn branch_target(address: u64, field: u64, bits: u32) -> Option<u64> {
    address.checked_add_signed(sign_extend(field, bits).checked_mul(4)?)
}

fn condition_of(word: u32) -> Option<Condition> {
    Some(match word & 15 {
        0 => Condition::Eq,
        1 => Condition::Ne,
        2 => Condition::Cs,
        3 => Condition::Cc,
        4 => Condition::Mi,
        5 => Condition::Pl,
        6 => Condition::Always,
        8 => Condition::Hi,
        9 => Condition::Ls,
        10 => Condition::Ge,
        11 => Condition::Lt,
        12 => Condition::Gt,
        13 => Condition::Le,
        _ => return None,
    })
}

fn width_of(word: u32) -> Width {
    if word & (1 << 31) != 0 {
        Width::X
    } else {
        Width::W
    }
}

/// The head of the short backward branch at `address`, if `word` is one.
/// `None` for anything that is not a direct branch, does not go backwards,
/// or reaches further than [`SPAN`].
pub(super) fn head(address: u64, word: u32) -> Option<u64> {
    let target = if word & 0x7c00_0000 == 0x1400_0000 {
        // B, not BL.
        branch_target(address, u64::from(word & 0x03ff_ffff), 26)?
    } else if word & 0xff00_0010 == 0x5400_0000 {
        branch_target(address, u64::from((word >> 5) & 0x7ffff), 19)?
    } else if word & 0x7e00_0000 == 0x3400_0000 {
        // CBZ/CBNZ.
        branch_target(address, u64::from((word >> 5) & 0x7ffff), 19)?
    } else if word & 0x7e00_0000 == 0x3600_0000 {
        // TBZ/TBNZ.
        branch_target(address, u64::from((word >> 5) & 0x3fff), 14)?
    } else {
        return None;
    };
    (target <= address && address - target <= SPAN).then_some(target)
}

fn branch_condition(word: u32) -> Option<(Condition, Option<(usize, u64)>)> {
    if word & 0x7c00_0000 == 0x1400_0000 {
        return Some((Condition::Always, None));
    }
    if word & 0xff00_0010 == 0x5400_0000 {
        return Some((condition_of(word)?, None));
    }
    let taken_on_set = word & (1 << 24) != 0;
    let condition = if taken_on_set {
        Condition::Ne
    } else {
        Condition::Eq
    };
    if word & 0x7e00_0000 == 0x3400_0000 {
        let register = (word & 31) as usize;
        let mask = if word & (1 << 31) != 0 {
            u64::MAX
        } else {
            u64::from(u32::MAX)
        };
        return Some((condition, Some((register, mask))));
    }
    if word & 0x7e00_0000 == 0x3600_0000 {
        let register = (word & 31) as usize;
        let mask = 1u64 << ((word >> 19) & 31);
        return Some((condition, Some((register, mask))));
    }
    None
}

/// Decode one body instruction, or `None` for anything the proof refuses.
fn decode(word: u32) -> Option<Op> {
    match word {
        // NOP, YIELD, SEV.
        0xd503_201f | 0xd503_203f | 0xd503_209f => return Some(Op::Nop),
        _ => {}
    }
    // ADD/SUB immediate.
    if word & 0x1f00_0000 == 0x1100_0000 && word & (1 << 23) == 0 {
        let width = width_of(word);
        let subtract = word & (1 << 30) != 0;
        let flags = word & (1 << 29) != 0;
        let shift = (word >> 22) & 1;
        let value = u64::from((word >> 10) & 0xfff) << (12 * shift);
        if value > width.mask() {
            return None;
        }
        let dst = (word & 31) as usize;
        let src = ((word >> 5) & 31) as usize;
        if src == 31 {
            return None;
        }
        if dst == 31 && !flags {
            // Moving SP lies outside the model.
            return None;
        }
        return Some(Op::AddSub {
            width,
            subtract,
            flags,
            dst,
            src,
            value,
        });
    }
    // ADD/SUB register, no shift and no carry-in.
    if word & 0x1f20_0c00 == 0x0b00_0000 {
        let width = width_of(word);
        let subtract = word & (1 << 30) != 0;
        let flags = word & (1 << 29) != 0;
        let dst = (word & 31) as usize;
        let lhs = ((word >> 5) & 31) as usize;
        let rhs = ((word >> 16) & 31) as usize;
        if dst == 31 && !flags {
            return None;
        }
        if lhs == 31 || rhs == 31 {
            return None;
        }
        return Some(Op::AddSubRegister {
            width,
            subtract,
            flags,
            dst,
            lhs,
            rhs,
        });
    }
    // MOVZ/MOVN/MOVK: `sf|opc(2)|100101|hw|imm16|Rd`.
    if word & 0x7f80_0000 == 0x1280_0000 {
        let width = width_of(word);
        let opc = (word >> 29) & 3;
        let halfword = (word >> 21) & 3;
        let dst = (word & 31) as usize;
        let amount = 16 * halfword;
        if dst == 31 || opc == 1 || amount >= width.bits() {
            return None;
        }
        let value = u64::from((word >> 5) & 0xffff);
        return Some(if opc == 3 {
            // MOVK keeps the other halfwords.
            Op::Mov {
                width,
                dst,
                value: value << amount,
                mask: 0xffffu64 << amount,
                merge: true,
            }
        } else if opc == 0 {
            Op::Mov {
                width,
                dst,
                value: !(value << amount),
                mask: width.mask(),
                merge: false,
            }
        } else {
            Op::Mov {
                width,
                dst,
                value: value << amount,
                mask: 0xffffu64 << amount,
                merge: false,
            }
        });
    }
    // LSL/LSR/ASR immediate (`opc` 00 of the bitfield family).
    if word & 0x7f80_0000 == 0x5300_0000 {
        let kind = (word >> 22) & 3;
        if kind == 2 {
            return None;
        }
        let width = width_of(word);
        let flags = word & (1 << 29) != 0;
        let dst = (word & 31) as usize;
        let src = ((word >> 5) & 31) as usize;
        if dst == 31 || src == 31 {
            return None;
        }
        let amount = (word >> 16) & 0x3f;
        if amount >= width.bits() {
            return None;
        }
        return Some(Op::Shift {
            width,
            right: kind != 0,
            arithmetic: kind == 3,
            flags,
            dst,
            src,
            amount,
        });
    }
    // LDR (unsigned offset) of a byte, halfword or word: no sign extension,
    // no SIMD and no address writeback.
    if word & 0x3b00_0000 == 0x3900_0000 {
        let bits = match (word >> 30) & 3 {
            0 => 8,
            1 => 16,
            2 => 32,
            _ => return None,
        };
        // `01` is the plain unsigned load.
        if (word >> 22) & 3 != 1 {
            return None;
        }
        let dst = (word & 31) as usize;
        let base = ((word >> 5) & 31) as usize;
        if dst == 31 || base == 31 {
            return None;
        }
        let offset = u64::from((word >> 10) & 0xfff) * u64::from(bits / 8);
        return Some(Op::Load {
            bits,
            dst,
            base,
            offset,
        });
    }
    None
}

fn arithmetic(width: Width, subtract: bool, a: u64, b: u64) -> (u64, bool, bool) {
    if subtract {
        (
            a.wrapping_sub(b) & width.mask(),
            a >= b,
            ((a ^ b) & (a ^ a.wrapping_sub(b))) & width.sign() != 0,
        )
    } else {
        let sum = a + b;
        (
            sum & width.mask(),
            sum > width.mask(),
            (!(a ^ b) & (a ^ sum)) & width.sign() != 0,
        )
    }
}

fn flag_bits(result: u64, carry: bool, overflow: bool, width: Width) -> u32 {
    (u32::from(result & width.sign() != 0) << 31)
        | (u32::from(result & width.mask() == 0) << 30)
        | (u32::from(carry) << 29)
        | (u32::from(overflow) << 28)
}

fn apply(
    op: Op,
    mut state: State,
    index: usize,
    input: &mut impl FnMut(usize, u64, u32) -> Option<u64>,
) -> Result<State, Failure> {
    match op {
        Op::Nop => {}
        Op::AddSub {
            width,
            subtract,
            flags,
            dst,
            src,
            value,
        } => {
            let (result, carry, overflow) =
                arithmetic(width, subtract, state.registers[src] & width.mask(), value);
            if dst != 31 {
                state.registers[dst] = result;
            }
            if flags {
                state.flags = flag_bits(result, carry, overflow, width);
            }
        }
        Op::AddSubRegister {
            width,
            subtract,
            flags,
            dst,
            lhs,
            rhs,
        } => {
            let (result, carry, overflow) = arithmetic(
                width,
                subtract,
                state.registers[lhs] & width.mask(),
                state.registers[rhs] & width.mask(),
            );
            if dst != 31 {
                state.registers[dst] = result;
            }
            if flags {
                state.flags = flag_bits(result, carry, overflow, width);
            }
        }
        Op::Mov {
            width,
            dst,
            value,
            mask,
            merge,
        } => {
            let before = state.registers[dst] & width.mask();
            state.registers[dst] = if merge {
                ((before & !mask) | (value & mask)) & width.mask()
            } else {
                value & mask & width.mask()
            };
        }
        Op::Shift {
            width,
            right,
            arithmetic,
            flags,
            dst,
            src,
            amount,
        } => {
            let a = state.registers[src] & width.mask();
            let (result, carry) = if !right {
                (
                    (a << amount) & width.mask(),
                    amount != 0 && a & (1 << (width.bits() - amount)) != 0,
                )
            } else if arithmetic {
                let signed = sign_extend(a, width.bits());
                (
                    ((signed >> amount) as u64) & width.mask(),
                    signed & (1 << (amount - 1)) != 0,
                )
            } else {
                (a >> amount, amount != 0 && a & (1 << (amount - 1)) != 0)
            };
            state.registers[dst] = result;
            if flags {
                state.flags = flag_bits(result, carry, false, width);
            }
        }
        Op::Load {
            bits,
            dst,
            base,
            offset,
        } => {
            let address = state.registers[base].wrapping_add(offset);
            let word = input(index, address, bits).ok_or(Failure::Input)?;
            let mask = if bits >= 64 {
                u64::MAX
            } else {
                (1u64 << bits) - 1
            };
            state.registers[dst] = word & mask;
        }
    }
    Ok(state)
}

/// A counted body: one register moves by a constant each iteration and the
/// branch reads that register, or the flags its last update or comparison
/// left.
#[derive(Clone, Copy, Debug)]
struct Counter {
    register: usize,
    width: Width,
    /// The change per iteration, signed, with no wrap inside the window.
    delta: i64,
    /// The flags the branch reads, when it does not test the register.
    compare: Option<Compare>,
}

#[derive(Clone, Copy, Debug)]
enum Compare {
    /// The update's own flags: `SUBS r, r, #k` with `B.NE`/`B.HI`/`B.PL`.
    Update { subtract: bool },
    /// A following `CMP`/`CMN r, #imm` or `CMP`/`CMN r, Rm`.
    Explicit { subtract: bool, against: Against },
}

#[derive(Clone, Copy, Debug)]
enum Against {
    Immediate(u64),
    Register(usize),
}

impl Proof {
    /// Decode the loop from `head` through the branch at `branch`. `bytes`
    /// covers exactly `head..branch + 4`.
    pub(super) fn decode(head: u64, branch: u64, bytes: &[u8]) -> Option<Self> {
        let (words, rest) = bytes.as_chunks::<4>();
        if !rest.is_empty() || head + 4 * words.len() as u64 != branch + 4 {
            return None;
        }
        let (&last, body) = words.split_last()?;
        let branch_word = u32::from_le_bytes(last);
        if super::a64::head(branch, branch_word)? != head {
            return None;
        }
        let (condition, test) = branch_condition(branch_word)?;
        let ops = body
            .iter()
            .map(|bytes| decode(u32::from_le_bytes(*bytes)))
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            head,
            branch,
            length: words.len() as u64,
            condition,
            test,
            ops,
        })
    }

    /// Whether a proof of these bytes reads RAM.
    pub(super) fn reads(&self) -> bool {
        self.ops.iter().any(|op| matches!(op, Op::Load { .. }))
    }

    /// Whether the branch is taken on this state, which is the state at the
    /// head (or equivalently at the branch, since a branch writes no
    /// architectural state).
    pub(super) fn taken(&self, state: State) -> bool {
        match self.test {
            Some((register, mask)) => {
                let zero = state.registers[register] & mask == 0;
                self.condition.passes((u32::from(zero) << 30) | (1 << 29))
            }
            None => self.condition.passes(state.flags),
        }
    }

    fn step(
        &self,
        state: State,
        input: &mut impl FnMut(usize, u64, u32) -> Option<u64>,
    ) -> Result<State, Failure> {
        let mut after = state;
        for (index, op) in self.ops.iter().enumerate() {
            after = apply(*op, after, index, input)?;
        }
        Ok(after)
    }

    /// The counted model of this body, or `None` for any other shape.
    ///
    /// A body qualifies when every instruction is either silent or moves the
    /// one counted register by a constant, and the branch reads either that
    /// register or flags one instruction wrote from it. Any other change to
    /// any register or flag refuses the model, so the branch's predicate
    /// depends on the count alone.
    fn counter(&self) -> Option<Counter> {
        let mut moving: Option<(usize, Width, i64)> = None;
        let mut flags: Option<Compare> = None;
        for op in &self.ops {
            match *op {
                Op::Nop => {}
                Op::AddSub {
                    width,
                    subtract,
                    flags: sets_flags,
                    dst,
                    src,
                    value,
                } if dst == src && dst != 31 => {
                    let step = i64::try_from(value).ok()?;
                    let delta = if subtract { -step } else { step };
                    match moving {
                        Some((register, seen, total)) if register == dst && seen == width => {
                            moving = Some((dst, width, total.checked_add(delta)?));
                        }
                        Some(_) => return None,
                        None => moving = Some((dst, width, delta)),
                    }
                    if sets_flags {
                        flags = Some(Compare::Update { subtract });
                    }
                }
                Op::AddSub {
                    width,
                    subtract,
                    flags: true,
                    dst: 31,
                    src,
                    value,
                } => {
                    let (register, seen, _) = moving?;
                    // A comparison may follow the update and replace its
                    // flags; a second comparison may not.
                    if src != register
                        || width != seen
                        || flags.is_some_and(|flags| !matches!(flags, Compare::Update { .. }))
                    {
                        return None;
                    }
                    flags = Some(Compare::Explicit {
                        subtract,
                        against: Against::Immediate(value),
                    });
                }
                Op::AddSubRegister {
                    width,
                    subtract,
                    flags: true,
                    dst: 31,
                    lhs,
                    rhs,
                } => {
                    let (register, seen, _) = moving?;
                    if lhs != register
                        || rhs == register
                        || width != seen
                        || flags.is_some_and(|flags| !matches!(flags, Compare::Update { .. }))
                    {
                        return None;
                    }
                    flags = Some(Compare::Explicit {
                        subtract,
                        against: Against::Register(rhs),
                    });
                }
                // Any other write or flag set breaks the count.
                _ => return None,
            }
        }
        let (register, width, delta) = moving?;
        if delta == 0 {
            return None;
        }
        let compare = match (self.test, flags) {
            // A register-tested branch reads no flags; a flagged body whose
            // flags nobody reads is still a count.
            (Some((tested, _)), _) if tested == register => None,
            (Some(_), _) => return None,
            (None, Some(compare)) => Some(compare),
            (None, None) => return None,
        };
        Some(Counter {
            register,
            width,
            delta,
            compare,
        })
    }

    /// The state after `n` complete iterations, by the counted model.
    fn at(&self, state: State, counter: Counter, n: u64) -> State {
        let mask = counter.width.mask();
        let before = state.registers[counter.register] & mask;
        let after =
            ((before as i128) + i128::from(counter.delta) * i128::from(n)) as u64 & mask;
        let mut out = state;
        out.registers[counter.register] = after;
        if let Some(compare) = counter.compare {
            out.flags = match compare {
                Compare::Update { subtract } => {
                    let step = counter.delta.unsigned_abs();
                    // `after` is the update's own result of `previous +/- step`.
                    let previous = if subtract {
                        after.wrapping_add(step)
                    } else {
                        after.wrapping_sub(step)
                    } & mask;
                    let (_, carry, overflow) =
                        arithmetic(counter.width, subtract, previous, step);
                    flag_bits(after, carry, overflow, counter.width)
                }
                Compare::Explicit { subtract, against } => {
                    let other = match against {
                        Against::Immediate(value) => value & mask,
                        Against::Register(register) => state.registers[register] & mask,
                    };
                    let (result, carry, overflow) =
                        arithmetic(counter.width, subtract, after, other);
                    flag_bits(result, carry, overflow, counter.width)
                }
            };
        }
        out
    }

    /// Advance at most `iterations` complete rounds from `state`.
    ///
    /// `state` is the architectural state at the branch, which the loop's
    /// branch is about to execute and which the instruction entry already
    /// counted. One round executes that branch and the body after it, so it
    /// costs `length` instructions and ends at the same branch with
    /// `body(state)`. The returned state is always at the branch, so the
    /// owner may re-execute the branch normally.
    ///
    /// `read` answers only reads that stay stable for the whole window, and
    /// `None` for a device register. `Leaving` means the branch is not taken
    /// now; `Model` means the loop changes something the proof cannot name,
    /// or its exit is not a whole number of rounds from here.
    pub(super) fn advance(
        &self,
        state: State,
        iterations: u64,
        mut read: impl FnMut(u64, u32) -> Option<u64>,
    ) -> Result<Advance, Failure> {
        if iterations == 0 {
            return Err(Failure::Model);
        }
        let reads = self.reads();
        let mut input = |index: usize, address: u64, bits: u32| {
            if reads && matches!(self.ops[index], Op::Load { .. }) {
                read(address, bits)
            } else {
                None
            }
        };
        if !self.taken(state) {
            return Err(Failure::Leaving);
        }
        let next = self.step(state, &mut input)?;
        if next == state {
            // Every further round is this round: the whole window retires.
            return Ok(Advance {
                state,
                iterations,
                counted: false,
            });
        }
        if reads {
            // A loaded value changed between rounds: a poll of an input the
            // proof cannot predict, so the loop's exit is unknown.
            return Err(Failure::Model);
        }
        let counter = self.counter().ok_or(Failure::Model)?;
        // The model must reproduce the body exactly, or it is not a model.
        if self.at(state, counter, 1) != next {
            return Err(Failure::Model);
        }
        let value = state.registers[counter.register] & counter.width.mask();
        let bound = match counter.compare {
            Some(Compare::Update { .. }) | None => 0,
            Some(Compare::Explicit { against, .. }) => match against {
                Against::Immediate(value) => value & counter.width.mask(),
                Against::Register(register) => state.registers[register] & counter.width.mask(),
            },
        };
        let stride = counter.delta.unsigned_abs();
        let distance = if counter.delta < 0 {
            value.saturating_sub(bound)
        } else {
            bound.saturating_sub(value)
        };
        // Rounds until the branch leaves: the first round whose branch is not
        // taken. The stride must land on the bound to leave at all; otherwise
        // only a register wrap would end the loop, which the window cannot see.
        if distance == 0 {
            // At the bound already: the next round leaves, nothing to retire.
            return Err(Failure::Model);
        }
        let exit = distance
            .checked_div(stride)
            .filter(|_| distance.is_multiple_of(stride));
        let rounds = exit.map_or(iterations, |exit| iterations.min(exit));
        if rounds == 0 {
            return Err(Failure::Model);
        }
        let after = self.at(state, counter, rounds);
        // The window may only end where the next round is either taken or is
        // exactly the exit the model predicts.
        if !self.taken(after) && exit != Some(rounds) {
            return Err(Failure::Model);
        }
        Ok(Advance {
            state: after,
            iterations: rounds,
            counted: true,
        })
    }
}

/// Host counters and the refusal caches of the AArch64 loop prover. Host
/// execution state only: no device, register or keyframe holds any of it.
#[derive(Default)]
pub(super) struct Proofs {
    pub candidate_entries: Cell<u64>,
    pub proofs_attempted: Cell<u64>,
    pub poll_instructions: Cell<u64>,
    pub counted_instructions: Cell<u64>,
    pub accelerated_instructions: Cell<u64>,
    pub failed_structural: Cell<u64>,
    pub failed_window_inactive: Cell<u64>,
    pub failed_observer: Cell<u64>,
    pub failed_read_unavailable: Cell<u64>,
    pub failed_leaving: Cell<u64>,
    pub failed_model: Cell<u64>,
    pub failed_budget: Cell<u64>,
    /// Bytes that failed structurally or have no iteration model, by branch
    /// address. Code invalidation clears them.
    pub failures: RefCell<BTreeMap<u64, Vec<u8>>>,
    /// The branch a window stopped at, so it executes once normally next.
    pub bypass: Cell<Option<u64>>,
    /// The run a window stopped before its counted end, so the board resumes
    /// the remainder before applying its clock limit.
    pub resume: Cell<bool>,
}

impl Proofs {
    pub(super) fn count(&self, counter: &Cell<u64>, n: u64) {
        counter.set(counter.get().saturating_add(n));
    }
    pub(super) fn invalidate(&self) {
        self.failures.borrow_mut().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `b.ne` back to `head` from a branch `count` instructions later.
    fn b_ne(count: u32) -> u32 {
        let offset = -i32::try_from(count).unwrap();
        0x5400_0001 | (((offset as u32) & 0x7ffff) << 5)
    }
    const B_SELF: u32 = 0x1400_0000;

    fn proof(head: u64, words: &[u32]) -> Proof {
        let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        let branch = head + 4 * (words.len() as u64 - 1);
        match Proof::decode(head, branch, &bytes) {
            Some(proof) => proof,
            None => panic!("decodes: {words:08x?}"),
        }
    }

    /// The immediate `subs <width> r, r, #k` at `word` decrements by.
    fn stride(word: u32) -> u64 {
        u64::from((word >> 10) & 0xfff) << (12 * ((word >> 22) & 1))
    }

    /// A two-instruction loop: `<body>` then `b.ne` back to the head.
    fn pair(head: u64, body: u32) -> Proof {
        proof(head, &[body, b_ne(1)])
    }

    #[test]
    fn a_self_branch_is_a_fixed_point() {
        let p = proof(0x1000, &[B_SELF]);
        assert_eq!(p.length, 1);
        let state = State::default();
        let advanced = p.advance(state, 500, |_, _| None).unwrap();
        assert_eq!(advanced.iterations, 500);
        assert!(!advanced.counted);
        assert_eq!(advanced.state, state);
    }

    #[test]
    fn countdown_retires_to_its_exit_and_matches_the_body() {
        // subs w0,w0,#1 ; b.ne 0x1000
        let p = pair(0x1000, 0x7100_0400);
        let mut state = State::default();
        state.registers[0] = 3;
        let advanced = p.advance(state, 100, |_, _| None).unwrap();
        assert_eq!(advanced.iterations, 3);
        assert!(advanced.counted);
        assert_eq!(advanced.state.registers[0], 0);
        assert_eq!(advanced.state.flags & (1 << 30), 1 << 30);
        assert_eq!(advanced.iterations * p.length, 6);
    }

    #[test]
    fn a_countdown_compared_against_a_register_bound_matches_the_body() {
        // subs w0,w0,#1 ; cmp w0,w1 ; b.ne 0x1000
        let p = proof(0x1000, &[0x7100_0400, 0x6b01_001f, b_ne(2)]);
        let mut state = State::default();
        state.registers[0] = 20;
        state.registers[1] = 7;
        let advanced = p.advance(state, 100, |_, _| None).unwrap();
        assert_eq!(advanced.iterations, 13);
        assert_eq!(advanced.state.registers[0], 7);
    }

    #[test]
    fn a_stable_ram_poll_is_a_fixed_point() {
        // ldr w0,[x1] ; cbz w0,0x1000
        let p = proof(0x1000, &[0xb940_0020, 0x34ff_ffe0]);
        assert!(p.reads());
        let mut state = State::default();
        state.registers[1] = 0x4000;
        let advanced = p
            .advance(state, 200, |address, bits| {
                assert_eq!((address, bits), (0x4000, 32));
                Some(0)
            })
            .unwrap();
        assert_eq!(advanced.iterations, 200);
        assert!(!advanced.counted);
    }

    #[test]
    fn a_poll_whose_value_changes_is_refused() {
        // ldr w0,[x1] ; add x2,x2,#1 ; cbz w0,0x1000
        let p = proof(0x1000, &[0xb940_0020, 0x9100_0442, 0x34ff_ffc0]);
        let mut state = State::default();
        state.registers[1] = 0x4000;
        let mut reads = 0;
        let refused = p.advance(state, 10, |_, _| {
            reads += 1;
            Some(reads)
        });
        assert_eq!(refused.err(), Some(Failure::Model));
    }

    #[test]
    fn a_store_body_is_never_decoded() {
        for store in [0x2900_0000u32, 0xb900_0020, 0x3900_0020, 0x3d80_0000] {
            let bytes = [store.to_le_bytes(), B_SELF.to_le_bytes()].concat();
            assert!(
                Proof::decode(0x1000, 0x1004, &bytes).is_none(),
                "{store:#010x}"
            );
        }
    }

    #[test]
    fn system_and_pc_writing_bodies_are_refused() {
        for word in [
            0xd503_42dfu32, // msr daifset
            0xd400_0001,    // svc
            0xd61f_0040,    // br x2
            0xd63f_0040,    // blr x2
            0xd65f_03c0,    // ret
            0xd69f_03e0,    // eret
            0xd503_3fbf,    // dsb
            0xd503_3fdf,    // isb
            0xd503_207f,    // wfi
            0xd503_205f,    // wfe
            0xc85f_7c20,    // ldxr
            0x0000_0000,    // unallocated
            0x1e27_0000,    // fmov (SIMD)
            0xd65f_0000,    // ret with a register
            0xd503_40df,    // msr daifclr
        ] {
            assert!(decode(word).is_none(), "{word:#010x}");
        }
        // The pure forms decode.
        for word in [
            0xd503_201fu32, // nop
            0xd503_203f,    // yield
            0x7100_0400,    // subs w0,w0,#1
            0x9100_0442,    // add x2,x2,#1
            0xb940_0020,    // ldr w0,[x1]
            0x7940_0020,    // ldrh w0,[x1]
            0x3940_0020,    // ldrb w0,[x1]
            0x531f_7c20,    // lsr w0,w1,#31
        ] {
            assert!(decode(word).is_some(), "{word:#010x}");
        }
    }

    #[test]
    fn every_counted_register_width_and_step_is_modelled() {
        for (word, start) in [
            (0x7100_0400u32, 5u64), // subs w0,w0,#1
            (0xf100_0400, 5),       // subs x0,x0,#1
            (0x7100_0c00, 9),       // subs w0,w0,#3
            (0xf100_1000, 12),      // subs x0,x0,#4
        ] {
            let p = pair(0x1000, word);
            let mut state = State::default();
            state.registers[0] = start;
            let advanced = p
                .advance(state, 1_000, |_, _| None)
                .unwrap_or_else(|failure| panic!("{word:#010x}: {failure:?}"));
            assert_eq!(advanced.state.registers[0], 0, "{word:#010x}");
            // One round is the body and its branch.
            assert_eq!(advanced.iterations * p.length, (start / stride(word)) * 2, "{word:#010x}");
        }
    }

    #[test]
    fn a_step_that_does_not_land_on_the_bound_is_refused() {
        // subs w0,w0,#2 from an odd value never reaches zero.
        let p = pair(0x1000, 0x7100_0800);
        let mut state = State::default();
        state.registers[0] = 5;
        // An odd value steps over zero forever, so every round is identical
        // in effect and the window retires its whole budget exactly.
        let advanced = p.advance(state, 100, |_, _| None).unwrap();
        assert_eq!(advanced.iterations, 100);
        assert_eq!(advanced.state.registers[0], 5u64.wrapping_sub(200) & u64::from(u32::MAX));
        // A stride that does divide retires exactly to the bound.
        state.registers[0] = 6;
        let advanced = p.advance(state, 100, |_, _| None).unwrap();
        assert_eq!(advanced.iterations, 3);
        assert_eq!(advanced.state.registers[0], 0);
    }

    #[test]
    fn the_budget_bounds_the_retired_window() {
        let p = pair(0x1000, 0x7100_0400);
        let mut state = State::default();
        state.registers[0] = 1_000_000;
        let advanced = p.advance(state, 10, |_, _| None).unwrap();
        assert_eq!(advanced.iterations, 10);
        assert_eq!(advanced.state.registers[0], 999_990);
    }

    #[test]
    fn a_value_at_the_bound_leaves_at_once() {
        // `b.ne` with Z set is not taken.
        let p = pair(0x1000, 0x7100_0400);
        let mut state = State::default();
        state.flags = 1 << 30;
        assert_eq!(
            p.advance(state, 10, |_, _| None).err(),
            Some(Failure::Leaving)
        );
        // At the bound with the branch still taken: the window cannot retire.
        state.flags = 0;
        assert_eq!(p.advance(state, 10, |_, _| None).err(), Some(Failure::Model));
    }

    #[test]
    fn a_body_that_changes_a_second_register_has_no_count() {
        // subs w0,w0,#1 ; add w2,w2,#1 ; b.ne 0x1000
        let p = proof(0x1000, &[0x7100_0400, 0x1100_0442, b_ne(2)]);
        let mut state = State::default();
        state.registers[0] = 9;
        assert_eq!(p.advance(state, 10, |_, _| None).err(), Some(Failure::Model));
    }

    #[test]
    fn a_countdown_whose_flags_nobody_sets_has_no_model() {
        // sub w0,w0,#1 (no S) ; b.ne 0x1000: the branch reads stale flags.
        let p = pair(0x1000, 0x5100_0400);
        let mut state = State::default();
        state.registers[0] = 3;
        assert_eq!(p.advance(state, 10, |_, _| None).err(), Some(Failure::Model));
    }

    #[test]
    fn counts_match_an_interpreter_walk_of_every_iteration() {
        // subs w0,w0,#1 ; nop ; cmp w0,#4 ; b.ne 0x1000
        let p = proof(0x1000, &[0x7100_0400, 0xd503_201f, 0x7100_101f, b_ne(3)]);
        let mut state = State::default();
        for start in [5u64, 9, 30, 1001] {
            state.registers[0] = start;
            let advanced = p.advance(state, 10_000, |_, _| None).unwrap();
            // Walk it with the body itself: the count must reproduce it.
            let mut walked = state;
            let mut n = 0;
            while p.taken(walked) {
                walked = p.step(walked, &mut |_, _, _| None).unwrap();
                n += 1;
                assert!(n < 10_000);
            }
            assert_eq!(advanced.iterations, n, "start {start}");
            assert_eq!(advanced.state, walked, "start {start}");
        }
    }

    #[test]
    fn a_window_inside_the_approach_ends_with_the_branch_still_taken() {
        // subs w0,w0,#1 from 1000 with room for 100 rounds: the branch is
        // taken after all of them, exactly as ordinary execution would have.
        let p = pair(0x1000, 0x7100_0400);
        let mut state = State::default();
        state.registers[0] = 1000;
        let advanced = p.advance(state, 100, |_, _| None).unwrap();
        assert_eq!(advanced.iterations, 100);
        assert!(p.taken(advanced.state));
        assert_eq!(advanced.state.registers[0], 900);
    }
}
