use super::*;

fn bytes(halfwords: &[u16]) -> Vec<u8> {
    halfwords.iter().flat_map(|h| h.to_le_bytes()).collect()
}
/// Decode Thumb code at 0x1000 whose final instruction is the loop branch.
fn t32(code: &[u16]) -> Option<Proof> {
    let bytes = bytes(code);
    let last = if code.len() >= 2 && code[code.len() - 2] >> 11 >= 0b11101 {
        bytes.len() - 4
    } else {
        bytes.len() - 2
    };
    let branch = 0x1000 + last as u64;
    let mut word = [0; 4];
    let tail = &bytes[last..];
    word[..tail.len()].copy_from_slice(tail);
    let word = u32::from_le_bytes(word);
    assert_eq!(
        loop_start(Isa::T32, branch, word),
        Some(0x1000),
        "{code:04x?}"
    );
    Proof::decode(Isa::T32, 0x1000, branch, &bytes)
}
fn state(cpsr: u32) -> State {
    State {
        registers: [0; 15],
        cpsr,
    }
}

#[test]
fn a32_stores_system_instructions_and_conditional_bodies_never_prove() {
    for word in [
        0xe5821000, 0xee110f10, 0xe320f003, 0xe1921f9f, 0xe592f000, 0xe5b21004, 0x02800001,
        0xe8b20003, 0xe1d210b0,
    ] {
        assert!(a32::decode(word).is_none(), "{word:08x}");
    }
    assert!(a32::target(0x1004, 0xebfffffd).is_none());
}
#[test]
fn t32_branch_encodings_find_their_loop_head() {
    // B<cond> T1, B T2, B<cond>.W T3 and B.W T4, each back to 0x1000.
    for (address, word, size) in [
        (0x1004, 0xd5fc, 2),
        (0x1004, 0xe7fc, 2),
        (0x1004, 0xaffc_f47f, 4),
        (0x1004, 0xbffc_f7ff, 4),
    ] {
        assert_eq!(
            loop_start(Isa::T32, address, word),
            Some(0x1000),
            "{word:08x}"
        );
        assert_eq!(branch_size(Isa::T32, address, word), Some(size));
    }
    // Forward branches, BL, CBZ, and loops longer than the span never start.
    for (address, word) in [
        (0x1004, 0xd500),
        (0x1004, 0xf800_f7ff),
        (0x1004, 0xb901),
        (0x1100, 0xe77e),
    ] {
        assert_eq!(loop_start(Isa::T32, address, word), None, "{word:08x}");
    }
}
#[test]
fn t32_subset_decodes_each_supported_shape() {
    for code in [
        vec![0x6811, 0x07c9, 0xd5fc],                 // LDR; LSLS #31; BPL
        vec![0xf8d2, 0x1000, 0xf011, 0x0f01, 0xd0fa], // LDR.W; TST.W; BEQ
        vec![0x5851, 0x2900, 0xd0fc],                 // LDR reg; CMP; BEQ
        vec![0xf852, 0x1023, 0x2900, 0xd0fb],         // LDR.W reg LSL #2; CMP; BEQ
        vec![0xf812, 0x1c04, 0x2900, 0xd0fb],         // LDRB.W [r2,#-4]; CMP; BEQ
        vec![0x8811, 0x7811, 0x9900, 0xe7fb],         // LDRH; LDRB; LDR SP; B
        vec![0x6811, 0xb901, 0xe7fc],                 // LDR; CBNZ out; B
        vec![0x6811, 0x2900, 0xbf0c, 0x2301, 0x2302, 0x2b02, 0xd0f8], // ITE EQ
        vec![0x3801, 0xd1fd],                         // SUBS; BNE
        vec![0xf1b0, 0x0001, 0xf47f, 0xaffc],         // SUBS.W; BNE.W
        vec![0xea5f, 0x0141, 0x4019, 0xe7fb],         // MOVS.W LSL; ANDS; B
        vec![0xe7fe],                                 // B .
    ] {
        assert!(t32(&code).is_some(), "{code:04x?}");
    }
}
#[test]
fn t32_stores_system_writeback_and_it_misuse_never_prove() {
    for code in [
        vec![0x6811, 0x6051, 0xe7fc],         // STR
        vec![0x6811, 0xf8c2, 0x1004, 0xe7fb], // STR.W
        vec![0x6811, 0x4349, 0xe7fc],         // MULS
        vec![0x4800, 0xe7fd],                 // LDR literal
        vec![0xf852, 0x1f04, 0xe7fc],         // LDR.W pre-index writeback
        vec![0xf992, 0x1000, 0xe7fc],         // LDRSB.W
        vec![0xb500, 0xe7fd],                 // PUSH
        vec![0xbf00, 0xe7fd],                 // NOP hint
        vec![0x6811, 0xbf08, 0x6051, 0xe7fb], // IT EQ; STREQ
        vec![0x6811, 0xbf08, 0xb901, 0xe7fb], // CBNZ inside IT
        vec![0x6811, 0xbf08, 0xe7fc],         // IT block covering the branch
        vec![0x2900, 0xb101, 0x2900, 0xe7fb], // CBZ to a target inside the loop
        vec![0x6811, 0xbf28, 0x0000, 0xe7fb], // MOVS inside IT (unpredictable)
        vec![0x4687, 0xe7fd],                 // MOV PC, R0
    ] {
        assert!(t32(&code).is_none(), "{code:04x?}");
    }
}
#[test]
fn t32_it_predicates_and_flag_rules_follow_the_architecture() {
    // CMP; ITE EQ; MOVEQ r3,#1; MOVNE r3,#2; CMP r3,#2; BEQ.
    let proof = t32(&[0x6811, 0x2900, 0xbf0c, 0x2301, 0x2302, 0x2b02, 0xd0f8]).unwrap();
    assert_eq!(proof.length(), 7);
    let mut entry = state(0x6000_0000);
    entry.registers[1] = 1;
    entry.registers[3] = 2;
    let read = |value: u32| {
        move |address: u32, width: usize| (address == 0 && width == 4).then_some(value)
    };
    // A nonzero flag selects MOVNE, CMP sets Z and the branch repeats: a fixed point.
    assert_eq!(
        proof.advance(entry, 50, read(1)),
        Ok(Advance {
            state: entry,
            iterations: 50,
            counted: false
        })
    );
    // A zero flag selects MOVEQ, so the loop leaves after this iteration.
    assert_eq!(
        proof.advance(entry, 50, read(0)),
        Err(AdvanceFailure::ConditionNotTaken)
    );
    // Inside IT, ADDS does not set flags: it changes R0 every iteration.
    let proof = t32(&[0xbf08, 0x3001, 0xd0fc]).unwrap();
    let advanced = proof.advance(state(0x4000_0000), 10, |_, _| None);
    assert_eq!(advanced, Err(AdvanceFailure::IterationModelUnsupported));
    // A polled value that changed but still loops is not a fixed point yet;
    // that failure is not a property of the loop's shape.
    let proof = t32(&[0xf8d2, 0x1000, 0xf011, 0x0f01, 0xd0fa]).unwrap();
    let mut entry = state(0x4000_0000);
    entry.registers[1] = 2;
    assert_eq!(
        proof.advance(entry, 10, read(4)),
        Err(AdvanceFailure::InputsChanged)
    );
    // CBNZ leaves on a nonzero register.
    let proof = t32(&[0x6811, 0xb901, 0xe7fc]).unwrap();
    assert_eq!(
        proof.advance(state(0), 10, read(1)),
        Err(AdvanceFailure::ConditionNotTaken)
    );
    assert_eq!(
        proof.advance(state(0), 10, |_, _| None),
        Err(AdvanceFailure::ReadUnavailable)
    );
}
#[test]
fn t32_modified_immediates_expand_with_their_carry() {
    for (imm12, value, carry) in [
        (0x0ab, 0xab, None),
        (0x1ab, 0x00ab_00ab, None),
        (0x2ab, 0xab00_ab00, None),
        (0x3ab, 0xabab_abab, None),
        (0x4ff, 0x7f80_0000, Some(false)),
        (0x47f, 0xff00_0000, Some(true)),
    ] {
        let Some(Operand::Immediate(v, c)) = t32::expand(imm12) else {
            panic!("{imm12:03x}");
        };
        assert_eq!((v, c), (value, carry), "{imm12:03x}");
    }
    assert!(t32::expand(0x100).is_none());
}
