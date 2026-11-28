//! Bounded register-only scaling and subtracting loops used in unsigned division.
use super::{Op, Operand, Proof, State, a32};

pub struct DivisionScaleLoop {
    divisor: usize,
    dividend: usize,
    place: usize,
    limit: u32,
    shift: u32,
    compare_limit: Op,
    compare_dividend: Op,
}

impl DivisionScaleLoop {
    pub fn decode(
        [compare_limit, compare_dividend, shift_divisor, shift_place]: [u32; 4],
    ) -> Option<Self> {
        let compare_limit = a32::decode(compare_limit)?;
        let compare_dividend = under(compare_dividend, 3)?;
        let Op::Data {
            opcode: 10,
            lhs: divisor,
            rhs: Operand::Immediate(limit, _),
            ..
        } = compare_limit
        else {
            return None;
        };
        let Op::Data {
            opcode: 10,
            lhs,
            rhs: Operand::Register(dividend, 0, 0),
            ..
        } = compare_dividend
        else {
            return None;
        };
        let (shifted_divisor, shift) = shift_register(under(shift_divisor, 3)?, false, 0)?;
        let (place, place_shift) = shift_register(under(shift_place, 3)?, false, 0)?;
        // An executed shift must never truncate the divisor. The place
        // register keeps the architectural wrapping behavior independently.
        if lhs != divisor
            || shifted_divisor != divisor
            || shift != place_shift
            || shift == 0
            || limit == 0
            || limit > (1 << (32 - shift))
            || divisor == dividend
            || divisor == place
            || dividend == place
        {
            return None;
        }
        Some(Self {
            divisor,
            dividend,
            place,
            limit,
            shift,
            compare_limit,
            compare_dividend,
        })
    }

    pub fn advance(&self, mut state: State, budget: u64) -> Option<(State, u64)> {
        const COST: u64 = 5;
        if !crate::arm::condition_passes(3, state.cpsr) || budget < COST {
            return None;
        }
        let value = state.registers[self.divisor];
        let limit = self.limit.min(state.registers[self.dividend]);
        let allowed = budget / COST;
        let taken = match value {
            _ if value >= limit => 0,
            0 => allowed,
            _ => {
                let difference = value.leading_zeros() - limit.leading_zeros();
                let estimate = difference.div_ceil(self.shift);
                let below = (u64::from(value) << (estimate * self.shift)) < u64::from(limit);
                u64::from(estimate + u32::from(below))
            }
        };
        // One more body evaluation after the last shift sets the branch's
        // exit flags, provided the remaining budget can pay for it.
        let rounds = allowed.min(taken + 1);
        let shifted = |value: u32, count: u64| {
            let bits = count.saturating_mul(u64::from(self.shift)).min(32) as u32;
            value.checked_shl(bits).unwrap_or(0)
        };
        let last_value = shifted(value, (rounds - 1).min(taken));
        state.registers[self.divisor] = last_value;
        let compare = if last_value >= self.limit {
            self.compare_limit
        } else {
            self.compare_dividend
        };
        state = Proof::apply_ops(state, &[compare], |_, _| None).ok()?;
        state.registers[self.divisor] = shifted(value, rounds.min(taken));
        state.registers[self.place] = shifted(state.registers[self.place], rounds.min(taken));
        Some((state, rounds * COST))
    }
}

pub struct DivisionRestoreLoop {
    dividend: usize,
    divisor: usize,
    place: usize,
    quotient: usize,
}

impl DivisionRestoreLoop {
    const DIGITS: u32 = 4;
    const COST: u64 = 16;

    pub fn decode(body: [u32; 15]) -> Option<Self> {
        let (dividend, divisor) = match a32::decode(body[0])? {
            Op::Data {
                opcode: 10,
                lhs,
                rhs: Operand::Register(divisor, 0, 0),
                ..
            } => (lhs, divisor),
            _ => return None,
        };
        let (quotient, place) = match under(body[2], 2)? {
            Op::Data {
                opcode: 12,
                flags: false,
                dst,
                lhs,
                rhs: Operand::Register(place, 0, 0),
            } if dst == lhs => (dst, place),
            _ => return None,
        };
        if (1u16 << dividend | 1 << divisor | 1 << place | 1 << quotient).count_ones() != 4 {
            return None;
        }
        for (index, [compare, subtract, accumulate]) in
            body[..12].as_chunks::<3>().0.iter().enumerate()
        {
            let operand_matches = |rhs: Operand, register| {
                matches!(rhs, Operand::Register(source, kind, shift)
                    if source == register && kind == u32::from(index != 0) && shift == index as u32)
            };
            if !matches!(a32::decode(*compare)?, Op::Data { opcode: 10, lhs, rhs, .. }
                if lhs == dividend && operand_matches(rhs, divisor))
                || !matches!(under(*subtract, 2)?, Op::Data { opcode: 2, flags: false, dst, lhs, rhs }
                    if dst == dividend && lhs == dividend && operand_matches(rhs, divisor))
                || !matches!(under(*accumulate, 2)?, Op::Data { opcode: 12, flags: false, dst, lhs, rhs }
                    if dst == quotient && lhs == quotient && operand_matches(rhs, place))
            {
                return None;
            }
        }
        if !matches!(a32::decode(body[12])?, Op::Data { opcode: 10, lhs, rhs: Operand::Immediate(0, _), .. }
            if lhs == dividend)
            || shift_register(under(body[13], 1)?, true, 1)? != (place, Self::DIGITS)
            || shift_register(under(body[14], 1)?, false, 1)? != (divisor, Self::DIGITS)
        {
            return None;
        }
        Some(Self {
            dividend,
            divisor,
            place,
            quotient,
        })
    }

    pub fn advance(&self, mut state: State, budget: u64) -> Option<(State, u64)> {
        if !crate::arm::condition_passes(1, state.cpsr) || budget < Self::COST {
            return None;
        }
        let mut dividend = state.registers[self.dividend];
        let mut divisor = state.registers[self.divisor];
        let mut place = state.registers[self.place];
        let mut quotient = state.registers[self.quotient];
        let mut entries = 0;
        let mut flags = 0;
        for _ in 0..budget / Self::COST {
            for bit in 0..Self::DIGITS {
                let (difference, borrow) = dividend.overflowing_sub(divisor >> bit);
                if !borrow {
                    dividend = difference;
                    quotient |= place >> bit;
                }
            }
            entries += Self::COST;
            if dividend == 0 {
                // CMP dividend, #0 sets Z and C. Both shifts are predicated out.
                flags = 0x6000_0000;
                break;
            }
            flags = (place & (1 << (Self::DIGITS - 1))) << (30 - Self::DIGITS);
            place >>= Self::DIGITS;
            if place == 0 {
                flags |= 1 << 30;
                break;
            }
            // The divisor shift consumes the flags produced by MOVS place.
            divisor >>= Self::DIGITS;
        }
        state.registers[self.dividend] = dividend;
        state.registers[self.divisor] = divisor;
        state.registers[self.place] = place;
        state.registers[self.quotient] = quotient;
        state.cpsr = (state.cpsr & 0x0fff_ffff) | flags;
        Some((state, entries))
    }
}

fn under(word: u32, condition: u32) -> Option<Op> {
    if word >> 28 != condition {
        return None;
    }
    a32::decode(word & 0x0fff_ffff | 0xe000_0000)
}

fn shift_register(op: Op, flags: bool, kind: u32) -> Option<(usize, u32)> {
    let Op::Data {
        opcode: 13,
        flags: set_flags,
        dst,
        lhs: 0,
        rhs: Operand::Register(source, shift_kind, shift),
    } = op
    else {
        return None;
    };
    (set_flags == flags && shift_kind == kind && dst == source).then_some((dst, shift))
}
