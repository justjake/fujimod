use super::*;
use crate::rprofile::{
    core::tests::{get, processor_with_options, resume, run, set, words},
    tcm,
};

fn processor() -> Core {
    processor_with_options(&Memory::default(), Options::default())
}
fn cp(crn: u32, crm: u32, opc2: u32) -> arm::Coprocessor {
    arm::Coprocessor {
        cp: 15,
        opc1: 0,
        crn,
        crm,
        opc2,
        secure: 0,
        wide: false,
    }
}
fn clock(cpu: &Core) -> (u64, u64) {
    let clock = cpu.execution.clock.get().unwrap();
    (clock.now, clock.phase)
}
#[test]
fn fractional_clock_pmu_banked_registers_and_block_ring_replay_exactly() {
    let cpu = processor();
    cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
    words(&cpu, 0x1000, &[0xe2800001, 0xeafffffd]); // add r0,1; b 1000
    set(&cpu, arm::CPSR, 0xd3);
    set(&cpu, arm::CORE[13], 0x1234);
    set(&cpu, arm::SPSR, 0x60000053);
    set(&cpu, arm::CPSR, 0xd2);
    set(&cpu, arm::CORE[13], 0x4568);
    set(&cpu, arm::SPSR, 0x200000d3);
    set(&cpu, arm::CPSR, 0xd3);
    cpu.set_guest_clock(Clock {
        now: 100,
        phase: 7,
        ratio: fujimod_emulator_cpu::timeline::Ratio::new(24, 1).unwrap(),
    })
    .unwrap();
    cpu.coprocessor(cp(9, 12, 0), Some(1)).unwrap();
    cpu.coprocessor(cp(9, 12, 1), Some(0x80000000)).unwrap();
    run(&cpu, 0x1000, 35).unwrap();
    let saved = cpu.snapshot(None).unwrap();
    let original_progress = cpu.instruction_progress();
    resume(&cpu, 2113).unwrap(); // Wrap the accounting ring many times.
    let expected = (
        get(&cpu, arm::PC),
        get(&cpu, arm::CORE[0]),
        clock(&cpu),
        cpu.coprocessor(cp(9, 13, 0), None).unwrap(),
    );
    set(&cpu, arm::CORE[13], 0xdead);
    set(&cpu, arm::SPSR, 0);
    set(&cpu, arm::CPSR, 0xd2);
    set(&cpu, arm::CORE[13], 0xbeef);
    set(&cpu, arm::SPSR, 0);
    cpu.set_interrupt(Interrupt::Irq, true).unwrap();
    for quantum in [1, 17, 1000] {
        cpu.restore(saved.as_ref()).unwrap();
        assert_eq!(cpu.instruction_progress(), original_progress);
        assert_eq!(get(&cpu, arm::CORE[13]), 0x1234);
        assert_eq!(get(&cpu, arm::SPSR), 0x60000053);
        set(&cpu, arm::CPSR, 0xd2);
        assert_eq!(get(&cpu, arm::CORE[13]), 0x4568);
        assert_eq!(get(&cpu, arm::SPSR), 0x200000d3);
        set(&cpu, arm::CPSR, 0xd3);
        let mut remaining = 2113;
        while remaining != 0 {
            let n = quantum.min(remaining);
            resume(&cpu, n).unwrap();
            assert_eq!(cpu.instruction_progress(), Some(n as u64));
            remaining -= n;
        }
        assert_eq!(
            (
                get(&cpu, arm::PC),
                get(&cpu, arm::CORE[0]),
                clock(&cpu),
                cpu.coprocessor(cp(9, 13, 0), None).unwrap()
            ),
            expected
        );
        assert!(!cpu.execution.irq.get());
    }
}

#[test]
fn both_tcm_banks_restore_bytes_and_swapped_windows_without_replacing_underlay() {
    let cpu = processor_with_options(
        &Memory::default(),
        Options {
            tcm: tcm::Options {
                btcm: tcm::BankOptions {
                    bytes: 0x1000,
                    reset: 0x8001,
                },
                atcm: tcm::BankOptions {
                    bytes: 0x1000,
                    reset: 0xa001,
                },
            },
            ..Options::default()
        },
    );
    cpu.map_zeroed(0x1000, 0xb000, 7).unwrap();
    words(&cpu, 0x8000, &[0x12345678]);
    words(&cpu, 0xa000, &[0xe3a0002a, 0xe320f003]); // mov r0,42; WFI in ATCM
    set(&cpu, arm::CPSR, 0xd3);
    set(&cpu, arm::PC, 0xa000);
    let saved = cpu.snapshot(None).unwrap();
    let reused = cpu.snapshot(Some(saved.as_ref())).unwrap();
    let a = saved.as_any().downcast_ref::<Saved>().unwrap();
    let b = reused.as_any().downcast_ref::<Saved>().unwrap();
    assert_eq!(a.tcm[0].1.len(), b.tcm[0].1.len());
    // Disable before a position swap, then replace code and data in both banks.
    cpu.coprocessor(cp(9, 1, 0), Some(0x8000)).unwrap();
    cpu.coprocessor(cp(9, 1, 1), Some(0xa000)).unwrap();
    cpu.coprocessor(cp(9, 1, 0), Some(0xa001)).unwrap();
    cpu.coprocessor(cp(9, 1, 1), Some(0x8001)).unwrap();
    words(&cpu, 0xa000, &[0xdddddddd]);
    words(&cpu, 0x8000, &[0xe3a00063, 0xe320f003]);
    run(&cpu, 0x8000, 2).unwrap();
    assert_eq!(get(&cpu, arm::CORE[0]), 99);
    cpu.restore(saved.as_ref()).unwrap();
    assert_eq!(cpu.coprocessor(cp(9, 1, 0), None).unwrap(), 0x800d);
    assert_eq!(cpu.coprocessor(cp(9, 1, 1), None).unwrap(), 0xa00d);
    let mut bytes = [0; 4];
    cpu.read_memory(0x8000, &mut bytes).unwrap();
    assert_eq!(u32::from_le_bytes(bytes), 0x12345678);
    assert!(matches!(
        resume(&cpu, 2).unwrap(),
        Exit::Hint(Hint::WaitForInterrupt)
    ));
    assert_eq!(get(&cpu, arm::CORE[0]), 42);
    cpu.coprocessor(cp(9, 1, 0), Some(0x8000)).unwrap();
    cpu.read_memory(0x8000, &mut bytes).unwrap();
    assert_eq!(bytes, [0; 4]); // External RAM was never replaced by private bytes.
}

#[test]
fn cpu_only_restore_respects_external_writes_and_rejects_other_owners_and_maps() {
    let ram = Backing::aligned_zeros(0x2000, 4096).unwrap();
    let cpu = processor();
    unsafe {
        cpu.map_physical(0x1000, 0, 0x2000, 7, NonNull::new(ram.as_ptr()).unwrap())
            .unwrap();
    }
    words(&cpu, 0x1000, &[0xe1910f9f, 0xe1813f92]); // LDREX r0,[r1]; STREX r3,r2,[r1]
    set(&cpu, arm::CPSR, 0xd3);
    set(&cpu, arm::CORE[1], 0x2000);
    set(&cpu, arm::CORE[2], 42);
    run(&cpu, 0x1000, 1).unwrap();
    let saved = cpu.snapshot(None).unwrap();
    // Changing registers only does not lose the logical reservation.
    cpu.execution.clear_exclusive().unwrap();
    cpu.restore(saved.as_ref()).unwrap();
    resume(&cpu, 1).unwrap();
    assert_eq!(get(&cpu, arm::CORE[3]), 0);
    cpu.restore(saved.as_ref()).unwrap(); // The successful store changed RAM.
    resume(&cpu, 1).unwrap();
    assert_eq!(get(&cpu, arm::CORE[3]), 1);
    let other = processor();
    assert!(
        other
            .restore(saved.as_ref())
            .unwrap_err()
            .contains("different processor")
    );
    assert!(other.snapshot(Some(saved.as_ref())).is_err());
    let pc = get(&cpu, arm::PC);
    cpu.protect(0x2000, 0x1000, 5).unwrap();
    assert!(
        cpu.restore(saved.as_ref())
            .unwrap_err()
            .contains("external mappings differ")
    );
    assert_eq!(get(&cpu, arm::PC), pc);
}

#[test]
fn vfp_context_and_thumb_it_state_survive_restore_and_active_capture_refuses() {
    let cpu = processor();
    cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
    set(&cpu, arm::CPSR, 0xd3);
    set(&cpu, Register::Arm32(arm::Register::Cpacr), 0xf00000);
    set(&cpu, Register::Arm32(arm::Register::Fpexc), 0x40000000);
    let d0 = Register::Arm32(arm::Register::D(0));
    cpu.write_register(d0, &0x0123456789abcdefu64.to_le_bytes())
        .unwrap();
    // CMP r0,r0; ITE EQ; MOVEQ r1,7; MOVNE r1,9; WFI.
    cpu.write_memory(
        0x1000,
        &[0x80, 0x42, 0x0c, 0xbf, 0x07, 0x21, 0x09, 0x21, 0x30, 0xbf],
    )
    .unwrap();
    run(&cpu, 0x1001, 2).unwrap();
    let saved = cpu.snapshot(None).unwrap();
    let before = get(&cpu, arm::CPSR);
    assert_ne!(before & 0x0600fc00, 0); // A live IT condition, not just Thumb mode.
    resume(&cpu, 3).unwrap();
    assert_eq!(get(&cpu, arm::CORE[1]), 7);
    cpu.write_register(d0, &0u64.to_le_bytes()).unwrap();
    set(&cpu, Register::Arm32(arm::Register::Fpexc), 0);
    cpu.restore(saved.as_ref()).unwrap();
    assert_eq!(get(&cpu, arm::CPSR), before);
    assert_eq!(get(&cpu, Register::Arm32(arm::Register::Fpexc)), 0x40000000);
    let mut bytes = [0; 8];
    assert_eq!(cpu.read_register(d0, &mut bytes).unwrap(), 8);
    assert_eq!(u64::from_le_bytes(bytes), 0x0123456789abcdef);
    resume(&cpu, 3).unwrap();
    assert_eq!(get(&cpu, arm::CORE[1]), 7);
    struct Observe<'a> {
        cpu: &'a Core,
        refused: Cell<bool>,
    }
    unsafe fn callback(_: u64, _: u32, data: *mut c_void) {
        let observe = unsafe { &*data.cast::<Observe<'_>>() };
        observe.refused.set(observe.cpu.snapshot(None).is_err());
    }
    let observe = Observe {
        cpu: &cpu,
        refused: Cell::new(false),
    };
    let hook = unsafe {
        cpu.install_hook(Hook::Instruction {
            callback,
            data: (&observe as *const Observe<'_>).cast_mut().cast(),
            range: HookRange::All,
        })
        .unwrap()
    };
    run(&cpu, 0x1001, 1).unwrap();
    assert!(observe.refused.get());
    cpu.remove_hook(hook).unwrap();
    let raw = cpu.execution.cpu.snapshot(None).unwrap();
    assert!(
        cpu.restore(raw.as_ref())
            .unwrap_err()
            .contains("different backend")
    );
}
