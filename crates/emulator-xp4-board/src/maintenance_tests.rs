use super::*;
use crate::{Definition, Session};
use fujimod_emulator_cpu::Model;
use fujimod_emulator_cpu_unicorn::Library;
use serde_json::json;

const ENTRY: u64 = 0x1000;
const ALL_IS: u32 = 0xee08_0f13;

fn actlr() -> arm::Coprocessor {
    arm::Coprocessor {
        cp: 15, wide: false, secure: 1, crn: 1, crm: 0, opc1: 0, opc2: 1,
    }
}

fn session() -> Session {
    let input = json!({
        "ram_size": 65536, "cores": 3, "discovery": false,
        "cbar": 0x2000_0000u32,
        "initial_registers": (0..3).map(|_| json!([["Pc", ENTRY]])).collect::<Vec<_>>(),
    });
    let definition: Definition = serde_json::from_value(input).unwrap();
    let session = Session::new(definition, None, vec![]).unwrap();
    session.write(ENTRY as usize, &ALL_IS.to_le_bytes()).unwrap();
    for core in 0..3 {
        session.coprocessor(core, arm::Coprocessor {
            cp: 15, wide: false, secure: 1, crn: 1, crm: 1, opc1: 0, opc2: 0,
        }, Some(0)).unwrap();
        session.write_register(core, arm::CPSR, 0xd3).unwrap();
        session.coprocessor(core, actlr(), Some(0x6040)).unwrap();
        assert_eq!(session.coprocessor(core, actlr(), None).unwrap(), 0x6040);
    }
    session
}

fn foreign_owner() -> (Core, native::Owner) {
    let cpu = Library::linked().unwrap().open(Model::CortexA7, 1 << 20).unwrap();
    // This fresh engine remains stopped and private for the complete test.
    let owner = unsafe { cpu.attach_translation_owner() }.unwrap();
    assert_eq!(cpu.translation_maintenance_status().unwrap().pending, 0);
    (cpu, owner)
}

fn assert_failed_session_is_frozen(session: &Session, error: &str) {
    let maintenance = session.translation_maintenance_state().unwrap();
    let state = serde_json::to_value(session.state(true)).unwrap();
    let devices = session.device_state_json();
    let records = session.device_records();
    let registers: Vec<_> = (0..3).map(|core| {
        arm::CORE.iter().chain([arm::CPSR].iter())
            .map(|&register| session.read_register(core, register).unwrap())
            .collect::<Vec<_>>()
    }).collect();
    let controls: Vec<_> = (0..3).map(|core|
        session.coprocessor(core, actlr(), None).unwrap()).collect();
    let mut ram = vec![0; 65536];
    session.read(0, &mut ram).unwrap();

    for core in 0..3 {
        assert_eq!(session.run(core, 1).unwrap_err(), error);
        assert_eq!(session.write_register(core, arm::CORE[0], 99).unwrap_err(), error);
        assert_eq!(session.coprocessor(core, actlr(), Some(0x6000)).unwrap_err(), error);
        assert_eq!(session.instruction_points(core, vec![ENTRY]).unwrap_err(), error);
    }
    assert_eq!(session.run_board(8, 1).unwrap_err(), error);
    assert_eq!(session.run_board(0, 1).unwrap_err(), error);
    assert!(!session.idle());
    assert_eq!(session.write(0x2000, &[1, 2, 3, 4]).unwrap_err(), error);
    assert_eq!(session.set_instruction_observer(None).unwrap_err(), error);
    assert_eq!(session.driver_guard_begin(
        vec![(ENTRY, ENTRY + 4)], vec![], vec![], 0x2000, 8, 0, None,
    ).unwrap_err(), error);
    assert_eq!(session.memory_watch_begin(vec![(0, 65536)], 8, false).unwrap_err(), error);
    assert_eq!(session.memory_watch_end().unwrap_err(), error);
    assert_eq!(session.function_watch_begin(vec![ENTRY], 1, 8, 8, None, false)
        .unwrap_err(), error);
    assert_eq!(session.function_watch_end().unwrap_err(), error);
    assert_eq!(session.set_gpio_input(0, 0, true).unwrap_err(), error);
    assert_eq!(session.set_adc_input(0, 0).unwrap_err(), error);
    assert_eq!(session.set_usb_connected(true).unwrap_err(), error);

    assert_eq!(session.translation_maintenance_state().unwrap(), maintenance);
    assert_eq!(serde_json::to_value(session.state(true)).unwrap(), state);
    assert_eq!(session.device_state_json(), devices);
    assert_eq!(session.device_records(), records);
    let mut after = vec![0; 65536];
    session.read(0, &mut after).unwrap();
    assert_eq!(after, ram);
    for core in 0..3 {
        for (&register, expected) in arm::CORE.iter().chain([arm::CPSR].iter())
            .zip(&registers[core])
        {
            assert_eq!(session.read_register(core, register).unwrap(), *expected);
        }
        assert_eq!(session.coprocessor(core, actlr(), None).unwrap(), controls[core]);
    }
}

fn assert_pending(session: &Session, error: &str, applied: u8, delivered: u64) {
    let snapshot = session.translation_maintenance_state().unwrap();
    assert_eq!(snapshot.completed, 0);
    assert_eq!(snapshot.delivered_peers, delivered);
    assert_eq!(snapshot.dropped, 0);
    assert!(snapshot.records.is_empty());
    assert_eq!(snapshot.failure.as_deref(), Some(error));
    assert!(snapshot.last_raw_error.is_none());
    let pending = snapshot.pending.as_ref().unwrap();
    assert_eq!(pending.source_core, 0);
    assert_eq!(pending.source_identity, snapshot.identities[0]);
    assert_eq!(pending.event.sequence, 1);
    assert_eq!(pending.event.selector, native::TLBIALLIS);
    assert_eq!(pending.event.actlr, 0x6040);
    assert_eq!(pending.peer_actlr, [0x6040; 3]);
    assert_eq!((pending.recipients, pending.applied, pending.excluded), (0b110, applied, 0));
    assert!(!pending.acknowledged);
    assert_eq!(pending.error.as_deref(), Some(error));
    assert!(pending.raw_error.is_none());
    assert_eq!((snapshot.native[0].pending, snapshot.native[0].poison), (1, 0));
    assert_eq!(snapshot.native[0].front, pending.event);
    assert_eq!(snapshot.native[0].next_sequence, 2);
    assert!(snapshot.native[1..].iter().all(|status| status.pending == 0 && status.poison == 0));
    assert_eq!(session.state(false).failure.as_deref(), Some(error));
    assert_eq!(session.instructions(), [1, 0, 0]);
    assert_eq!(session.now(), 1);
    assert_eq!(session.read_register(0, arm::PC).unwrap(), ENTRY + 4);
}

#[test]
fn later_peer_capability_failure_preserves_partial_delivery_without_ack_or_retry() {
    let mut session = session();
    let (foreign, owner) = foreign_owner();
    assert_ne!(foreign.identity(), session.inner.cpus[2].identity());
    let before = session.translation_maintenance_state().unwrap();
    session.inner.maintenance.owners[2] = owner;
    assert_eq!(session.translation_maintenance_state().unwrap(), before);

    let error = session.run(0, 1).unwrap_err();
    assert_eq!(error,
        "Translation delivery to core 2: Translation owner belongs to a different processor");
    assert_pending(&session, &error, 0b010, 1);
    assert_failed_session_is_frozen(&session, &error);
    assert_eq!(foreign.translation_maintenance_status().unwrap().pending, 0);
}

#[test]
fn source_capability_failure_preserves_all_peer_flushes_without_ack_or_retry() {
    let mut session = session();
    let (foreign, owner) = foreign_owner();
    assert_ne!(foreign.identity(), session.inner.cpus[0].identity());
    let before = session.translation_maintenance_state().unwrap();
    session.inner.maintenance.owners[0] = owner;
    assert_eq!(session.translation_maintenance_state().unwrap(), before);

    let error = session.run(0, 1).unwrap_err();
    assert_eq!(error, "Translation owner belongs to a different processor");
    assert_pending(&session, &error, 0b110, 2);
    assert_failed_session_is_frozen(&session, &error);
    assert_eq!(foreign.translation_maintenance_status().unwrap().pending, 0);
}
