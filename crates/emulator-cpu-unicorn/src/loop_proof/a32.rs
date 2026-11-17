//! A32 loop decode.
use super::{Op, Operand, Proof, SPAN};

/// Only a direct, backward A32 B (never BL, BLX or the unconditional space).
pub(super) fn target(address: u64, word: u32) -> Option<u64> {
    if word >> 28 == 15 || word & 0x0f00_0000 != 0x0a00_0000 {
        return None;
    }
    let displacement = ((word << 8) as i32 >> 6) as i64;
    let start = address.checked_add(8)?.checked_add_signed(displacement)?;
    (start <= address && address - start <= SPAN).then_some(start)
}
pub(super) fn decode(word: u32) -> Option<Op> {
    // Conditional bodies, PC, writeback, register-offset loads, atomics,
    // coprocessors, stores, barriers and hints are deliberately unsupported.
    if word >> 28 != 14 {
        return None;
    }
    let dst = ((word >> 12) & 15) as usize;
    let lhs = ((word >> 16) & 15) as usize;
    if dst == 15 || lhs == 15 {
        return None;
    }
    if word & 0x0f30_0000 == 0x0510_0000 {
        let offset = word & 0xfff;
        return Some(Op::Load {
            dst,
            base: lhs,
            offset: if word & (1 << 23) == 0 {
                0u32.wrapping_sub(offset)
            } else {
                offset
            },
            index: None,
            width: if word & (1 << 22) == 0 { 4 } else { 1 },
        });
    }
    if word & 0x0c00_0000 != 0 {
        return None;
    }
    let opcode = (word >> 21) & 15;
    if !matches!(opcode, 0 | 1 | 2 | 4 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15) {
        return None;
    }
    let flags = word & (1 << 20) != 0;
    if (8..=11).contains(&opcode) && (!flags || dst != 0) {
        return None;
    }
    let rhs = if word & (1 << 25) != 0 {
        let rotate = ((word >> 8) & 15) * 2;
        let value = (word & 255).rotate_right(rotate);
        Operand::Immediate(value, (rotate != 0).then_some(value >> 31 != 0))
    } else {
        if word & 16 != 0 || word & 15 == 15 {
            return None;
        }
        Operand::Register((word & 15) as usize, (word >> 5) & 3, (word >> 7) & 31)
    };
    Some(Op::Data {
        opcode,
        flags,
        dst,
        lhs,
        rhs,
    })
}
pub(super) fn proof(start: u64, branch: u64, bytes: &[u8]) -> Option<Proof> {
    let (words, rest) = bytes.as_chunks::<4>();
    if !rest.is_empty() || start + 4 * words.len() as u64 != branch + 4 {
        return None;
    }
    let (&word, body) = words.split_last()?;
    let word = u32::from_le_bytes(word);
    if target(branch, word)? != start {
        return None;
    }
    Some(Proof {
        branch,
        condition: word >> 28,
        instructions: words.len() as u64,
        body: body
            .iter()
            .map(|&bytes| decode(u32::from_le_bytes(bytes)))
            .collect::<Option<_>>()?,
    })
}
