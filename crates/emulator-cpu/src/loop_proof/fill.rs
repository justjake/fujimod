//! Exact state and memory span of a counted A32 word fill.
use super::{Op, Operand, Proof, State, a32};

/// An incrementing word store, a decrement, and a branch while nonzero.
pub struct WordFillLoop {
    address: usize,
    value: usize,
    count: usize,
    decrement: Op,
}

/// The writes and register state for complete iterations from the branch.
pub struct WordFillAdvance {
    pub value: u32,
    pub words: usize,
    pub state: State,
    pub entries: u64,
}

impl WordFillLoop {
    /// Decode the instructions before a BNE back to the store.
    pub fn decode([store, decrement]: [u32; 2]) -> Option<Self> {
        // STR Rt, [Rn], #4 with an unconditional predicate and word access.
        if store & 0xfff0_0fff != 0xe480_0004 {
            return None;
        }
        let address = ((store >> 16) & 15) as usize;
        let value = ((store >> 12) & 15) as usize;
        let decrement = a32::decode(decrement)?;
        let Op::Data {
            opcode: 2,
            flags: true,
            dst: count,
            lhs,
            rhs: Operand::Immediate(1, _),
        } = decrement
        else {
            return None;
        };
        if address == 15
            || value == 15
            || value == address
            || count != lhs
            || count == value
            || count == address
        {
            return None;
        }
        Some(Self {
            address,
            value,
            count,
            decrement,
        })
    }

    pub fn address(&self, state: &State) -> u32 {
        state.registers[self.address]
    }

    /// Plan complete iterations up to the scheduler's budget and the word
    /// range already checked writable by the caller.
    pub fn advance(&self, mut state: State, budget: u64, limit: u32) -> Option<WordFillAdvance> {
        const COST: u64 = 3;
        if !crate::arm::condition_passes(1, state.cpsr) {
            return None;
        }
        let count = state.registers[self.count];
        let iterations = u64::from(count.min(limit)).min(budget / COST) as u32;
        if iterations == 0 {
            return None;
        }
        let value = state.registers[self.value];
        state.registers[self.address] =
            state.registers[self.address].wrapping_add(iterations.wrapping_mul(4));
        // Reconstruct the flags from the final subtraction's actual input.
        state.registers[self.count] = count - iterations + 1;
        let state = Proof::apply_ops(state, &[self.decrement], |_, _| None).ok()?;
        Some(WordFillAdvance {
            value,
            words: iterations as usize,
            state,
            entries: u64::from(iterations) * COST,
        })
    }
}
