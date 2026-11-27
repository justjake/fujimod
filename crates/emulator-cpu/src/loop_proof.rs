//! Conservative instruction-set loop proofs, shared by execution backends.
//!
//! A proof is decoded from permission-checked instruction bytes that end in a
//! short backward branch. It models only register, flag and load effects; any
//! store, system, PC-writing or unlisted instruction fails the decode. The
//! execution owner supplies the inputs (RAM and device-declared stable reads)
//! and bounds the window at every event or peer boundary. No firmware
//! addresses, task names or software structures take part.
use crate::arm;

mod a32;
mod register;
mod t32;

pub use register::RegisterLoop;

/// The instruction set a loop branch was fetched in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Isa {
    A32,
    T32,
}

/// Longest proven loop, in bytes from its head to its branch.
const SPAN: u64 = 60;

#[derive(Clone, Copy, Debug)]
enum Operand {
    Immediate(u32, Option<bool>),
    Register(usize, u32, u32),
    RegisterShift(usize, u32, usize),
}
#[derive(Clone, Copy, Debug)]
enum Op {
    Data {
        opcode: u32,
        flags: bool,
        dst: usize,
        lhs: usize,
        rhs: Operand,
    },
    Load {
        dst: usize,
        base: usize,
        offset: u32,
        /// Register index shifted left by the amount, added to the offset.
        index: Option<(usize, u32)>,
        width: usize,
    },
    /// An IT-block condition: the next operation executes only when it passes
    /// on the flags current at that instruction.
    Predicate(u32),
    NoEffect,
    /// CBZ (`nonzero` false) or CBNZ leaving the loop.
    Exit {
        register: usize,
        nonzero: bool,
    },
}

/// Architectural state at the loop branch: R0-R14 and the CPSR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    pub registers: [u32; 15],
    pub cpsr: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdvanceFailure {
    BudgetRoom,
    ConditionNotTaken,
    ReadUnavailable,
    /// A loaded value differs from the previous iteration's; not cached.
    InputsChanged,
    IterationModelUnsupported,
    CounterAtBoundary,
}

/// A proven window: the state at the branch after `iterations` complete
/// iterations, and whether a counted model (rather than a fixed point)
/// produced it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Advance {
    pub state: State,
    pub iterations: u64,
    pub counted: bool,
}

#[derive(Clone, Debug)]
pub struct Proof {
    branch: u64,
    condition: u32,
    instructions: u64,
    body: Vec<Op>,
}

/// The loop head of a direct, short, backward branch at `address`, or `None`.
/// `word` holds the instruction bytes at `address` in little-endian order.
pub fn loop_start(isa: Isa, address: u64, word: u32) -> Option<u64> {
    match isa {
        Isa::A32 => a32::target(address, word),
        Isa::T32 => t32::target(address, word).map(|(start, _)| start),
    }
}

/// The size in bytes of the loop branch at `address`.
pub fn branch_size(isa: Isa, address: u64, word: u32) -> Option<u64> {
    match isa {
        Isa::A32 => a32::target(address, word).map(|_| 4),
        Isa::T32 => t32::target(address, word).map(|(_, size)| size),
    }
}

impl Proof {
    /// Decode the loop from `start` through the branch at `branch`. `bytes`
    /// covers exactly `start..branch + branch size`.
    pub fn decode(isa: Isa, start: u64, branch: u64, bytes: &[u8]) -> Option<Self> {
        match isa {
            Isa::A32 => a32::proof(start, branch, bytes),
            Isa::T32 => t32::proof(start, branch, bytes),
        }
    }
    pub fn branch(&self) -> u64 {
        self.branch
    }
    /// Instructions executed per iteration, including the branch.
    pub fn length(&self) -> u64 {
        self.instructions
    }
    pub fn reads_memory(&self) -> bool {
        self.body.iter().any(|op| matches!(op, Op::Load { .. }))
    }
    fn apply(
        &self,
        state: State,
        read: impl FnMut(u32, usize) -> Option<u32>,
    ) -> Result<State, AdvanceFailure> {
        Self::apply_ops(state, &self.body, read)
    }
    fn apply_ops(
        mut state: State,
        ops: &[Op],
        mut read: impl FnMut(u32, usize) -> Option<u32>,
    ) -> Result<State, AdvanceFailure> {
        let mut skip = false;
        for op in ops {
            if std::mem::take(&mut skip) {
                continue;
            }
            match *op {
                Op::NoEffect => (),
                Op::Predicate(condition) => skip = !arm::condition_passes(condition, state.cpsr),
                Op::Exit { register, nonzero } => {
                    if (state.registers[register] != 0) == nonzero {
                        return Err(AdvanceFailure::ConditionNotTaken);
                    }
                }
                Op::Load {
                    dst,
                    base,
                    offset,
                    index,
                    width,
                } => {
                    let index = index.map_or(0, |(r, shift)| state.registers[r] << shift);
                    let address = state.registers[base]
                        .wrapping_add(offset)
                        .wrapping_add(index);
                    state.registers[dst] =
                        read(address, width).ok_or(AdvanceFailure::ReadUnavailable)?;
                }
                Op::Data {
                    opcode,
                    flags,
                    dst,
                    lhs,
                    rhs,
                } => {
                    let a = state.registers[lhs];
                    let (b, shifted_carry) = rhs.value(state);
                    let (result, carry, overflow) = match opcode {
                        2 | 10 => {
                            let result = a.wrapping_sub(b);
                            (result, a >= b, ((a ^ b) & (a ^ result)) >> 31 != 0)
                        }
                        4 | 11 => {
                            let (result, carry) = a.overflowing_add(b);
                            (result, carry, (!(a ^ b) & (a ^ result)) >> 31 != 0)
                        }
                        _ => (
                            match opcode {
                                0 | 8 => a & b,
                                1 | 9 => a ^ b,
                                12 => a | b,
                                13 => b,
                                14 => a & !b,
                                15 => !b,
                                _ => unreachable!(),
                            },
                            shifted_carry,
                            state.cpsr & (1 << 28) != 0,
                        ),
                    };
                    if !(8..=11).contains(&opcode) {
                        state.registers[dst] = result;
                    }
                    if flags {
                        state.cpsr = (state.cpsr & 0x0fff_ffff)
                            | (result & 0x8000_0000)
                            | (u32::from(result == 0) << 30)
                            | (u32::from(carry) << 29)
                            | (u32::from(overflow) << 28);
                    }
                }
            }
        }
        Ok(state)
    }
    fn taken(&self, state: State) -> bool {
        arm::condition_passes(self.condition, state.cpsr)
    }
    /// A single unit-step counter, followed only by a fixed comparison.
    /// Restricting the algebra keeps its first-exit proof small and auditable.
    fn counter(&self, state: State) -> Option<(usize, i64, u32)> {
        let (&first, rest) = self.body.split_first()?;
        let Op::Data {
            opcode,
            flags,
            dst,
            lhs,
            rhs: Operand::Immediate(1, _),
        } = first
        else {
            return None;
        };
        if dst != lhs || !matches!(opcode, 2 | 4) {
            return None;
        }
        let compare = match rest {
            [] if flags => 0,
            [
                Op::Data {
                    opcode: 10,
                    lhs,
                    rhs,
                    ..
                },
            ] if *lhs == dst => match *rhs {
                Operand::Immediate(value, _) => value,
                Operand::Register(r, 0, 0) if r != dst => state.registers[r],
                _ => return None,
            },
            _ => return None,
        };
        matches!(self.condition, 1 | 2 | 3 | 8 | 9).then_some((
            dst,
            if opcode == 2 { -1 } else { 1 },
            compare,
        ))
    }
    /// The supported iteration pattern is determined by decoded instruction
    /// shape, register roles, and branch condition. Register contents only
    /// determine the comparison value at runtime.
    fn has_iteration_model(&self) -> bool {
        let Some((&first, rest)) = self.body.split_first() else {
            return false;
        };
        let Op::Data {
            opcode,
            flags,
            dst,
            lhs,
            rhs: Operand::Immediate(1, _),
            ..
        } = first
        else {
            return false;
        };
        if dst != lhs || !matches!(opcode, 2 | 4) {
            return false;
        }
        (match rest {
            [] => flags,
            [
                Op::Data {
                    opcode: 10,
                    lhs,
                    rhs,
                    ..
                },
            ] if *lhs == dst => match *rhs {
                Operand::Immediate(_, _) => true,
                Operand::Register(r, 0, 0) => r != dst,
                _ => false,
            },
            _ => false,
        }) && matches!(self.condition, 1 | 2 | 3 | 8 | 9)
    }
    /// Advance at most `iterations` complete iterations from `state`, which is
    /// the state at the branch. `read` must answer only reads that stay
    /// stable for the whole window.
    pub fn advance(
        &self,
        state: State,
        iterations: u64,
        mut read: impl FnMut(u32, usize) -> Option<u32>,
    ) -> Result<Advance, AdvanceFailure> {
        if iterations == 0 {
            return Err(AdvanceFailure::BudgetRoom);
        }
        if !self.taken(state) {
            return Err(AdvanceFailure::ConditionNotTaken);
        }
        let next = self.apply(state, &mut read)?;
        // Fixed point of the complete architectural state. The read callback
        // proves that every addressed byte is stable for this entire window.
        if next == state {
            return Ok(Advance {
                state,
                iterations,
                counted: false,
            });
        }
        if !self.has_iteration_model() {
            // Leaving after this iteration, or a polled input that changed
            // since the previous one, says nothing about the loop's shape.
            return Err(if !self.taken(next) {
                AdvanceFailure::ConditionNotTaken
            } else if self.reads_memory() {
                AdvanceFailure::InputsChanged
            } else {
                AdvanceFailure::IterationModelUnsupported
            });
        }
        let (counter, step, compare) = self
            .counter(state)
            .ok_or(AdvanceFailure::CounterAtBoundary)?;
        let value = state.registers[counter];
        let distance = if step < 0 {
            u64::from(value)
        } else {
            u64::from(u32::MAX - value)
        };
        let mut limit = iterations.min(distance);
        // NE can become false at equality and true again afterwards.
        if self.condition == 1 {
            let equal = if step < 0 {
                value.checked_sub(compare)
            } else {
                compare.checked_sub(value)
            };
            if let Some(equal) = equal.filter(|&n| n != 0) {
                limit = limit.min(u64::from(equal));
            }
        }
        if limit == 0 {
            return Err(AdvanceFailure::CounterAtBoundary);
        }
        // The counted model has neither loads nor exits, so it cannot fail.
        let at = |n: u64| {
            let mut before = state;
            before.registers[counter] = (i64::from(value) + step * (n as i64 - 1)) as u32;
            self.apply(before, |_, _| None).unwrap()
        };
        // Unsigned comparisons are monotonic between the no-wrap boundaries.
        if !self.taken(at(limit)) {
            let (mut low, mut high) = (1, limit);
            while low < high {
                let mid = low + (high - low) / 2;
                if self.taken(at(mid)) {
                    low = mid + 1;
                } else {
                    high = mid;
                }
            }
            limit = low;
        }
        Ok(Advance {
            state: at(limit),
            iterations: limit,
            counted: true,
        })
    }
}

impl Operand {
    fn value(self, state: State) -> (u32, bool) {
        let carry = state.cpsr & (1 << 29) != 0;
        match self {
            Self::Immediate(value, out) => (value, out.unwrap_or(carry)),
            Self::RegisterShift(r, kind, count) => {
                let value = state.registers[r];
                let shift = state.registers[count] & 255;
                match (kind, shift) {
                    (_, 0) => (value, carry),
                    (0, 1..=31) => (value << shift, value & (1 << (32 - shift)) != 0),
                    (0, 32) => (0, value & 1 != 0),
                    (0, _) => (0, false),
                    (1, 1..=31) => (value >> shift, value & (1 << (shift - 1)) != 0),
                    (1, 32) => (0, value >> 31 != 0),
                    (1, _) => (0, false),
                    (2, 1..=31) => (
                        ((value as i32) >> shift) as u32,
                        value & (1 << (shift - 1)) != 0,
                    ),
                    (2, _) => (((value as i32) >> 31) as u32, value >> 31 != 0),
                    (3, _) => {
                        let result = value.rotate_right(shift);
                        (result, result >> 31 != 0)
                    }
                    _ => unreachable!(),
                }
            }
            Self::Register(r, kind, shift) => {
                let value = state.registers[r];
                match (kind, shift) {
                    (0, 0) => (value, carry),
                    (0, n) => (value << n, value & (1 << (32 - n)) != 0),
                    (1, 0) => (0, value >> 31 != 0),
                    (1, n) => (value >> n, value & (1 << (n - 1)) != 0),
                    (2, 0) => (((value as i32) >> 31) as u32, value >> 31 != 0),
                    (2, n) => (((value as i32) >> n) as u32, value & (1 << (n - 1)) != 0),
                    (3, 0) => ((u32::from(carry) << 31) | (value >> 1), value & 1 != 0),
                    (3, n) => (value.rotate_right(n), value & (1 << (n - 1)) != 0),
                    _ => unreachable!(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
