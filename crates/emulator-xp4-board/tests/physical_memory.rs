use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

fn definition() -> Value {
    json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":0xffff0000u32,
        "memory_regions":[
            {"base":0xfff00000u32,"size":4096,"permissions":5},
            {"base":0xfff10000u32,"size":4096,"permissions":7}],
        "initial_registers":[[[{"Arm32":{"R":15}},0xfff00000u32],
            [{"Arm32":{"R":0}},0xfff10000u32]]]})
}

#[test]
fn reset_executes_read_only_rom_and_writes_separate_sram() {
    let definition: Definition = serde_json::from_value(definition()).unwrap();
    let session = Session::new(definition, None, vec![]).unwrap();
    let code = [0xe3a0102au32, 0xe5801000, 0xe320f003];
    let code: Vec<u8> = code.into_iter().flat_map(u32::to_le_bytes).collect();
    session.write(0xfff00000, &code).unwrap();
    session.run(0, 10).unwrap();
    let mut actual = [0; 4];
    session.read(0xfff10000, &mut actual).unwrap();
    assert_eq!(u32::from_le_bytes(actual), 42);
    assert!(session.read(0xfff20000, &mut actual).is_err());
    assert_eq!(session.read_virtual(0, 0xfff10000, 4).unwrap(), actual);
}

#[test]
fn physical_windows_and_reset_registers_refuse_ambiguous_extents() {
    let mut input = definition();
    input["memory_regions"][1]["base"] = json!(0xfff00000u32);
    let definition: Definition = serde_json::from_value(input).unwrap();
    assert!(Session::new(definition, None, vec![]).is_err());
    let mut input = self::definition();
    input["initial_registers"] = json!([[], []]);
    let definition: Definition = serde_json::from_value(input).unwrap();
    assert!(Session::new(definition, None, vec![]).is_err());
}

#[test]
fn cpu_cannot_write_read_only_rom() {
    let mut input = definition();
    input["initial_registers"][0][1][1] = json!(0xfff00000u32);
    let definition: Definition = serde_json::from_value(input).unwrap();
    let session = Session::new(definition, None, vec![]).unwrap();
    let code = [0xe3a0102au32, 0xe5801000, 0xe320f003];
    let code: Vec<u8> = code.into_iter().flat_map(u32::to_le_bytes).collect();
    session.write(0xfff00000, &code).unwrap();
    let _ = session.run(0, 10);
    let mut actual = [0; 4];
    session.read(0xfff00000, &mut actual).unwrap();
    assert_eq!(u32::from_le_bytes(actual), 0xe3a0102a);
    assert!(session.state(false).failure.is_some());
}

#[test]
fn guarded_high_rom_call_records_its_sparse_sram_store() {
    let mut input = definition();
    input["initial_registers"][0]
        .as_array_mut()
        .unwrap()
        .push(json!([{ "Arm32": { "R": 14 } }, 0x8000]));
    let definition: Definition = serde_json::from_value(input).unwrap();
    let session = Session::new(definition, None, vec![]).unwrap();
    let code = [0xe3a0102au32, 0xe5801000, 0xe12fff1e];
    let code: Vec<u8> = code.into_iter().flat_map(u32::to_le_bytes).collect();
    session.write(0xfff00000, &code).unwrap();
    session
        .driver_guard_begin(
            vec![(0xfff00000, 0xfff0000c)],
            vec![(0xfff10000, 0xfff10004)],
            vec![(0xfff10000, 0xfff10004)],
            0x8000,
            8,
            0,
            None,
            true,
        )
        .unwrap();
    session.run(0, 16).unwrap();
    let guard = session.driver_guard_state().unwrap();
    assert!(guard.returned);
    assert!(guard.failure.is_none());
    assert_eq!(guard.stores, vec![(0, 0xfff00004, 0xfff10000, 4, 42)]);
    session.driver_guard_end().unwrap();
}

#[test]
fn guarded_return_sentinel_requires_executable_backing() {
    for permissions in [3, 5] {
        let mut input = definition();
        input["memory_regions"][1]["permissions"] = json!(permissions);
        input["initial_registers"][0]
            .as_array_mut()
            .unwrap()
            .push(json!([{ "Arm32": { "R": 14 } }, 0xfff10000u32]));
        let session = Session::new(serde_json::from_value(input).unwrap(), None, vec![]).unwrap();
        session
            .write(0xfff00000, &0xe12fff1eu32.to_le_bytes())
            .unwrap();
        let admitted = session.driver_guard_begin(
            vec![(0xfff00000, 0xfff00004)],
            vec![],
            vec![],
            0xfff10000,
            8,
            0,
            None,
            true,
        );
        if permissions == 3 {
            assert!(
                admitted.is_err(),
                "non-executable sentinel must be rejected"
            );
            assert!(session.driver_guard_state().is_none());
            assert!(session.state(false).failure.is_none());
        } else {
            admitted.unwrap();
            session.run(0, 16).unwrap();
            let guard = session.driver_guard_state().unwrap();
            assert!(guard.returned);
            assert!(guard.failure.is_none());
            assert!(session.state(false).failure.is_none());
        }
    }
}

fn guarded_session(code: &[u32]) -> Session {
    let mut input = definition();
    input["initial_registers"][0]
        .as_array_mut()
        .unwrap()
        .push(json!([{ "Arm32": { "R": 14 } }, 0x8000]));
    let session = Session::new(serde_json::from_value(input).unwrap(), None, vec![]).unwrap();
    let code: Vec<u8> = code.iter().flat_map(|word| word.to_le_bytes()).collect();
    session.write(0xfff00000, &code).unwrap();
    session
}

fn store_loop(session: &Session, record_stores: bool) {
    session
        .driver_guard_begin(
            vec![(0xfff00000, 0xfff00014)],
            vec![],
            vec![(0xfff10000, 0xfff10004)],
            0x8000,
            8,
            0,
            None,
            record_stores,
        )
        .unwrap();
    let _ = session.run(0, 200);
}

#[test]
fn store_bound_applies_only_to_recorded_stores() {
    // Twenty stores to one word, then return.
    let code = [
        0xe3a01014, // mov r1, #20
        0xe5801000, // str r1, [r0]
        0xe2511001, // subs r1, r1, #1
        0x1afffffc, // bne loop
        0xe12fff1e, // bx lr
    ];
    let unrecorded = guarded_session(&code);
    store_loop(&unrecorded, false);
    let guard = unrecorded.driver_guard_state().unwrap();
    assert!(guard.returned);
    assert!(guard.failure.is_none());
    assert!(guard.stores.is_empty());
    let mut word = [0; 4];
    unrecorded.read(0xfff10000, &mut word).unwrap();
    assert_eq!(u32::from_le_bytes(word), 1);

    let recorded = guarded_session(&code);
    store_loop(&recorded, true);
    let guard = recorded.driver_guard_state().unwrap();
    assert!(!guard.returned);
    assert!(
        guard
            .failure
            .as_deref()
            .unwrap()
            .contains("store record bound")
    );
    assert_eq!(guard.stores.len(), 8);
}

#[test]
fn unrecorded_stores_still_obey_the_writable_ranges() {
    // The second store lands outside the writable word.
    let code = [
        0xe5801000, // str r1, [r0]
        0xe5801004, // str r1, [r0, #4]
        0xe12fff1e, // bx lr
    ];
    let session = guarded_session(&code);
    store_loop(&session, false);
    let guard = session.driver_guard_state().unwrap();
    assert!(!guard.returned);
    assert!(guard.failure.as_deref().unwrap().contains("rejected write"));
    assert!(guard.stores.is_empty());
}

fn byte_arithmetic_results(a: u32, b: u32, guarded: bool) -> [u32; 6] {
    // r3 = a, r4 = b. Results: add, subtract, add with Rn == Rm, subtract
    // with Rn == Rm, then add with Rd == Rn and subtract with Rd == Rm.
    let code = [
        0xe5903004, // ldr r3, [r0, #4]
        0xe5904008, // ldr r4, [r0, #8]
        0xe6632f94, // uqadd8 r2, r3, r4
        0xe5802010, // str r2, [r0, #0x10]
        0xe6632ff4, // uqsub8 r2, r3, r4
        0xe5802014, // str r2, [r0, #0x14]
        0xe6635f93, // uqadd8 r5, r3, r3
        0xe5805018, // str r5, [r0, #0x18]
        0xe6636ff3, // uqsub8 r6, r3, r3
        0xe580601c, // str r6, [r0, #0x1c]
        0xe5907004, // ldr r7, [r0, #4]
        0xe6677f94, // uqadd8 r7, r7, r4
        0xe5807020, // str r7, [r0, #0x20]
        0xe5908008, // ldr r8, [r0, #8]
        0xe5909004, // ldr r9, [r0, #4]
        0xe6698ff8, // uqsub8 r8, r9, r8
        0xe5808024, // str r8, [r0, #0x24]
        0xe12fff1e, // bx lr
    ];
    let session = guarded_session(&code);
    session.write(0xfff10004, &a.to_le_bytes()).unwrap();
    session.write(0xfff10008, &b.to_le_bytes()).unwrap();
    if guarded {
        session
            .driver_guard_begin(
                vec![(0xfff00000, 0xfff00000 + 4 * code.len() as u64)],
                vec![(0xfff10000, 0xfff10100)],
                vec![(0xfff10000, 0xfff10100)],
                0x8000,
                64,
                0,
                None,
                false,
            )
            .unwrap();
    }
    session.run(0, 64).unwrap();
    if guarded {
        let guard = session.driver_guard_state().unwrap();
        assert!(guard.failure.is_none(), "{:?}", guard.failure);
        assert!(guard.returned);
    }
    assert!(session.state(false).failure.is_none());
    let mut out = [0; 6];
    for (slot, offset) in [0x10, 0x14, 0x18, 0x1c, 0x20, 0x24].into_iter().enumerate() {
        let mut word = [0; 4];
        session.read(0xfff10000 + offset, &mut word).unwrap();
        out[slot] = u32::from_le_bytes(word);
    }
    out
}

#[test]
fn saturating_byte_arithmetic_matches_known_vectors_through_the_guard() {
    // Bytes (high to low): ff+01 saturates, 01-fe floors at zero, 80+20 and
    // f0+00 stay exact.
    let vectors = [
        (
            0x01ff80f0,
            0xfe012000,
            [0xffffa0f0, 0x00fe60f0, 0x02ffffff, 0],
        ),
        // Exact 0xff sums and 0x00 differences, the edges of the range.
        (
            0x7f80ff00,
            0x817f0100,
            [0xffffff00, 0x0001fe00, 0xfeffff00, 0],
        ),
    ];
    for (a, b, expected) in vectors {
        for guarded in [false, true] {
            let got = byte_arithmetic_results(a, b, guarded);
            assert_eq!(got[..4], expected, "a={a:#x} b={b:#x} guarded={guarded}");
            // Rd == Rn and Rd == Rm give the same results as distinct registers.
            assert_eq!(got[4], got[0]);
            assert_eq!(got[5], got[1]);
        }
    }
}

#[test]
fn saturating_byte_arithmetic_leaves_the_ge_flags_alone() {
    let code = [
        0xe3a07805, // mov r7, #0x50000 (GE = 0b0101)
        0xe124f007, // msr CPSR_s, r7
        0xe3e03000, // mvn r3, #0
        0xe3a04001, // mov r4, #1
        0xe6632f94, // uqadd8 r2, r3, r4
        0xe10f8000, // mrs r8, CPSR
        0xe5808000, // str r8, [r0]
        0xe6632ff4, // uqsub8 r2, r3, r4
        0xe10f9000, // mrs r9, CPSR
        0xe5809004, // str r9, [r0, #4]
        0xe12fff1e, // bx lr
    ];
    let session = guarded_session(&code);
    session.run(0, 32).unwrap();
    assert!(session.state(false).failure.is_none());
    let mut word = [0; 4];
    for offset in [0, 4] {
        session.read(0xfff10000 + offset, &mut word).unwrap();
        assert_eq!(u32::from_le_bytes(word) >> 16 & 15, 5);
    }
}

#[test]
fn preload_hints_run_as_no_ops_through_the_guard() {
    let code = [
        0xe3a0102a, // mov r1, #42
        0xf5d0f008, // pld [r0, #8]
        0xf590f008, // pldw [r0, #8]
        0xf7d0f001, // pld [r0, r1]
        0xe5801000, // str r1, [r0]
        0xe12fff1e, // bx lr
    ];
    let session = guarded_session(&code);
    session
        .driver_guard_begin(
            vec![(0xfff00000, 0xfff00018)],
            vec![],
            vec![(0xfff10000, 0xfff10004)],
            0x8000,
            8,
            0,
            None,
            true,
        )
        .unwrap();
    session.run(0, 32).unwrap();
    let guard = session.driver_guard_state().unwrap();
    assert!(guard.failure.is_none(), "{:?}", guard.failure);
    assert!(guard.returned);
    assert_eq!(guard.stores, vec![(0, 0xfff00010, 0xfff10000, 4, 42)]);
}
