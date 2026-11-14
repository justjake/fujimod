use fujimod_emulator_cpu::arm;
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

const CODE: usize = 0x1000;
const DATA: usize = 0x2000;

fn definition(ram: u32) -> Value {
    json!({"ram_size":ram,"cores":1,"discovery":false,"cbar":0xffff0000u32})
}

/// Counts in memory forever: LDR r1,[r0]; ADD r1,r1,1; STR r1,[r0]; B loop.
fn counter(ram: u32) -> Session {
    let definition: Definition = serde_json::from_value(definition(ram)).unwrap();
    let session = Session::new(definition, None, vec![]).unwrap();
    let code = [0xe590_1000u32, 0xe281_1001, 0xe580_1000, 0xeaff_fffb];
    let code: Vec<u8> = code.into_iter().flat_map(u32::to_le_bytes).collect();
    session.write(CODE, &code).unwrap();
    session.write_register(0, arm::PC, CODE as u64).unwrap();
    session.write_register(0, arm::CORE[0], DATA as u64).unwrap();
    session
}
fn count(session: &Session) -> u32 {
    let mut bytes = [0; 4];
    session.read(DATA, &mut bytes).unwrap();
    u32::from_le_bytes(bytes)
}
fn pc(session: &Session) -> u64 {
    session.read_register(0, arm::PC).unwrap()
}
fn r1(session: &Session) -> u64 {
    session.read_register(0, arm::CORE[1]).unwrap()
}

#[test]
fn restore_returns_to_the_checkpoint_and_the_run_repeats_exactly() {
    let session = counter(65536);
    session.run(0, 10).unwrap();
    let saved = session.checkpoint().unwrap();
    let (value, instructions, ticks, program, reg1) = (
        count(&session),
        session.state(false).instructions,
        session.state(false).virtual_ticks,
        pc(&session),
        r1(&session),
    );
    assert_eq!((saved.instructions(), saved.ticks()), (instructions, ticks));
    assert!(saved.exact(), "no devices in this board: {:?}", saved.observational_devices());
    session.run(0, 200).unwrap();
    let (later_value, later_instructions, later_pc, later_r1) = (
        count(&session),
        session.state(false).instructions,
        pc(&session),
        r1(&session),
    );
    assert!(later_value > value && later_instructions == instructions + 200);

    session.restore(&saved).unwrap();
    assert_eq!(count(&session), value);
    assert_eq!(session.state(false).instructions, instructions);
    assert_eq!(session.state(false).virtual_ticks, ticks);
    assert_eq!((pc(&session), r1(&session)), (program, reg1));

    // The same budget from the checkpoint gives the same final state.
    session.run(0, 200).unwrap();
    assert_eq!(count(&session), later_value);
    assert_eq!(session.state(false).instructions, later_instructions);
    assert_eq!((pc(&session), r1(&session)), (later_pc, later_r1));

    // A checkpoint can be used again, and a restore undoes a host write.
    session.restore(&saved).unwrap();
    session.write(DATA, &1000u32.to_le_bytes()).unwrap();
    session.restore(&saved).unwrap();
    assert_eq!(count(&session), value);
}

#[test]
fn restore_discards_translated_code_the_guest_rewrote_after_the_checkpoint() {
    let definition: Definition = serde_json::from_value(definition(65536)).unwrap();
    let session = Session::new(definition, None, vec![]).unwrap();
    // 0x1000 CMP r7,0; 0x1004 STRNE r3,[r4]; 0x1008 MCRNE ICIALLU;
    // 0x100c ADD r1,r1,1; 0x1010 B 0x1000. With r7 set the guest rewrites the
    // ADD at [r4] into ADD r1,r1,5 and invalidates the instruction cache.
    let code = [0xe357_0000u32, 0x1584_3000, 0x1e07_0f15, 0xe281_1001, 0xeaff_fffa];
    let code: Vec<u8> = code.into_iter().flat_map(u32::to_le_bytes).collect();
    session.write(0x1000, &code).unwrap();
    for (register, value) in [(arm::PC, 0x1000), (arm::CORE[3], 0xe281_1005), (arm::CORE[4], 0x100c)] {
        session.write_register(0, register, value).unwrap();
    }
    session.run(0, 10).unwrap();
    assert_eq!(r1(&session), 2);
    let saved = session.checkpoint().unwrap();
    session.write_register(0, arm::CORE[7], 1).unwrap();
    for _ in 0..4 {
        session.run(0, 15).unwrap();
    }
    let patched = r1(&session);
    assert!(patched >= 2 + 5, "the guest patch must take effect: {patched}");
    session.restore(&saved).unwrap();
    assert_eq!((r1(&session), session.read_register(0, arm::CORE[7]).unwrap()), (2, 0));
    session.run(0, 15).unwrap();
    // The original ADD ..,1 is back and no stale translation adds five.
    assert_eq!(r1(&session), 2 + 3, "restored code must not keep the guest patch");
}

#[test]
fn a_checkpoint_does_not_fit_another_board_and_active_watches_refuse() {
    let session = counter(65536);
    session.run(0, 5).unwrap();
    let saved = session.checkpoint().unwrap();
    let other = counter(131072);
    assert!(other.restore(&saved).is_err());
    assert!(other.state(false).failure.is_none());
    session.memory_watch_begin(vec![(0x100, 0x104)], 8).unwrap();
    assert!(session.checkpoint().is_err());
    assert!(session.restore(&saved).is_err());
    session.memory_watch_end().unwrap();
    session.restore(&saved).unwrap();
}
