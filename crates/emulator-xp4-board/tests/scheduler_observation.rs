//! Compact observations must preserve scheduler semantics without exporting GPIO/history.
use fujimod_emulator_cpu::arm;
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

fn session() -> Session {
    let definition: Definition = serde_json::from_value(json!({
        "ram_size": 65536, "cores": 4, "discovery": false,
        "cbar": 0x2000_0000u32, "gpio": vec![0u16; 16],
        "control_requests": [[0x1000_0000u32, 20]],
    }))
    .unwrap();
    Session::new(definition, None, vec![]).unwrap()
}

fn check(session: &Session) {
    let before = serde_json::to_value(session.state(true)).unwrap();
    let devices = session.device_state_json();
    let records = serde_json::to_value(session.device_records()).unwrap();
    let interrupts = serde_json::to_value(session.interrupt_state()).unwrap();
    let mut expected = serde_json::to_value(session.state(false)).unwrap();
    let object = expected.as_object_mut().unwrap();
    assert!(object.remove("gpio").unwrap().is_array());
    object.remove("history");
    for _ in 0..10 {
        let compact = serde_json::to_value(session.scheduler_state()).unwrap();
        assert_eq!(compact, expected);
        assert_eq!(session.failure(), session.state(false).failure);
    }
    assert_eq!(serde_json::to_value(session.state(true)).unwrap(), before);
    assert_eq!(session.device_state_json(), devices);
    assert_eq!(
        serde_json::to_value(session.device_records()).unwrap(),
        records
    );
    assert_eq!(
        serde_json::to_value(session.interrupt_state()).unwrap(),
        interrupts
    );
}

#[test]
fn compact_state_matches_full_projection_at_pending_deadline_and_sticky_failure() {
    let session = session();
    check(&session);
    // STR r1,[r0]; WFI; LDR r3,[r2]. The last access has no physical backing.
    let code: Vec<u8> = [0xe580_1000u32, 0xe320_f003, 0xe592_3000]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    session.write(0x1000, &code).unwrap();
    session.write_register(0, arm::PC, 0x1000).unwrap();
    session
        .write_register(0, arm::CORE[0], 0x1000_0000)
        .unwrap();
    session.write_register(0, arm::CORE[1], 2).unwrap();
    session.write_register(0, arm::CORE[2], 0x20000).unwrap();
    session.run(0, 1).unwrap();
    assert!(session.scheduler_state().next_device_tick.is_some());
    check(&session);
    session.run(0, 1).unwrap();
    assert!(session.scheduler_state().waiting[0]);
    check(&session);
    // A separate core faults while the request deadline is still pending.
    session.write_register(1, arm::PC, 0x1008).unwrap();
    session.write_register(1, arm::CORE[2], 0x20000).unwrap();
    let failure = session.run(1, 1).unwrap_err();
    assert_eq!(session.failure().as_deref(), Some(failure.as_str()));
    check(&session);
    let before: Value = serde_json::to_value(session.state(true)).unwrap();
    assert!(!session.idle());
    assert_eq!(serde_json::to_value(session.state(true)).unwrap(), before);
}
