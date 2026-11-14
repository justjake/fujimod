//! Accepted guest maintenance and the stopped-engine owner protocol.
//! These tests do not install a board owner or claim cross-core delivery.
use crate::{Core, ffi, maintenance, test_support};
use fujimod_emulator_cpu::{Cpu, ExecutionError, Model, Register, Run, arm};

const ALL_IS: u32 = 0xee08_0f13;
const ALL_LOCAL: u32 = 0xee08_0f17;
const MVA_IS: u32 = 0xee08_0f33;
const ADD_R1: u32 = 0xe281_1001;

fn cp(crn: u32, crm: u32, opc1: u32, opc2: u32) -> arm::Coprocessor {
    arm::Coprocessor { cp: 15, crn, crm, opc1, opc2, secure: 1, wide: false }
}
fn set(cpu: &Core, register: Register, value: u32) {
    cpu.write_register(register, &value.to_le_bytes()).unwrap();
}
fn reg(cpu: &Core, register: Register) -> u32 {
    let mut out = [0; 4];
    assert_eq!(cpu.read_register(register, &mut out).unwrap(), 4);
    u32::from_le_bytes(out)
}
fn state(cpu: &Core, ns: bool, mode: u32) {
    cpu.coprocessor(cp(1, 1, 0, 2), Some(1 << 18)).unwrap();
    cpu.coprocessor(cp(1, 1, 0, 0), Some(0)).unwrap();
    set(cpu, arm::CPSR, 0xd3);
    assert_eq!(reg(cpu, arm::CPSR), 0xd3);
    set(cpu, arm::CPSR, 0xd6);
    assert_eq!(reg(cpu, arm::CPSR), 0xd6);
    cpu.coprocessor(cp(1, 1, 0, 0), Some(u64::from(ns))).unwrap();
    set(cpu, arm::CPSR, 0xc0 | mode);
    assert_eq!(reg(cpu, arm::CPSR), 0xc0 | mode);
    assert_eq!(cpu.coprocessor(cp(1, 1, 0, 0), None).unwrap(), u64::from(ns));
}
fn core(code: &[u32]) -> Core {
    let cpu = test_support::library().open(Model::CortexA7, 1 << 20).unwrap();
    cpu.map_zeroed(0x1000, 0x2000, 7).unwrap();
    let bytes: Vec<_> = code.iter().flat_map(|v| v.to_le_bytes()).collect();
    cpu.write_memory(0x1000, &bytes).unwrap();
    state(&cpu, false, 0x13);
    cpu.coprocessor(cp(1, 0, 0, 1), Some(0x6040)).unwrap();
    cpu
}
fn run(cpu: &Core, begin: u64, entries: usize) -> Result<(), ExecutionError> {
    cpu.run(Run { begin, until: Some(begin + entries as u64 * 4),
        instructions: Some(entries), watchdog_us: 1_000_000 }).map(|_| ())
}
fn attach(cpu: &Core) -> maintenance::Owner {
    // The fixture owns its Core and executes no callbacks or peers.
    unsafe { cpu.attach_translation_owner() }.unwrap()
}

#[test]
fn owner_abi_layout_and_attachment_are_checked() {
    assert_eq!(std::mem::size_of::<maintenance::Event>(), 64);
    assert_eq!(std::mem::size_of::<maintenance::Status>(), 152);
    let cpu = core(&[ALL_IS]);
    let call = cpu.api.extensions.tlbi_attach.unwrap();
    assert_ne!(unsafe { call(cpu.raw, 0, 19) }, 0);
    assert_ne!(unsafe { call(cpu.raw, maintenance::ABI, 0) }, 0);
    assert_eq!(cpu.translation_maintenance_status().unwrap(), maintenance::Status::default());
    let _owner = attach(&cpu);
    assert!(unsafe { cpu.attach_translation_owner() }.is_err());
    let status = cpu.translation_maintenance_status().unwrap();
    assert_ne!(status.owner, 0);
    assert_eq!((status.next_sequence, status.pending, status.poison), (1, 0, 0));
    let used = core(&[ALL_LOCAL]);
    run(&used, 0x1000, 1).unwrap();
    assert!(unsafe { used.attach_translation_owner() }.is_err());
    let r5 = test_support::library().open(Model::CortexR5F, 1 << 20).unwrap();
    assert!(unsafe { r5.attach_translation_owner() }.is_err());
}

#[test]
fn accepted_event_keeps_execution_time_scope_and_standalone_is_unchanged() {
    // Two accepted events bracket a guest ACTLR write within one run.
    let code = [ALL_IS, 0xee01_2f30, ALL_IS, ADD_R1]; // MCR ACTLR,r2.
    let standalone = core(&code);
    run(&standalone, 0x1000, 4).unwrap();
    assert_eq!(standalone.translation_maintenance_status().unwrap().pending, 0);
    let cpu = core(&code);
    set(&cpu, arm::CORE[2], 0x6000);
    let owner = attach(&cpu);
    run(&cpu, 0x1000, 4).unwrap();
    assert_eq!(reg(&cpu, arm::PC), 0x1010);
    let first = cpu.translation_maintenance_status().unwrap();
    assert_eq!(first.pending, 2);
    assert_eq!(first.front.sequence, 1);
    assert_eq!(first.front.selector, maintenance::TLBIALLIS);
    assert_eq!((first.front.secure, first.front.exception_level), (1, 3));
    assert_eq!((first.front.cpsr, first.front.actlr), (0xd3, 0x6040));
    cpu.acknowledge_translation_maintenance(&owner, &first.front).unwrap();
    let second = cpu.translation_maintenance_status().unwrap();
    assert_eq!((second.pending, second.front.sequence, second.front.actlr), (1, 2, 0x6000));
    cpu.acknowledge_translation_maintenance(&owner, &second.front).unwrap();
    assert_eq!(cpu.translation_maintenance_status().unwrap().pending, 0);
}

#[test]
fn nonsecure_and_monitor_events_capture_actual_regime() {
    for (ns, mode, secure, el) in [(true, 0x13, 0, 1), (true, 0x1a, 0, 2), (true, 0x16, 1, 3)] {
        let cpu = core(&[ALL_IS]);
        state(&cpu, ns, mode);
        let _owner = attach(&cpu);
        run(&cpu, 0x1000, 1).unwrap();
        let event = cpu.translation_maintenance_status().unwrap().front;
        assert_eq!((event.cpsr, event.secure, event.exception_level), (0xc0 | mode, secure, el));
        assert_eq!((event.scr, event.nsacr, event.actlr), (1, 1 << 18, 0x6040));
    }
}

#[test]
fn failed_condition_local_host_and_trapped_access_emit_no_event() {
    for instruction in [ALL_LOCAL, ALL_IS & 0x0fff_ffff] {
        let cpu = core(&[instruction, ADD_R1]);
        let _owner = attach(&cpu);
        run(&cpu, 0x1000, 2).unwrap();
        assert_eq!(reg(&cpu, arm::CORE[1]), 1);
        assert_eq!(cpu.translation_maintenance_status().unwrap().pending, 0);
    }
    for user in [false, true] {
        let cpu = core(&[ALL_IS, ADD_R1]);
        if user {
            state(&cpu, false, 0x10);
        } else {
            state(&cpu, true, 0x13);
            let mut hcr = cp(1, 1, 4, 0);
            hcr.secure = 0;
            cpu.coprocessor(hcr, Some(1 << 25)).unwrap(); // HCR.TTLB.
        }
        let _owner = attach(&cpu);
        assert!(matches!(run(&cpu, 0x1000, 2), Err(ExecutionError::InvalidInstruction(_))));
        assert_eq!(reg(&cpu, arm::PC), 0x1000);
        assert_eq!(reg(&cpu, arm::CORE[1]), 0);
        let status = cpu.translation_maintenance_status().unwrap();
        assert_eq!((status.pending, status.poison), (0, 0));
    }
    let cpu = core(&[ALL_IS]);
    let _owner = attach(&cpu);
    let raw = ffi::CpReg { cp: 15, is64: 0, sec: 1, crn: 8, crm: 3, opc1: 0, opc2: 0, val: 0 };
    assert_eq!(unsafe { (cpu.api.uc_reg_write)(cpu.raw, 139, (&raw as *const ffi::CpReg).cast()) }, 0);
    assert_eq!(cpu.translation_maintenance_status().unwrap().pending, 0);
    for secure in [0, 1] {
        for (crm, opc1, opc2) in [(3,0,0),(3,0,1),(3,0,2),(3,0,3),(3,0,5),(3,0,7),
            (3,4,0),(3,4,1),(3,4,4),(3,4,5),(0,4,1),(0,4,5)] {
            let mut selector = cp(8, crm, opc1, opc2);
            selector.secure = secure;
            assert!(cpu.coprocessor(selector, Some(0)).is_err());
        }
    }
    assert_eq!(cpu.translation_maintenance_status().unwrap().pending, 0);
}

#[test]
fn pending_work_is_idempotent_and_cannot_be_skipped_or_acknowledged_elsewhere() {
    let cpu = core(&[ALL_IS, ALL_IS]);
    let owner = attach(&cpu);
    let other = core(&[ALL_IS]);
    let other_owner = attach(&other);
    run(&cpu, 0x1000, 2).unwrap();
    let before = cpu.translation_maintenance_status().unwrap();
    assert_eq!(before, cpu.translation_maintenance_status().unwrap());
    assert!(run(&cpu, 0x1008, 0).is_err());
    assert!(cpu.write_register(arm::CORE[1], &7u32.to_le_bytes()).is_err());
    assert!(cpu.write_memory(0x2000, &[5]).is_err());
    assert!(cpu.map_zeroed(0x4000, 0x1000, 7).is_err());
    assert!(cpu.invalidate_code(None).is_err());
    assert!(cpu.flush_local_translations(&owner).is_err());
    assert!(other.acknowledge_translation_maintenance(&owner, &before.front).is_err());
    assert!(cpu.acknowledge_translation_maintenance(&other_owner, &before.front).is_err());
    let mut altered = before.front;
    altered.sequence += 1;
    assert!(cpu.acknowledge_translation_maintenance(&owner, &altered).is_err());
    assert_eq!(before, cpu.translation_maintenance_status().unwrap());
    let ack = cpu.api.extensions.tlbi_ack.unwrap();
    assert_ne!(unsafe { ack(cpu.raw, maintenance::ABI, before.owner + 1, 1) }, 0);
    assert_ne!(unsafe { ack(cpu.raw, maintenance::ABI, before.owner, 2) }, 0);
    cpu.acknowledge_translation_maintenance(&owner, &before.front).unwrap();
    assert!(cpu.acknowledge_translation_maintenance(&owner, &before.front).is_err());
    let second = cpu.translation_maintenance_status().unwrap();
    cpu.acknowledge_translation_maintenance(&owner, &second.front).unwrap();
    cpu.flush_local_translations(&owner).unwrap();
    assert_eq!(other.translation_maintenance_status().unwrap().pending, 0);
}

#[test]
fn capacity_yields_after_completed_instruction_then_resumes_without_loss() {
    let mut code = vec![ALL_IS; maintenance::CAPACITY as usize + 1];
    code.push(ADD_R1);
    let cpu = core(&code);
    cpu.set_instruction_counter(true).unwrap();
    cpu.reset_instruction_counter().unwrap();
    let owner = attach(&cpu);
    run(&cpu, 0x1000, code.len()).unwrap();
    assert_eq!(reg(&cpu, arm::PC), 0x1100);
    assert_eq!(reg(&cpu, arm::CORE[1]), 0);
    assert_eq!(cpu.instruction_counter().unwrap(), 64);
    assert_eq!(cpu.translation_maintenance_status().unwrap().pending, 64);
    for sequence in 1..=64 {
        let status = cpu.translation_maintenance_status().unwrap();
        assert_eq!((status.front.sequence, status.poison), (sequence, 0));
        cpu.acknowledge_translation_maintenance(&owner, &status.front).unwrap();
    }
    // A budget, not an address exit, ends this run: the counter then holds
    // exactly the guest instructions.
    cpu.run(Run { begin: 0x1100, until: None, instructions: Some(2), watchdog_us: 1_000_000 }).unwrap();
    assert_eq!(reg(&cpu, arm::PC), 0x1108);
    assert_eq!(reg(&cpu, arm::CORE[1]), 1);
    let status = cpu.translation_maintenance_status().unwrap();
    assert_eq!((status.pending, status.front.sequence, status.poison), (1, 65, 0));
    assert_eq!(cpu.instruction_counter().unwrap(), 66);
}

#[test]
fn unsupported_shared_operation_poison_keeps_earlier_event_and_stops_following_code() {
    let cpu = core(&[ALL_IS, MVA_IS, ADD_R1]);
    let owner = attach(&cpu);
    assert!(run(&cpu, 0x1000, 3).is_err());
    assert_eq!(reg(&cpu, arm::PC), 0x1008);
    assert_eq!(reg(&cpu, arm::CORE[1]), 0);
    let status = cpu.translation_maintenance_status().unwrap();
    assert_eq!((status.pending, status.poison, status.front.sequence, status.failed.sequence), (1, 3, 1, 2));
    assert_eq!(status.failed.selector, maintenance::TLBIALLIS | 1);
    assert!(cpu.acknowledge_translation_maintenance(&owner, &status.front).is_err());
    assert!(run(&cpu, 0x1008, 1).is_err());
    assert!(cpu.write_register(arm::PC, &0x1008u32.to_le_bytes()).is_err());
    assert_eq!(status, cpu.translation_maintenance_status().unwrap());
}

#[test]
fn attached_contexts_cannot_remove_owner_obligations() {
    let cpu = core(&[ALL_IS]);
    let saved = cpu.snapshot(None).unwrap();
    let owner = attach(&cpu);
    assert!(cpu.snapshot(None).is_err());
    assert!(cpu.restore(saved.as_ref()).is_err());
    run(&cpu, 0x1000, 1).unwrap();
    let before = cpu.translation_maintenance_status().unwrap();
    assert!(cpu.snapshot(None).is_err());
    assert!(cpu.restore(saved.as_ref()).is_err());
    assert_eq!(before, cpu.translation_maintenance_status().unwrap());
    cpu.acknowledge_translation_maintenance(&owner, &before.front).unwrap();
    assert!(cpu.restore(saved.as_ref()).is_err());
}

#[test]
fn later_guest_fault_retains_the_accepted_event_for_settlement() {
    let cpu = core(&[ALL_IS, 0xe594_3000, ADD_R1]); // LDR r3,[r4] unmapped.
    set(&cpu, arm::CORE[4], 0x8000);
    let owner = attach(&cpu);
    assert!(matches!(run(&cpu, 0x1000, 3), Err(ExecutionError::Memory { .. })));
    assert_eq!(reg(&cpu, arm::PC), 0x1004);
    assert_eq!(reg(&cpu, arm::CORE[1]), 0);
    let status = cpu.translation_maintenance_status().unwrap();
    assert_eq!((status.pending, status.poison, status.front.sequence), (1, 0, 1));
    cpu.acknowledge_translation_maintenance(&owner, &status.front).unwrap();
    // This raw CPU interface does not own board failure policy. The board must
    // retain the original run error even after successful event settlement.
}

#[test]
fn force_broadcast_is_explicitly_unsupported_without_silent_local_success() {
    for ns in [false, true] {
        let cpu = core(&[ALL_LOCAL, ADD_R1]);
        state(&cpu, ns, 0x13);
        let mut hcr = cp(1, 1, 4, 0);
        hcr.secure = 0;
        cpu.coprocessor(hcr, Some(1 << 9)).unwrap(); // HCR.FB.
        let _owner = attach(&cpu);
        let result = run(&cpu, 0x1000, 2);
        if ns {
            assert!(result.is_err());
            assert_eq!(reg(&cpu, arm::CORE[1]), 0);
            let status = cpu.translation_maintenance_status().unwrap();
            assert_eq!((status.pending, status.poison, status.failed.hcr), (0, 3, 1 << 9));
        } else {
            // Backend Secure SVC is EL3, so HCR does not promote its local op.
            result.unwrap();
            assert_eq!(cpu.translation_maintenance_status().unwrap().poison, 0);
        }
    }
}

#[test]
fn owner_local_flush_discards_cached_data_translation_without_guest_execution() {
    let cpu = test_support::library().open(Model::CortexA7, 1 << 20).unwrap();
    cpu.map_zeroed(0, 0x40_0000, 7).unwrap();
    state(&cpu, false, 0x13);
    cpu.coprocessor(cp(1, 0, 0, 1), Some(0x6040)).unwrap();
    for (address, word) in [(0x4000, 0x0000_0c02u32), (0x4004, 0x0020_0c02),
        (0x20_0000, 0x11), (0x30_0000, 0x22), (0x1000, 0xe590_2000)] {
        cpu.write_memory(address, &word.to_le_bytes()).unwrap();
    }
    set(&cpu, arm::CORE[0], 0x10_0000);
    cpu.coprocessor(cp(2, 0, 0, 0), Some(0x4000)).unwrap();
    cpu.coprocessor(cp(3, 0, 0, 0), Some(0xffff_ffff)).unwrap();
    let sctlr = cpu.coprocessor(cp(1, 0, 0, 0), None).unwrap();
    cpu.coprocessor(cp(1, 0, 0, 0), Some(sctlr | 1)).unwrap();
    cpu.set_instruction_counter(true).unwrap();
    let owner = attach(&cpu);
    run(&cpu, 0x1000, 1).unwrap();
    assert_eq!(reg(&cpu, arm::CORE[2]), 0x11);
    // Deliberately retain a stale backend entry to test the host primitive;
    // this descriptor edit is not an architectural maintenance sequence.
    cpu.write_memory(0x4004, &0x0030_0c02u32.to_le_bytes()).unwrap();
    run(&cpu, 0x1000, 1).unwrap();
    assert_eq!(reg(&cpu, arm::CORE[2]), 0x11);
    let registers: Vec<_> = arm::CORE.into_iter().map(|r| reg(&cpu, r)).collect();
    let count = cpu.instruction_counter().unwrap();
    cpu.flush_local_translations(&owner).unwrap();
    assert_eq!(cpu.instruction_counter().unwrap(), count);
    assert_eq!(arm::CORE.into_iter().map(|r| reg(&cpu, r)).collect::<Vec<_>>(), registers);
    run(&cpu, 0x1000, 1).unwrap();
    assert_eq!(reg(&cpu, arm::CORE[2]), 0x22);
    assert_eq!(cpu.translation_maintenance_status().unwrap().pending, 0);
}

#[test]
fn owned_core_snapshot_and_restore_round_trip() {
    let cpu = core(&[ADD_R1, ADD_R1, ADD_R1]);
    set(&cpu, arm::CORE[1], 10);
    let owner = attach(&cpu);
    // The plain snapshot still refuses an attached engine.
    assert!(cpu.snapshot(None).is_err());
    let saved = cpu.snapshot_owned(&owner, None).unwrap();
    run(&cpu, 0x1000, 2).unwrap();
    assert_eq!(reg(&cpu, arm::CORE[1]), 12);
    assert_eq!(reg(&cpu, arm::PC), 0x1008);
    cpu.restore_owned(&owner, &*saved).unwrap();
    assert_eq!(reg(&cpu, arm::CORE[1]), 10);
    // The engine runs on after the restore and reaches the same state.
    run(&cpu, 0x1000, 2).unwrap();
    assert_eq!(reg(&cpu, arm::CORE[1]), 12);
    assert_eq!(reg(&cpu, arm::PC), 0x1008);
    // A snapshot can be restored more than once.
    cpu.restore_owned(&owner, &*saved).unwrap();
    assert_eq!(reg(&cpu, arm::CORE[1]), 10);
    assert_eq!(cpu.translation_maintenance_status().unwrap().pending, 0);
    // A snapshot of another engine is refused.
    let other = core(&[ADD_R1]);
    let foreign = other.snapshot(None).unwrap();
    assert!(cpu.restore_owned(&owner, &*foreign).is_err());
}

#[test]
fn owned_core_snapshot_refuses_pending_events_and_wrong_owner() {
    let cpu = core(&[ALL_IS]);
    let owner = attach(&cpu);
    let clean = cpu.snapshot_owned(&owner, None).unwrap();
    run(&cpu, 0x1000, 1).unwrap();
    assert_eq!(cpu.translation_maintenance_status().unwrap().pending, 1);
    assert!(cpu.snapshot_owned(&owner, None).is_err());
    assert!(cpu.restore_owned(&owner, &*clean).is_err());
    let front = cpu.translation_maintenance_status().unwrap().front;
    cpu.acknowledge_translation_maintenance(&owner, &front).unwrap();
    cpu.restore_owned(&owner, &*clean).unwrap();
    let other = core(&[ALL_IS]);
    let other_owner = attach(&other);
    assert!(cpu.snapshot_owned(&other_owner, None).is_err());
}
