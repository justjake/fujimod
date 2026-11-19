//! Labels, call returns and PC observers go through a passive-observation
//! filter keyed by instruction-address bits 2..18: addresses that share a key
//! must still be told apart, and every observed address must still be seen.
use fujimod_emulator_cpu::arm;
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};
use std::{cell::Cell, rc::Rc};

const ENTRY: u32 = 0x1000;
const CALLEE: u32 = 0x1100;
/// Same filter key as `CALLEE` (bits 2..18 equal), never executed.
const TWIN: u32 = CALLEE + 0x4_0000;
const RETURN: u32 = ENTRY + 8;
const RESULT: u32 = 0x2000;
const CBAR: u32 = 0x2000_0000;

fn session() -> Session {
    let input = json!({"ram_size": 65536, "cores": 1, "discovery": false, "cbar": CBAR,
        "labels": [CALLEE, TWIN], "initial_registers": [[["Pc", ENTRY]]]});
    let definition: Definition = serde_json::from_value(input).unwrap();
    let session = Session::new(definition, None, vec![]).unwrap();
    let program: [(u32, u32); 6] = [
        (ENTRY, 0xe3a0_0005),      // MOV r0, #5
        (ENTRY + 4, 0xeb00_003d),  // BL CALLEE
        (RETURN, 0xe583_0000),     // STR r0, [r3]
        (ENTRY + 12, 0xeaff_fffe), // B .
        (CALLEE, 0xe280_0001),     // ADD r0, r0, #1
        (CALLEE + 4, 0xe12f_ff1e), // BX lr
    ];
    for (address, word) in program {
        session
            .write(address as usize, &word.to_le_bytes())
            .unwrap();
    }
    session
        .write_register(0, arm::CORE[3], RESULT.into())
        .unwrap();
    session.write_register(0, arm::CORE[13], 0x8000).unwrap();
    session
}

fn history(session: &Session) -> Value {
    serde_json::to_value(session.state(true)).unwrap()["history"].clone()
}

#[test]
fn a_labeled_call_and_its_return_are_observed_and_a_key_twin_is_not() {
    let session = session();
    session.run_board(64, 16).unwrap();
    assert!(session.state(false).failure.is_none());
    let mut word = [0; 4];
    session.read(RESULT as usize, &mut word).unwrap();
    assert_eq!(u32::from_le_bytes(word), 6);
    let history = history(&session);
    let milestones = history["milestones"].as_array().unwrap();
    assert_eq!(milestones.len(), 1, "{milestones:?}");
    assert_eq!(milestones[0][1], CALLEE);
    let calls = history["calls"].as_array().unwrap();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0]["pc"], CALLEE);
    assert_eq!(calls[0]["lr"], RETURN);
    assert_eq!(calls[0]["args"][0], 5);
    assert_eq!(calls[0]["result"], 6, "the return was observed");
}

#[test]
fn a_pc_observer_sees_its_address_and_not_its_key_twin() {
    let session = Rc::new(session());
    session
        .instruction_points(0, vec![u64::from(RETURN), u64::from(TWIN)])
        .unwrap();
    let hits = Rc::new(Cell::new(Vec::new()));
    let seen = hits.clone();
    session
        .set_instruction_observer(Some(Rc::new(move |_, address| {
            let mut all = seen.take();
            all.push(address);
            seen.set(all);
            Ok(())
        })))
        .unwrap();
    session.run_board(64, 16).unwrap();
    assert!(session.state(false).failure.is_none());
    assert_eq!(hits.take(), vec![u64::from(RETURN)]);
}
