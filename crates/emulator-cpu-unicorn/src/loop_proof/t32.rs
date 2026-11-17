//! T32 loop decode: the A32 subset in its Thumb encodings, plus CBZ/CBNZ
//! exits and IT blocks whose predicated instructions are all in the subset.
//! An IT block must close before the loop branch.
use super::{Op, Operand, Proof, SPAN};

fn wide(first: u16) -> bool {
    first >> 11 >= 0b11101
}
/// A direct B: its target, condition and size. Never BL, BLX or CBZ.
fn branch(address: u64, first: u16, second: u16) -> Option<(u64, u32, u64)> {
    let (first, second) = (u32::from(first), u32::from(second));
    let (displacement, condition, size) = if first & 0xf000 == 0xd000 {
        let condition = (first >> 8) & 15;
        if condition >= 14 {
            return None;
        }
        (i64::from(first as u8 as i8) * 2, condition, 2)
    } else if first & 0xf800 == 0xe000 {
        (i64::from(((first & 0x7ff) << 5) as u16 as i16 >> 4), 14, 2)
    } else if first & 0xf800 == 0xf000 && second & 0xd000 == 0x8000 {
        let condition = (first >> 6) & 15;
        if condition >= 14 {
            return None;
        }
        let imm = ((first >> 10) & 1) << 20
            | ((second >> 11) & 1) << 19
            | ((second >> 13) & 1) << 18
            | (first & 0x3f) << 12
            | (second & 0x7ff) << 1;
        (i64::from((imm << 11) as i32 >> 11), condition, 4)
    } else if first & 0xf800 == 0xf000 && second & 0xd000 == 0x9000 {
        let s = (first >> 10) & 1;
        let i1 = !((second >> 13) ^ s) & 1;
        let i2 = !((second >> 11) ^ s) & 1;
        let imm = s << 24 | i1 << 23 | i2 << 22 | (first & 0x3ff) << 12 | (second & 0x7ff) << 1;
        (i64::from((imm << 7) as i32 >> 7), 14, 4)
    } else {
        return None;
    };
    Some((
        address.checked_add(4)?.checked_add_signed(displacement)?,
        condition,
        size,
    ))
}
/// The loop head and branch size of a short backward B at `address`.
pub(super) fn target(address: u64, word: u32) -> Option<(u64, u64)> {
    let (start, _, size) = branch(address, word as u16, (word >> 16) as u16)?;
    (start <= address && address - start <= SPAN).then_some((start, size))
}
pub(super) fn proof(start: u64, branch_address: u64, bytes: &[u8]) -> Option<Proof> {
    let halfword = |offset: u64| {
        let offset = usize::try_from(offset).ok()?;
        let bytes = bytes.get(offset..offset.checked_add(2)?)?;
        Some(u16::from_le_bytes([bytes[0], bytes[1]]))
    };
    let end = start.checked_add(bytes.len() as u64)?;
    let mut body = Vec::new();
    let mut instructions = 0;
    // ITSTATE: first condition and mask, advanced after each instruction.
    let mut it = 0u8;
    let mut at = start;
    loop {
        let first = halfword(at - start)?;
        let second = if wide(first) {
            halfword(at - start + 2)?
        } else {
            0
        };
        let size = if wide(first) { 4 } else { 2 };
        instructions += 1;
        if at == branch_address {
            let (target, condition, branch_size) = branch(at, first, second)?;
            if it != 0 || target != start || at + branch_size != end {
                return None;
            }
            return Some(Proof {
                branch: branch_address,
                condition,
                instructions,
                body,
            });
        }
        if at + size > branch_address {
            return None;
        }
        if first & 0xff00 == 0xbf00 && first & 15 != 0 {
            if it != 0 || (first >> 4) & 15 >= 14 {
                return None;
            }
            it = first as u8;
            at += 2;
            continue;
        }
        if it != 0 {
            body.push(Op::Predicate(u32::from(it >> 4)));
        }
        body.push(if wide(first) {
            decode32(first, second)?
        } else {
            decode16(first, it != 0, at, end)?
        });
        it = if it & 7 == 0 {
            0
        } else {
            (it & 0xe0) | ((it << 1) & 0x1f)
        };
        at += size;
    }
}
/// A 16-bit instruction. Inside an IT block the flag-setting forms other
/// than comparisons leave the flags unchanged.
fn decode16(word: u16, in_it: bool, address: u64, end: u64) -> Option<Op> {
    let flags = !in_it;
    let r = |shift: u16| usize::from((word >> shift) & 7);
    let word32 = u32::from(word);
    let data = |opcode, flags, dst, lhs, rhs| {
        Some(Op::Data {
            opcode,
            flags,
            dst,
            lhs,
            rhs,
        })
    };
    let load = |dst, base, offset, index, width| {
        Some(Op::Load {
            dst,
            base,
            offset,
            index,
            width,
        })
    };
    match word >> 11 {
        // LSLS/LSRS/ASRS immediate; LSLS #0 is MOVS, which IT forbids.
        0b00000..=0b00010 => {
            let (kind, shift) = (word32 >> 11, (word32 >> 6) & 31);
            if kind == 0 && shift == 0 && in_it {
                return None;
            }
            data(13, flags, r(0), 0, Operand::Register(r(3), kind, shift))
        }
        0b00011 => data(
            if word & 0x200 != 0 { 2 } else { 4 },
            flags,
            r(0),
            r(3),
            if word & 0x400 != 0 {
                Operand::Immediate((word32 >> 6) & 7, None)
            } else {
                Operand::Register(r(6), 0, 0)
            },
        ),
        0b00100 => data(13, flags, r(8), 0, Operand::Immediate(word32 & 255, None)),
        0b00101 => data(10, true, 0, r(8), Operand::Immediate(word32 & 255, None)),
        0b00110 => data(4, flags, r(8), r(8), Operand::Immediate(word32 & 255, None)),
        0b00111 => data(2, flags, r(8), r(8), Operand::Immediate(word32 & 255, None)),
        0b01000 if word & 0x400 == 0 => {
            let opcode = match (word32 >> 6) & 15 {
                op @ (0 | 1 | 8 | 10 | 11 | 12 | 14 | 15) => op,
                _ => return None,
            };
            let rhs = Operand::Register(r(3), 0, 0);
            if (8..=11).contains(&opcode) {
                data(opcode, true, 0, r(0), rhs)
            } else {
                data(
                    opcode,
                    flags,
                    r(0),
                    if opcode == 15 { 0 } else { r(0) },
                    rhs,
                )
            }
        }
        // High-register ADD, CMP and MOV; BX and BLX are unsupported.
        0b01000 => {
            let rd = usize::from((word >> 4) & 8 | word & 7);
            let rm = usize::from((word >> 3) & 15);
            if rd == 15 || rm == 15 {
                return None;
            }
            let rhs = Operand::Register(rm, 0, 0);
            match (word >> 8) & 3 {
                0 => data(4, false, rd, rd, rhs),
                1 => data(10, true, 0, rd, rhs),
                2 => data(13, false, rd, 0, rhs),
                _ => None,
            }
        }
        0b01010 | 0b01011 => {
            let width = match (word >> 9) & 7 {
                4 => 4,
                5 => 2,
                6 => 1,
                _ => return None,
            };
            load(r(0), r(3), 0, Some((r(6), 0)), width)
        }
        0b01101 => load(r(0), r(3), ((word32 >> 6) & 31) * 4, None, 4),
        0b01111 => load(r(0), r(3), (word32 >> 6) & 31, None, 1),
        0b10001 => load(r(0), r(3), ((word32 >> 6) & 31) * 2, None, 2),
        0b10011 => load(r(8), 13, (word32 & 255) * 4, None, 4),
        // CBZ/CBNZ, which must leave the loop and may not sit in an IT block.
        0b10110 | 0b10111 if word & 0xf500 == 0xb100 => {
            let target =
                address + 4 + u64::from(((word32 >> 9) & 1) << 6 | ((word32 >> 3) & 31) << 1);
            (!in_it && target >= end).then_some(Op::Exit {
                register: r(0),
                nonzero: word & 0x800 != 0,
            })
        }
        _ => None,
    }
}
fn decode32(first: u16, second: u16) -> Option<Op> {
    let (first, second) = (u32::from(first), u32::from(second));
    let rn = (first & 15) as usize;
    let rd = ((second >> 8) & 15) as usize;
    let flags = first & 0x10 != 0;
    let op = (first >> 5) & 15;
    if first & 0xfa00 == 0xf000 && second & 0x8000 == 0 {
        let imm12 = ((first >> 10) & 1) << 11 | ((second >> 12) & 7) << 8 | (second & 255);
        return data(op, flags, rd, rn, expand(imm12)?);
    }
    if first & 0xfe00 == 0xea00 && second & 0x8000 == 0 {
        let rm = (second & 15) as usize;
        if rm >= 13 {
            return None;
        }
        let shift = ((second >> 12) & 7) << 2 | ((second >> 6) & 3);
        return data(
            op,
            flags,
            rd,
            rn,
            Operand::Register(rm, (second >> 4) & 3, shift),
        );
    }
    // Unsigned LDR/LDRH/LDRB without writeback; no literal, LDRT or hints.
    if first & 0xfe10 == 0xf810 && first & 0x100 == 0 {
        let width = match (first >> 5) & 3 {
            0 => 1,
            1 => 2,
            2 => 4,
            _ => return None,
        };
        let rt = (second >> 12) as usize;
        if rn == 15 || rt >= 13 {
            return None;
        }
        let (offset, index) = if first & 0x80 != 0 {
            (second & 0xfff, None)
        } else if second & 0xf00 == 0xc00 {
            (0u32.wrapping_sub(second & 255), None)
        } else if second & 0xfc0 == 0 && second & 15 < 13 {
            (0, Some(((second & 15) as usize, (second >> 4) & 3)))
        } else {
            return None;
        };
        return Some(Op::Load {
            dst: rt,
            base: rn,
            offset,
            index,
            width,
        });
    }
    None
}
/// Map a T32 data-processing operation onto its A32 opcode.
fn data(op: u32, flags: bool, rd: usize, rn: usize, rhs: Operand) -> Option<Op> {
    let (opcode, dst, lhs) = match op {
        0 if rd == 15 => (8, 0, rn),
        0 => (0, rd, rn),
        1 => (14, rd, rn),
        2 if rn == 15 => (13, rd, 0),
        2 => (12, rd, rn),
        3 if rn == 15 => (15, rd, 0),
        4 if rd == 15 => (9, 0, rn),
        4 => (1, rd, rn),
        8 if rd == 15 => (11, 0, rn),
        8 => (4, rd, rn),
        13 if rd == 15 => (10, 0, rn),
        13 => (2, rd, rn),
        _ => return None,
    };
    let compare = (8..=11).contains(&opcode);
    if (compare && !flags) || dst == 15 || (lhs == 15 && !matches!(opcode, 13 | 15)) {
        return None;
    }
    Some(Op::Data {
        opcode,
        flags,
        dst,
        lhs,
        rhs,
    })
}
/// ThumbExpandImm_C: a rotated form also produces the shifter carry.
pub(super) fn expand(imm12: u32) -> Option<Operand> {
    let imm8 = imm12 & 255;
    if imm12 >> 10 != 0 {
        let value = (0x80 | (imm12 & 0x7f)).rotate_right(imm12 >> 7);
        return Some(Operand::Immediate(value, Some(value >> 31 != 0)));
    }
    let value = match (imm12 >> 8) & 3 {
        0 => imm8,
        _ if imm8 == 0 => return None,
        1 => imm8 << 16 | imm8,
        2 => imm8 << 24 | imm8 << 8,
        _ => imm8 * 0x0101_0101,
    };
    Some(Operand::Immediate(value, None))
}
