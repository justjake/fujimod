use fujimod_emulator_cpu::arm;
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::json;
use std::{cell::Cell, rc::Rc};

const UPDATE: u32 = 0x1000;
const LOAD: u32 = 0x1800;
const TABLE: u32 = 0x4000;
const VA: u32 = 0x10_0000;
const OLD: u32 = 0x20_0000;
const NEW: u32 = 0x30_0000;
// Normal, non-cacheable, shareable section, full read/write access, domain 0.
const ATTR: u32 = (1 << 16) | (1 << 12) | (3 << 10) | 2;
const ALL_IS: u32 = 0xee08_0f13;
const ALL_LOCAL: u32 = 0xee08_0f17;
const DSB: u32 = 0xf57f_f04f;
const ISB: u32 = 0xf57f_f06f;
const NOP: u32 = 0xe1a0_0000;

fn cp(crn: u32, crm: u32, opc1: u32, opc2: u32) -> arm::Coprocessor {
    arm::Coprocessor { cp: 15, crn, crm, opc1, opc2, secure: 1, wide: false }
}
fn reg(s: &Session, core: usize, index: usize, value: u32) {
    s.write_register(core, arm::CORE[index], value.into()).unwrap();
}
fn words(s: &Session, address: u32, code: &[u32]) {
    let bytes: Vec<_> = code.iter().flat_map(|word| word.to_le_bytes()).collect();
    s.write(address as usize, &bytes).unwrap();
}
fn session(cores: usize) -> Session {
    let definition: Definition = serde_json::from_value(json!({
        "ram_size": 0x40_0000, "cores": cores, "discovery": false,
        "cbar": 0x2000_0000u32,
        "initial_registers": (0..cores).map(|_| json!([["Pc", LOAD]])).collect::<Vec<_>>(),
    })).unwrap();
    let s = Session::new(definition, None, vec![]).unwrap();
    words(&s, TABLE, &[ATTR, OLD | ATTR]);
    words(&s, OLD, &[0x11]);
    words(&s, NEW, &[0x22]);
    words(&s, LOAD, &[0xe590_2000, NOP]); // LDR r2,[r0].
    for core in 0..cores {
        s.coprocessor(core, cp(1, 1, 0, 0), Some(0)).unwrap(); // SCR: Secure.
        s.write_register(core, arm::CPSR, 0xd3).unwrap();
        s.coprocessor(core, cp(1, 0, 0, 1), Some(0x6040)).unwrap(); // ACTLR.SMP.
        s.coprocessor(core, cp(2, 0, 0, 0), Some(TABLE.into())).unwrap();
        s.coprocessor(core, cp(3, 0, 0, 0), Some(0x5555_5555)).unwrap();
        let control = s.coprocessor(core, cp(1, 0, 0, 0), None).unwrap();
        s.coprocessor(core, cp(1, 0, 0, 0), Some(control | 1)).unwrap();
        reg(&s, core, 0, VA);
    }
    s
}
fn load(s: &Session, core: usize) -> u64 {
    s.write_register(core, arm::PC, LOAD.into()).unwrap();
    s.run(core, 1).unwrap();
    s.read_register(core, arm::CORE[2]).unwrap()
}
fn warm(s: &Session, cores: usize) {
    for core in 0..cores { assert_eq!(load(s, core), 0x11); }
}
fn updater(s: &Session, operation: u32) {
    // Break before make: invalidate descriptor, publish, invalidate TLBs,
    // complete maintenance, install replacement, then publish replacement.
    words(s, UPDATE, &[0xe584_3000, DSB, operation, DSB, ISB, 0xe584_5000, DSB, ISB]);
    reg(s, 0, 3, 0);
    reg(s, 0, 4, TABLE + 4);
    reg(s, 0, 5, NEW | ATTR);
    s.write_register(0, arm::PC, UPDATE.into()).unwrap();
}
fn update(s: &Session, quantum: usize) {
    let before = s.instructions()[0];
    while s.instructions()[0] - before < 8 {
        let prior = s.instructions()[0];
        let remaining = 8 - (s.instructions()[0] - before) as usize;
        s.run(0, quantum.min(remaining)).unwrap();
        assert!(s.instructions()[0] > prior, "updater made no progress");
    }
    assert_eq!(s.instructions()[0] - before, 8);
    assert_eq!(s.read_register(0, arm::PC).unwrap(), u64::from(UPDATE + 32));
}

fn regime(s: &Session, core: usize, ns: bool, mode: u32) {
    s.write_register(core, arm::CPSR, 0xd6).unwrap(); // Monitor selects the next bank.
    s.coprocessor(core, cp(1, 1, 0, 0), Some(u64::from(ns))).unwrap();
    s.write_register(core, arm::CPSR, u64::from(0xc0 | mode)).unwrap();
    assert_eq!(s.read_register(core, arm::CPSR).unwrap(), u64::from(0xc0 | mode));
    assert_eq!(s.coprocessor(core, cp(1, 1, 0, 0), None).unwrap(), u64::from(ns));
}

#[test]
fn accepted_nonsecure_and_monitor_sources_deliver_without_peer_regime_filtering() {
    for (mode, secure, el) in [(0x13, 0, 1), (0x1a, 0, 2), (0x16, 1, 3)] {
        let s = session(2);
        warm(&s, 2);
        updater(&s, ALL_IS);
        regime(&s, 0, true, mode);
        update(&s, 8);
        let state = s.translation_maintenance_state().unwrap();
        assert_eq!(state.completed, 1);
        let event = &state.records[0].event;
        assert_eq!((event.secure, event.exception_level, event.cpsr & 31), (secure, el, mode));
        assert_eq!(load(&s, 1), 0x22);
    }
}

#[test]
fn peer_current_security_does_not_hide_its_older_cached_secure_translation() {
    for (operation, expected) in [(ALL_IS, 0x22), (ALL_LOCAL, 0x11)] {
        let s = session(2);
        warm(&s, 2);
        // Leave a secure cached translation while the peer is currently NS.
        regime(&s, 1, true, 0x13);
        updater(&s, operation);
        update(&s, 8);
        regime(&s, 1, false, 0x13);
        assert_eq!(load(&s, 1), expected);
    }
}

#[test]
fn shared_break_before_make_reaches_every_peer_for_short_and_long_quanta() {
    for cores in [2, 4] {
        for quantum in [1, 8] {
            let s = session(cores);
            warm(&s, cores);
            updater(&s, ALL_IS);
            let before = s.instructions();
            let tick = s.now();
            update(&s, quantum);
            assert_eq!(&s.instructions()[1..], &before[1..]);
            assert_eq!(s.now() - tick, 8);
            let state = s.translation_maintenance_state().unwrap();
            assert_eq!((state.completed, state.delivered_peers), (1, (cores - 1) as u64));
            let record = &state.records[0];
            assert_eq!(record.recipients, ((1u8 << cores) - 1) & !1);
            assert_eq!(record.applied, record.recipients);
            assert!(record.acknowledged);
            assert!(state.native.iter().all(|native| native.pending == 0 && native.poison == 0));
            for core in 0..cores { assert_eq!(load(&s, core), 0x22); }
        }
    }
}

#[test]
fn local_missing_and_failed_condition_operations_leave_peer_translation_cached() {
    for operation in [ALL_LOCAL, NOP, ALL_IS & 0x0fff_ffff] {
        let s = session(2);
        warm(&s, 2);
        updater(&s, operation);
        update(&s, 8);
        assert_eq!(s.translation_maintenance_state().unwrap().completed, 0);
        assert_eq!(load(&s, 1), 0x11);
        if operation == ALL_LOCAL { assert_eq!(load(&s, 0), 0x22); }
    }
}

#[test]
fn shared_invalidation_changes_peer_instruction_fetch_translation() {
    for (operation, expected) in [(ALL_IS, 0x22), (ALL_LOCAL, 0x11)] {
        let s = session(2);
        words(&s, OLD, &[0xe3a0_6011, NOP]); // MOV r6,#0x11.
        words(&s, NEW, &[0xe3a0_6022, NOP]); // MOV r6,#0x22.
        s.write_register(1, arm::PC, VA.into()).unwrap();
        s.run(1, 1).unwrap();
        assert_eq!(s.read_register(1, arm::CORE[6]).unwrap(), 0x11);
        updater(&s, operation);
        update(&s, 8);
        s.write_register(1, arm::PC, VA.into()).unwrap();
        s.run(1, 1).unwrap();
        assert_eq!(s.read_register(1, arm::CORE[6]).unwrap(), expected);
    }
}

#[test]
fn shared_invalidation_redirects_peer_stores_to_the_new_physical_page() {
    let s = session(2);
    words(&s, LOAD, &[0xe580_1000, NOP]); // STR r1,[r0].
    reg(&s, 1, 1, 0x33);
    s.run(1, 1).unwrap();
    updater(&s, ALL_IS);
    update(&s, 8);
    reg(&s, 1, 1, 0x44);
    s.write_register(1, arm::PC, LOAD.into()).unwrap();
    s.run(1, 1).unwrap();
    for (address, expected) in [(OLD, 0x33), (NEW, 0x44)] {
        let mut word = [0; 4];
        s.read(address as usize, &mut word).unwrap();
        assert_eq!(u32::from_le_bytes(word), expected);
    }
}

#[test]
fn nonparticipating_peer_is_excluded_and_other_sessions_are_isolated() {
    let s = session(4);
    let other = session(2);
    s.coprocessor(2, cp(1, 0, 0, 1), Some(0x6000)).unwrap();
    warm(&s, 4);
    warm(&other, 2);
    updater(&s, ALL_IS);
    // Change the other board's table too, without issuing its own maintenance.
    words(&other, TABLE + 4, &[NEW | ATTR]);
    update(&s, 8);
    let state = s.translation_maintenance_state().unwrap();
    assert_eq!(state.records[0].recipients, 0b1010);
    assert_eq!(state.records[0].excluded, 0b0100);
    assert_eq!(load(&s, 1), 0x22);
    assert_eq!(load(&s, 2), 0x11);
    assert_eq!(load(&s, 3), 0x22);
    assert_eq!(load(&other, 1), 0x11);
    assert_eq!(other.translation_maintenance_state().unwrap().completed, 0);
}

#[test]
fn source_outside_coherency_invalidates_only_itself() {
    let s = session(4);
    s.coprocessor(0, cp(1, 0, 0, 1), Some(0x6000)).unwrap();
    warm(&s, 4);
    updater(&s, ALL_IS);
    update(&s, 8);
    let state = s.translation_maintenance_state().unwrap();
    assert_eq!((state.records[0].recipients, state.records[0].excluded), (0, 0b1110));
    assert!(state.records[0].acknowledged);
    assert_eq!(state.delivered_peers, 0);
    assert_eq!(load(&s, 0), 0x22);
    for core in 1..4 { assert_eq!(load(&s, core), 0x11); }
}

#[test]
fn unsupported_participation_fails_before_any_peer_delivery_and_stays_failed() {
    for (core, actlr) in [(0, 0xe040), (2, 0xe040)] {
        let s = session(4);
        warm(&s, 4);
        s.coprocessor(core, cp(1, 0, 0, 1), Some(actlr)).unwrap();
        updater(&s, ALL_IS);
        let error = s.run(0, 8).unwrap_err();
        let state = s.translation_maintenance_state().unwrap();
        assert_eq!(state.completed, 0);
        assert_eq!(state.delivered_peers, 0);
        assert_eq!(state.native[0].pending, 1);
        assert_eq!(state.pending.as_ref().unwrap().applied, 0);
        assert_eq!(s.run(1, 1).unwrap_err(), error);
        assert_eq!(s.coprocessor(core, cp(1, 0, 0, 1), Some(0x6040)).unwrap_err(), error);
        assert_eq!(s.set_instruction_observer(None).unwrap_err(), error);
        assert_eq!(s.function_watch_end().unwrap_err(), error);
    }
}

#[test]
fn capacity_yield_delivers_all_events_without_executing_the_next_instruction() {
    let s = session(2);
    let mut code = vec![ALL_IS; 65];
    code.push(0xe281_1001); // ADD r1,r1,#1.
    words(&s, UPDATE, &code);
    s.write_register(0, arm::PC, UPDATE.into()).unwrap();
    s.run(0, 66).unwrap();
    assert_eq!(s.read_register(0, arm::PC).unwrap(), u64::from(UPDATE + 64 * 4));
    assert_eq!(s.read_register(0, arm::CORE[1]).unwrap(), 0);
    assert_eq!(s.instructions(), [64, 0]);
    assert_eq!(s.translation_maintenance_state().unwrap().completed, 64);
    s.run(0, 2).unwrap();
    assert_eq!(s.instructions(), [66, 0]);
    assert_eq!(s.read_register(0, arm::CORE[1]).unwrap(), 1);
    let state = s.translation_maintenance_state().unwrap();
    assert_eq!((state.completed, state.delivered_peers, state.dropped), (65, 65, 1));
    assert_eq!(state.records.len(), 64);
    assert_eq!(state.records.first().unwrap().event.sequence, 2);
    assert_eq!(state.records.last().unwrap().event.sequence, 65);
    assert!(state.native.iter().all(|native| native.pending == 0));
}

#[test]
fn earlier_accepted_event_is_settled_even_when_later_guest_access_fails() {
    let s = session(2);
    // MMU off makes this an absent physical region, not a handled guest
    // translation fault. The failure must remain fatal after maintenance.
    let control = s.coprocessor(0, cp(1, 0, 0, 0), None).unwrap();
    s.coprocessor(0, cp(1, 0, 0, 0), Some(control & !1)).unwrap();
    words(&s, UPDATE, &[ALL_IS, 0xe593_4000]);
    reg(&s, 0, 3, 0x40_0000);
    s.write_register(0, arm::PC, UPDATE.into()).unwrap();
    let error = s.run(0, 2).unwrap_err();
    let state = s.translation_maintenance_state().unwrap();
    assert_eq!((state.completed, state.delivered_peers), (1, 1));
    assert_eq!(state.native[0].pending, 0);
    assert!(state.records[0].acknowledged);
    assert!(state.last_raw_error.is_some());
    assert_eq!(s.run(1, 1).unwrap_err(), error);
}

#[test]
fn native_poison_preserves_earlier_event_and_does_not_flush_peers() {
    let s = session(2);
    words(&s, UPDATE, &[ALL_IS, 0xee08_0f33, 0xe281_1001]); // TLBIMVAIS unsupported.
    s.write_register(0, arm::PC, UPDATE.into()).unwrap();
    let error = s.run(0, 3).unwrap_err();
    let state = s.translation_maintenance_state().unwrap();
    assert_eq!((state.completed, state.delivered_peers), (0, 0));
    assert_eq!((state.native[0].pending, state.native[0].poison), (1, 3));
    assert_eq!(state.native[0].failed.sequence, 2);
    assert_eq!(s.read_register(0, arm::CORE[1]).unwrap(), 0);
    assert_eq!(s.run(1, 1).unwrap_err(), error);
}

#[test]
fn source_participation_is_captured_at_instruction_acceptance() {
    let s = session(2);
    // The shared operation was accepted before the guest cleared ACTLR.SMP.
    words(&s, UPDATE, &[ALL_IS, 0xee01_3f30]); // MCR ACTLR,r3.
    reg(&s, 0, 3, 0x6000);
    s.write_register(0, arm::PC, UPDATE.into()).unwrap();
    s.run(0, 2).unwrap();
    let state = s.translation_maintenance_state().unwrap();
    assert_eq!(state.completed, 1);
    assert_eq!(state.records[0].event.actlr, 0x6040);
    assert_eq!(s.coprocessor(0, cp(1, 0, 0, 1), None).unwrap(), 0x6000);
}

#[test]
fn live_observer_cannot_change_peer_participation_or_replace_instrumentation() {
    let s = Rc::new(session(2));
    words(&s, UPDATE, &[ALL_IS]);
    s.write_register(0, arm::PC, UPDATE.into()).unwrap();
    s.instruction_points(0, vec![u64::from(UPDATE)]).unwrap();
    let weak = Rc::downgrade(&s);
    let count = Rc::new(Cell::new(0));
    let seen = count.clone();
    s.set_instruction_observer(Some(Rc::new(move |core, _| {
        let s = weak.upgrade().unwrap();
        assert!(s.write_register(1, arm::CPSR, 0xd3).is_err());
        assert!(s.coprocessor(1, cp(1, 0, 0, 1), Some(0x6000)).is_err());
        assert!(s.set_instruction_observer(None).is_err());
        assert!(s.driver_guard_end().is_err());
        assert!(s.function_watch_end().is_err());
        s.write_register(core, arm::CORE[7], 99)?;
        seen.set(seen.get() + 1);
        Ok(())
    }))).unwrap();
    s.run(0, 1).unwrap();
    assert_eq!(count.get(), 1);
    assert_eq!(s.read_register(0, arm::CORE[7]).unwrap(), 99);
    assert_eq!(s.translation_maintenance_state().unwrap().completed, 1);
}
