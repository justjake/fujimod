//! Exact fast-forward of signed ARM countdown loops whose writes are confined
//! to registers. One nested countdown is allowed when its initial counter is
//! invariant in the outer loop.
use super::{Op, Operand, Proof, State, a32};

/// A counting loop that shifts an unsigned register right until it is at
/// most an immediate bound, incrementing a separate register each iteration.
pub struct ShiftCountLoop {
    shifted: usize,
    counter: usize,
    shift: u32,
    step: u32,
    bound: u32,
    compare: Op,
}

impl ShiftCountLoop {
    /// Decode the three instructions before a BHI back to the shift.
    pub fn decode([shift, increment, compare]: [u32; 3]) -> Option<Self> {
        let Op::Data {
            opcode: 13,
            flags: false,
            dst: shifted,
            lhs: 0,
            rhs: Operand::Register(source, 1, shift),
        } = a32::decode(shift)?
        else {
            return None;
        };
        let Op::Data {
            opcode: 4,
            flags: false,
            dst: counter,
            lhs,
            rhs: Operand::Immediate(step, _),
        } = a32::decode(increment)?
        else {
            return None;
        };
        let compare = a32::decode(compare)?;
        let Op::Data {
            opcode: 10,
            flags: true,
            lhs: compared,
            rhs: Operand::Immediate(bound, _),
            ..
        } = compare
        else {
            return None;
        };
        if shifted != source || shifted != compared || counter != lhs || shifted == counter {
            return None;
        }
        Some(Self {
            shifted,
            counter,
            shift: if shift == 0 { 32 } else { shift },
            step,
            bound,
            compare,
        })
    }

    /// Consume complete iterations from the taken branch, leaving the PC at
    /// the branch after the last comparison.
    pub fn advance(&self, mut state: State, budget: u64) -> Option<(State, u64)> {
        const COST: u64 = 4;
        if !crate::arm::condition_passes(8, state.cpsr) || budget < COST {
            return None;
        }
        let value = state.registers[self.shifted];
        let bit_difference = self
            .bound
            .leading_zeros()
            .saturating_sub(value.leading_zeros());
        let estimate = bit_difference.div_ceil(self.shift).max(1);
        let shifted = |rounds| value.checked_shr(rounds * self.shift).unwrap_or(0);
        // At the estimate the value has at most as many bits as the bound.
        // If it is still larger, one more right shift makes it smaller.
        let until_exit = estimate + u32::from(shifted(estimate) > self.bound);
        let rounds = u64::from(until_exit).min(budget / COST) as u32;
        state.registers[self.shifted] = shifted(rounds);
        state.registers[self.counter] =
            state.registers[self.counter].wrapping_add(self.step.wrapping_mul(rounds));
        let state = Proof::apply_ops(state, &[self.compare], |_, _| None).ok()?;
        Some((state, u64::from(rounds) * COST))
    }
}

pub struct RegisterLoop {
    counter: usize,
    before: Vec<Op>,
    inner: Option<(Operand, Box<RegisterLoop>)>,
    after: Vec<Op>,
    instructions: u64,
    inputs: u16,
    writes: u16,
}

impl RegisterLoop {
    /// `no_effect` classifies architectural operations that this processor
    /// model guarantees have no observable effect for the duration of the run.
    pub fn decode(start: u64, bytes: &[u8], no_effect: impl Fn(u32) -> bool) -> Option<Self> {
        let (words, []) = bytes.as_chunks::<4>() else {
            return None;
        };
        let words: Vec<_> = words.iter().copied().map(u32::from_le_bytes).collect();
        Self::parse(start, &words, &no_effect, true)
    }

    fn parse(
        start: u64,
        words: &[u32],
        no_effect: &impl Fn(u32) -> bool,
        nested: bool,
    ) -> Option<Self> {
        let (&branch, body) = words.split_last()?;
        if branch >> 28 != 10 || a32::target(start + 4 * body.len() as u64, branch)? != start {
            return None;
        }
        let (&last, _) = body.split_last()?;
        let Op::Data {
            opcode: 2,
            flags: true,
            dst: counter,
            lhs,
            rhs: Operand::Immediate(1, _),
        } = a32::decode(last)?
        else {
            return None;
        };
        if counter != lhs {
            return None;
        };

        let decode = |words: &[u32]| {
            words
                .iter()
                .map(|&word| {
                    if no_effect(word) {
                        Some(Op::NoEffect)
                    } else {
                        a32::decode(word)
                    }
                })
                .collect::<Option<Vec<_>>>()
        };
        let mut inner = None;
        let (prefix, suffix) = if let Some((index, &word)) = body
            .iter()
            .enumerate()
            .find(|&(_, word)| word & 0x0e00_0000 == 0x0a00_0000)
        {
            if !nested {
                return None;
            }
            let at = start + 4 * index as u64;
            let target = a32::target(at, word)?;
            let from = usize::try_from(target.checked_sub(start)? / 4).ok()?;
            let child = Self::parse(target, body.get(from..=index)?, no_effect, false)?;
            let prefix = body.get(..from)?;
            let Op::Data {
                opcode: 13,
                flags: false,
                dst,
                rhs: initial @ (Operand::Immediate(..) | Operand::Register(_, 0, 0)),
                ..
            } = a32::decode(*prefix.last()?)?
            else {
                return None;
            };
            if dst != child.counter {
                return None;
            }
            inner = Some((initial, Box::new(child)));
            (prefix, body.get(index + 1..)?)
        } else {
            (&body[..0], body)
        };

        let before = decode(prefix)?;
        let after = decode(suffix)?;
        let mut inputs = 0;
        let mut writes = 0;
        let mut record = |reads, destinations| {
            inputs |= reads & !writes;
            writes |= destinations;
        };
        for &op in &before {
            let (reads, destinations) = reads_writes(op)?;
            record(reads, destinations);
        }
        if let Some((_, child)) = &inner {
            record(child.inputs, child.writes);
        }
        let (decrement, rest) = after.split_last()?;
        for &op in rest {
            let (reads, destinations) = reads_writes(op)?;
            record(reads, destinations);
        }
        let counter_mask = 1 << counter;
        if writes & counter_mask != 0 || inputs & writes != 0 {
            return None;
        }
        let (reads, destinations) = reads_writes(*decrement)?;
        inputs |= reads & !writes;
        writes |= destinations;
        if let Some((Operand::Register(register, _, _), _)) = &inner
            && writes & (1 << register) != 0
        {
            return None;
        }

        Some(Self {
            counter,
            instructions: (before.len() + after.len() + 1) as u64,
            before,
            inner,
            after,
            inputs,
            writes,
        })
    }

    /// Return the new state at the same branch and exact guest entries
    /// consumed. The last iteration is evaluated with its real counter.
    pub fn advance(&self, state: State, budget: u64) -> Option<(State, u64)> {
        if !crate::arm::condition_passes(10, state.cpsr) {
            return None;
        }
        let counter = state.registers[self.counter];
        let rounds = Self::iterations(counter)?;
        let cost = self.cost(state)?;
        let count = rounds.min(budget / cost);
        if count == 0 {
            return None;
        }
        let state = self.evaluate(state, counter - (count - 1) as u32)?;
        Some((state, count * cost))
    }

    fn iterations(counter: u32) -> Option<u64> {
        i32::try_from(counter).ok().map(|_| u64::from(counter) + 1)
    }

    fn cost(&self, state: State) -> Option<u64> {
        let inner = if let Some((initial, child)) = &self.inner {
            Self::iterations(initial.value(state).0)?.checked_mul(child.instructions)?
        } else {
            0
        };
        self.instructions.checked_add(inner)
    }

    fn evaluate(&self, mut state: State, last_counter: u32) -> Option<State> {
        state.registers[self.counter] = last_counter;
        state = Proof::apply_ops(state, &self.before, |_, _| None).ok()?;
        if let Some((_, child)) = &self.inner {
            // The nested loop executes to completion. Its last body begins
            // at zero and the parent's initialization proves the count.
            state = child.evaluate(state, 0)?;
        }
        Proof::apply_ops(state, &self.after, |_, _| None).ok()
    }
}

fn reads_writes(op: Op) -> Option<(u16, u16)> {
    let Op::Data {
        opcode,
        dst,
        lhs,
        rhs,
        ..
    } = op
    else {
        return matches!(op, Op::NoEffect).then_some((0, 0));
    };
    let rhs = match rhs {
        Operand::Immediate(..) => 0,
        Operand::Register(_, 3, 0) => return None, // RRX consumes incoming carry.
        Operand::Register(r, _, _) => 1 << r,
        Operand::RegisterShift(r, _, shift) => (1 << r) | (1 << shift),
    };
    let inputs = rhs
        | if matches!(opcode, 13 | 15) {
            0
        } else {
            1 << lhs
        };
    let writes = if (8..=11).contains(&opcode) {
        0
    } else {
        1 << dst
    };
    Some((inputs, writes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::armv7a;

    const NESTED: [u32; 8] = [
        0xe1a0_1003, // mov r1, r3
        0xe184_2511, // orr r2, r4, r1, lsl r5
        0x0e07_2f5a, // dccsweq r2
        0xe251_1001, // subs r1, r1, #1
        0xaaff_fffb, // bge to orr
        0xe082_6000, // add r6, r2, r0
        0xe250_0001, // subs r0, r0, #1
        0xaaff_fff7, // bge to mov
    ];

    fn decode(words: [u32; 8], no_effect: impl Fn(u32) -> bool) -> Option<RegisterLoop> {
        let bytes = words
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        RegisterLoop::decode(0x4000, &bytes, no_effect)
    }

    #[test]
    fn charges_nested_entries_and_reconstructs_flags_at_the_exit() {
        let proof = decode(NESTED, armv7a::data_cache_set_way).unwrap();
        let mut before = State {
            registers: [0; 15],
            cpsr: 0xd3,
        };
        before.registers[0] = 2;
        before.registers[3] = 3;
        before.registers[4] = 7;
        before.registers[5] = 3;

        assert!(proof.advance(before, 19).is_none());
        let (intermediate, entries) = proof.advance(before, 41).unwrap();
        assert_eq!(entries, 40);
        assert_eq!(intermediate.registers[0], 0);
        assert_eq!(intermediate.registers[6], 8);
        assert_eq!(intermediate.cpsr, 0x6000_00d3);
        let (after, entries) = proof.advance(intermediate, 99).unwrap();
        assert_eq!(entries, 20);
        assert_eq!(after.registers[0], u32::MAX);
        assert_eq!(after.registers[1], u32::MAX);
        assert_eq!(after.registers[6], 7);
        assert_eq!(after.cpsr, 0x8000_00d3);
        assert!(proof.advance(after, 99).is_none());
    }

    #[test]
    fn refuses_unmodeled_system_effects_stores_and_variant_nested_counts() {
        assert!(decode(NESTED, |_| false).is_none());
        let mut with_store = NESTED;
        with_store[2] = 0xe580_1000; // str r1, [r0]
        assert!(decode(with_store, armv7a::data_cache_set_way).is_none());
        let mut variant_count = NESTED;
        variant_count[0] = 0xe1a0_1000; // mov r1, r0
        assert!(decode(variant_count, armv7a::data_cache_set_way).is_none());
    }
}
