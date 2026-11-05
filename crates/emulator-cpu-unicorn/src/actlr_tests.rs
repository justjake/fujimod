//! Cortex-A7 ACTLR register semantics from DDI0464F 4.3.31 and 4.3.34.
//!
//! MMU and caches remain disabled.
//! These tests cover register state and access, not cache timing, powerdown,
//! or delivery of maintenance to another CPU.
use crate::{Core, test_support};
use fujimod_emulator_cpu::{Cpu, Diagnostic, ExecutionError, Model, Register, Run, arm};

const RESET: u32 = 0x6000;
const MASK: u32 = 0x1000_fc40;
const SMP: u32 = 1 << 6;
const NS_SMP: u32 = 1 << 18;
const READ: u64 = 0x1000;
const WRITE: u64 = 0x1004;
const RMW: u64 = 0x100c;
const FALSE_WRITE: u64 = 0x101c;
const CODE: [u32; 8] = [
    0xee11_0f30, // MRC p15,0,r0,c1,c0,1: ACTLR.
    0xee01_0f30, // MCR p15,0,r0,c1,c0,1: ACTLR.
    0xee11_1f30, // MRC p15,0,r1,c1,c0,1: ACTLR.
    0xee11_0f30, // MRC p15,0,r0,c1,c0,1.
    0xe380_0040, // ORR r0,r0,#0x40: guest SMP enable preserves reset bits.
    0xee01_0f30, // MCR p15,0,r0,c1,c0,1.
    0xee11_1f30, // MRC p15,0,r1,c1,c0,1.
    0x0e01_0f30, // MCREQ p15,0,r0,c1,c0,1: skipped with Z=0.
];

fn cp(opc1: u32, crm: u32, opc2: u32, secure: u32) -> arm::Coprocessor {
    arm::Coprocessor {
        cp: 15,
        opc1,
        crn: 1,
        crm,
        opc2,
        secure,
        wide: false,
    }
}

fn set_reg(cpu: &Core, register: Register, value: u32) {
    cpu.write_register(register, &value.to_le_bytes()).unwrap();
}

fn reg(cpu: &Core, register: Register) -> u32 {
    let mut bytes = [0; 4];
    assert_eq!(cpu.read_register(register, &mut bytes).unwrap(), 4);
    u32::from_le_bytes(bytes)
}

fn core(model: Model) -> Core {
    let cpu = test_support::library().open(model, 1 << 20).unwrap();
    cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
    let bytes: Vec<u8> = CODE.into_iter().flat_map(u32::to_le_bytes).collect();
    cpu.write_memory(READ, &bytes).unwrap();
    if model == Model::CortexA7 {
        state(&cpu, false, 0x13, 0);
        assert_eq!(cpu.coprocessor(cp(0, 1, 0, 1), None).unwrap() & 1, 0);
    } else {
        // R5 has no SCR: only set SVC/ARM, IRQ/FIQ masked, Z=0.
        set_reg(&cpu, arm::CPSR, 0xd3);
    }
    cpu
}

fn run(cpu: &Core, begin: u64, count: usize) {
    let end = begin + count as u64 * 4;
    cpu.run(Run {
        begin,
        until: Some(end),
        instructions: Some(count),
        watchdog_us: 1_000_000,
    })
    .unwrap();
    assert_eq!(reg(cpu, arm::PC) as u64, end);
}

fn read(cpu: &Core) -> u32 {
    run(cpu, READ, 1);
    reg(cpu, arm::CORE[0])
}

fn write(cpu: &Core, value: u32) -> u32 {
    set_reg(cpu, arm::CORE[0], value);
    run(cpu, WRITE, 2);
    reg(cpu, arm::CORE[1])
}

fn assert_state(cpu: &Core, nonsecure: bool, mode: u32, nsacr: u32) {
    assert_eq!(reg(cpu, arm::CPSR), 0xc0 | mode);
    assert_eq!(
        cpu.coprocessor(cp(0, 1, 0, 1), None).unwrap(),
        u64::from(nonsecure),
    );
    assert_eq!(
        cpu.coprocessor(cp(0, 1, 2, 1), None).unwrap(),
        u64::from(nsacr),
    );
}

fn state(cpu: &Core, nonsecure: bool, mode: u32, nsacr: u32) {
    // Host setup selects a valid bridge through Secure SVC and Monitor.
    // A direct NS-SVC -> Monitor/Hyp CPSR write can silently retain SVC.
    cpu.coprocessor(cp(0, 1, 2, 1), Some(u64::from(nsacr)))
        .unwrap(); // NSACR.
    cpu.coprocessor(cp(0, 1, 0, 1), Some(0)).unwrap(); // SCR.NS=0.
    set_reg(cpu, arm::CPSR, 0xd3); // Secure SVC; rebuild execution flags.
    assert_state(cpu, false, 0x13, nsacr);
    set_reg(cpu, arm::CPSR, 0xd6); // Monitor can select either final regime.
    assert_state(cpu, false, 0x16, nsacr);
    cpu.coprocessor(cp(0, 1, 0, 1), Some(u64::from(nonsecure)))
        .unwrap(); // Final SCR.NS; Monitor itself remains Secure.
    assert_state(cpu, nonsecure, 0x16, nsacr);
    set_reg(cpu, arm::CPSR, 0xc0 | mode); // Rebuild flags for the final regime.
    assert_state(cpu, nonsecure, mode, nsacr);
}

#[test]
fn a7_reset_guest_rmw_and_reserved_bits() {
    let cpu = core(Model::CortexA7);
    assert_eq!(read(&cpu), RESET);
    run(&cpu, RMW, 4);
    assert_eq!(reg(&cpu, arm::CORE[1]), RESET | SMP);
    assert_eq!(write(&cpu, u32::MAX), MASK);
    assert_eq!(write(&cpu, !MASK), 0);
    // A fresh CPU resets independently; ACTLR is not a library/global value.
    let other = core(Model::CortexA7);
    assert_eq!(read(&other), RESET);
    assert_eq!(read(&cpu), 0);
}

#[test]
fn a7_nonsecure_pl1_and_pl2_only_change_permitted_smp() {
    for mode in [0x13, 0x1a] { // SVC (PL1), Hyp (PL2).
        for nsacr in [0, NS_SMP] {
            let cpu = core(Model::CortexA7);
            assert_eq!(write(&cpu, MASK), MASK);
            state(&cpu, true, mode, nsacr);
            assert_eq!(read(&cpu), MASK);
            let cleared = if nsacr == 0 { MASK } else { MASK & !SMP };
            assert_eq!(write(&cpu, 0), cleared);
            state(&cpu, false, 0x13, nsacr);
            assert_eq!(read(&cpu), cleared); // NS write is visible in Secure state.
            state(&cpu, true, mode, nsacr);
            assert_eq!(write(&cpu, u32::MAX), MASK);
            state(&cpu, false, 0x13, nsacr);
            assert_eq!(read(&cpu), MASK); // Common state, not a banked copy.
        }
    }
}

#[test]
fn a7_monitor_write_is_secure_even_when_scr_ns_is_set() {
    let cpu = core(Model::CortexA7);
    state(&cpu, true, 0x16, 0); // Monitor remains Secure despite SCR.NS=1.
    assert_eq!(write(&cpu, MASK), MASK);
    state(&cpu, true, 0x13, 0);
    assert_eq!(write(&cpu, 0), MASK); // Non-secure denied write is ignored.
    state(&cpu, false, 0x13, 0);
    assert_eq!(read(&cpu), MASK);
}

#[test]
fn a7_failed_condition_does_not_write_or_trap() {
    let cpu = core(Model::CortexA7);
    state(&cpu, true, 0x13, NS_SMP);
    cpu.coprocessor(cp(4, 1, 0, 0), Some(1 << 21))
        .unwrap(); // HCR.TAC: an executed ACTLR access would trap.
    assert_eq!(cpu.coprocessor(cp(4, 1, 0, 0), None).unwrap(), 1 << 21);
    set_reg(&cpu, arm::CORE[0], u32::MAX);
    run(&cpu, FALSE_WRITE, 1); // Z=0, so there is no access and no trap.
    assert_state(&cpu, true, 0x13, NS_SMP);
    state(&cpu, false, 0x13, 0);
    assert_eq!(read(&cpu), RESET);
}

#[test]
fn a7_native_context_restores_common_state_in_nonsecure_mode() {
    let cpu = core(Model::CortexA7);
    assert_eq!(write(&cpu, MASK), MASK);
    state(&cpu, true, 0x13, 0);
    let saved = cpu.snapshot(None).unwrap();
    state(&cpu, false, 0x13, 0);
    assert_eq!(write(&cpu, 0), 0);
    cpu.restore(saved.as_ref()).unwrap();
    assert_state(&cpu, true, 0x13, 0);
    assert_eq!(read(&cpu), MASK);
    assert_eq!(write(&cpu, 0), MASK); // Restored NS state and denied policy.
}

#[test]
fn a7_raw_setup_masks_reserved_bits_without_guest_nsacr_policy() {
    let cpu = core(Model::CortexA7);
    state(&cpu, true, 0x13, 0);
    cpu.coprocessor(cp(0, 0, 1, 0), Some(u64::from(u32::MAX)))
        .unwrap(); // Explicit raw state setup, not a guest MCR.
    assert_eq!(read(&cpu), MASK);
    assert_eq!(write(&cpu, 0), MASK);
}

#[test]
fn a7_user_access_still_raises_undefined_instruction() {
    for pc in [READ, WRITE] {
        let cpu = core(Model::CortexA7);
        state(&cpu, false, 0x10, 0); // User, Secure state.
        set_reg(&cpu, arm::CORE[0], u32::MAX);
        let result = cpu.run(Run {
            begin: pc,
            until: Some(pc + 4),
            instructions: Some(2),
            watchdog_us: 1_000_000,
        });
        // The raw ARM backend maps EXCP_UDEF to UC_ERR_INSN_INVALID before
        // dispatching exception hooks (arm_stop_interrupt/cpu_handle_exception).
        assert!(matches!(result, Err(ExecutionError::InvalidInstruction(_))));
        assert_eq!(cpu.coprocessor(cp(0, 0, 1, 0), None).unwrap(), RESET as u64);
    }
}

#[test]
fn a7_nonsecure_hcr_tac_still_traps_reads_and_writes() {
    for (pc, syndrome) in [(READ, 0x0fe2_0401), (WRITE, 0x0fe2_0400)] {
        for trap in [false, true] {
            // The same opcode succeeds on a fresh NS-SVC CPU with TAC clear.
            let cpu = core(Model::CortexA7);
            state(&cpu, true, 0x13, NS_SMP);
            let hcr = if trap { 1_u64 << 21 } else { 0 };
            cpu.coprocessor(cp(4, 1, 0, 0), Some(hcr)).unwrap();
            assert_eq!(cpu.coprocessor(cp(4, 1, 0, 0), None).unwrap(), hcr);
            assert_state(&cpu, true, 0x13, NS_SMP);
            assert_eq!(
                cpu.coprocessor(cp(0, 0, 1, 0), None).unwrap(),
                u64::from(RESET),
            );
            set_reg(&cpu, arm::CORE[0], u32::MAX);
            if trap {
                let result = cpu.run(Run {
                    begin: pc,
                    until: Some(pc + 4),
                    instructions: Some(1),
                    watchdog_us: 1_000_000,
                });
                // The raw backend reports the categorized EL2-targeted UDEF
                // before architectural exception delivery or register effects.
                assert!(matches!(result, Err(ExecutionError::InvalidInstruction(_))));
                assert_eq!(reg(&cpu, arm::PC) as u64, pc);
                assert_eq!(
                    reg(&cpu, Register::Diagnostic(Diagnostic::ExceptionSyndrome)),
                    syndrome,
                );
                assert_eq!(
                    cpu.coprocessor(cp(0, 0, 1, 0), None).unwrap(),
                    u64::from(RESET),
                );
            } else {
                run(&cpu, pc, 1);
                let expected = if pc == READ { RESET } else { RESET | SMP };
                assert_eq!(
                    cpu.coprocessor(cp(0, 0, 1, 0), None).unwrap(),
                    u64::from(expected),
                );
                if pc == READ {
                    assert_eq!(reg(&cpu, arm::CORE[0]), RESET);
                }
            }
            assert_state(&cpu, true, 0x13, NS_SMP);
            assert_eq!(cpu.coprocessor(cp(4, 1, 0, 0), None).unwrap(), hcr);
        }
    }
}

#[test]
fn r5_actlr_retains_its_previous_constant_behavior() {
    let cpu = core(Model::CortexR5F);
    let initial = read(&cpu);
    assert_eq!(initial, 0);
    assert_eq!(write(&cpu, u32::MAX), initial);
}
