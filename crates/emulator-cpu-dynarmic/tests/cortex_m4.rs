#![cfg(all(
    feature = "jit",
    any(target_arch = "aarch64", target_arch = "x86_64"),
    any(target_os = "linux", target_os = "macos")
))]

use fujimod_emulator_cpu::{Cpu, Exit, Hook, HookRange, Model, Register, Run, arm};
use fujimod_emulator_cpu_dynarmic::Core;
use fujimod_emulator_cpu_unicorn::Library;
use std::{cell::Cell, ffi::c_void};

use arm::Register as Arm;

const CODE: u32 = 0x1000;
const DATA: u32 = 0x2000;
const MSP: u32 = 0x3f00;
const PSP: u32 = 0x3800;

const IT_DSP_UNALIGNED: &[u16] = &[
    0x2800, // cmp r0, #0
    0xbf0c, // ite eq
    0xf44f, 0x7290, // moveq.w r2, #0x120
    0xf04f, 0x0234, // movne.w r2, #0x34
    0xf302, 0x0307, // ssat r3, #8, r2
    0xfa85, 0xf446, // uadd8 r4, r5, r6
    0xfaa5, 0xf786, // sel r7, r5, r6
    0xf8c1, 0x4002, // str.w r4, [r1, #2]
    0xf8d1, 0x8002, // ldr.w r8, [r1, #2]
    0xf8b1, 0x9004, // ldrh.w r9, [r1, #4]
    0xb40c, // push {r2, r3}
    0xe8bd, 0x0c00, // pop.w {r10, r11}
    0xf8c1, 0x7007, // str.w r7, [r1, #7]
];
const VFP: &[u16] = &[
    0xeee1, 0xca10, // vmsr fpscr, r12
    0xee70, 0xfa20, // vadd.f32 s31, s0, s1
    0xee6f, 0x7aa0, // vmul.f32 s15, s31, s1
    0xeef4, 0x7a6f, // vcmp.f32 s15, s31
    0xeef1, 0x3a10, // vmrs r3, fpscr
    0xeef1, 0xfa10, // vmrs APSR_nzcv, fpscr
    0xedc1, 0xfa04, // vstr s31, [r1, #16]
    0xed91, 0x7a04, // vldr s14, [r1, #16]
];
const SPECIAL: [Arm; 8] = [
    Arm::Msp,
    Arm::Psp,
    Arm::Control,
    Arm::Primask,
    Arm::Basepri,
    Arm::Faultmask,
    Arm::Ipsr,
    Arm::Fpscr,
];

fn put(cpu: &dyn Cpu, register: Arm, value: u32) {
    cpu.write_register(Register::Arm32(register), &value.to_le_bytes())
        .unwrap();
}

fn get(cpu: &dyn Cpu, register: Register) -> u32 {
    let mut buf = [0; 4];
    assert_eq!(cpu.read_register(register, &mut buf).unwrap(), 4);
    u32::from_le_bytes(buf)
}

fn memory(cpu: &dyn Cpu, addr: u32, size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size];
    cpu.read_memory(addr.into(), &mut bytes).unwrap();
    bytes
}

fn install_program(cpu: &dyn Cpu, code: &[u16]) {
    cpu.map_zeroed(CODE.into(), 0x1000, 7).unwrap();
    // Keep the unaligned transfer across two independently mapped pages.
    cpu.map_zeroed(DATA.into(), 0x1000, 3).unwrap();
    cpu.map_zeroed((DATA + 0x1000).into(), 0x1000, 3).unwrap();
    let code: Vec<u8> = code.iter().flat_map(|word| word.to_le_bytes()).collect();
    cpu.write_memory(CODE.into(), &code).unwrap();
}

fn request(begin: u32, count: usize) -> Run {
    Run {
        begin: u64::from(begin | 1),
        until: None,
        watchdog_us: 0,
        instructions: Some(count),
    }
}

unsafe fn count_instruction(_: u64, _: u32, data: *mut c_void) {
    let count = unsafe { &*data.cast::<Cell<u64>>() };
    count.set(count.get() + 1);
}

fn assert_unicorn_parity(
    program: &[u16],
    instructions: u64,
    setup: impl Fn(&dyn Cpu),
    check: impl Fn(&dyn Cpu),
) {
    let count = Cell::new(0);
    let unicorn = Library::linked()
        .unwrap()
        .open(Model::CortexM4, 4 * 1024 * 1024)
        .unwrap();
    let dynarmic = Core::new(Model::CortexM4).unwrap();
    for cpu in [&unicorn as &dyn Cpu, &dynarmic] {
        install_program(cpu, program);
        put(cpu, Arm::Msp, MSP);
        put(cpu, Arm::Psp, PSP);
        put(cpu, Arm::Primask, 1);
        put(cpu, Arm::Basepri, 0x40);
        put(cpu, Arm::Faultmask, 1);
        put(cpu, Arm::Xpsr, 1 << 24);
        setup(cpu);
    }
    for cpu in [&unicorn as &dyn Cpu, &dynarmic] {
        put(cpu, Arm::Cpacr, 0x00f0_0000);
    }
    let hook = unsafe {
        unicorn.install_hook(Hook::Instruction {
            callback: count_instruction,
            data: (&raw const count).cast_mut().cast(),
            range: HookRange::All,
        })
    }
    .unwrap();
    let end = CODE + program.len() as u32 * 2;
    let run = Run {
        until: Some(end.into()),
        ..request(CODE, 100)
    };
    assert!(matches!(unicorn.run(run).unwrap(), Exit::Stopped));
    unicorn.remove_hook(hook).unwrap();
    assert!(matches!(dynarmic.run(run).unwrap(), Exit::Stopped));
    assert_eq!(count.get(), instructions);
    assert_eq!(dynarmic.instruction_progress(), Some(count.get()));
    assert_eq!(get(&dynarmic, Register::Pc), end);
    for register in arm::CORE.into_iter().chain(
        SPECIAL
            .into_iter()
            .chain([Arm::Xpsr])
            .chain((0..32).map(Arm::S))
            .map(Register::Arm32),
    ) {
        let mut expected = get(&unicorn, register);
        if register == Register::Arm32(Arm::Control) {
            // Unicorn can set the ARMv8-M SFPA bit on FP context creation.
            // Cortex-M4 CONTROL defines only FPCA, SPSEL and nPRIV.
            expected &= 7;
        }
        assert_eq!(get(&dynarmic, register), expected, "{register:?}");
    }
    for page in [DATA, DATA + 0x1000] {
        assert_eq!(
            memory(&dynarmic, page, 0x1000),
            memory(&unicorn, page, 0x1000)
        );
    }
    check(&dynarmic);
}

#[test]
fn thumb_it_dsp_and_unaligned_access_match_unicorn() {
    for (condition, selected) in [(0, 0x120), (1, 0x34)] {
        assert_unicorn_parity(
            IT_DSP_UNALIGNED,
            13,
            |cpu| {
                put(cpu, Arm::Control, 2);
                put(cpu, Arm::R(0), condition);
                put(cpu, Arm::R(1), 0x2ffd);
                put(cpu, Arm::R(5), 0x10ff_017f);
                put(cpu, Arm::R(6), 0x0202_0381);
            },
            |cpu| {
                assert_eq!(get(cpu, arm::CORE[8]), 0x1201_0400);
                assert_eq!(get(cpu, arm::CORE[9]), 0x1201);
                assert_eq!(get(cpu, arm::CORE[10]), selected);
                assert_eq!(get(cpu, arm::CORE[11]), selected.min(127));
                assert_eq!(get(cpu, Register::Sp), PSP);
            },
        );
    }
}

#[test]
fn thumb_vfp_matches_unicorn() {
    assert_unicorn_parity(
        VFP,
        8,
        |cpu| {
            put(cpu, Arm::Control, 4);
            put(cpu, Arm::R(1), DATA);
            put(cpu, Arm::R(12), 1 << 22);
            for n in 0..32 {
                put(cpu, Arm::S(n), f32::from(n).to_bits());
            }
            put(cpu, Arm::S(0), 1.5_f32.to_bits());
            put(cpu, Arm::S(1), 2.0_f32.to_bits());
        },
        |cpu| {
            assert_eq!(get(cpu, Register::Arm32(Arm::S(15))), 7.0_f32.to_bits());
            assert_eq!(get(cpu, Register::Arm32(Arm::Xpsr)), 0x2100_0000);
            assert_eq!(memory(cpu, DATA + 16, 4), 3.5_f32.to_le_bytes());
        },
    );
}

#[test]
fn cpacr_changes_invalidate_warm_thumb_translations() {
    let cpu = Core::new(Model::CortexM4).unwrap();
    install_program(&cpu, &[0xbf00, 0xee70, 0xfa20, 0xe7fe]); // nop; vadd; b .
    let run = request(CODE, 2);
    put(&cpu, Arm::S(0), 1.5_f32.to_bits());
    put(&cpu, Arm::S(1), 2.0_f32.to_bits());
    for (cpacr, progress) in [(0, 1), (0x00f0_0000, 2), (0, 1)] {
        put(&cpu, Arm::Cpacr, cpacr);
        let exit = cpu.run(run).unwrap();
        if cpacr == 0 {
            let Exit::Trap(trap) = exit else {
                panic!("expected NOCP, got {exit:?}");
            };
            assert_eq!(trap.syndrome, arm::thumb::NOCP);
            assert_eq!(trap.virtual_address, u64::from(CODE + 2));
        } else {
            assert!(matches!(exit, Exit::Stopped));
            assert_eq!(get(&cpu, Register::Arm32(Arm::S(31))), 3.5_f32.to_bits());
        }
        assert_eq!(cpu.instruction_progress(), Some(progress));
    }
}

#[test]
fn native_batches_stop_before_observations_and_reobserve_after_cps() {
    let cpu = Core::new(Model::CortexM4).unwrap();
    install_program(&cpu, &[0x3001, 0x3001, 0x3001, 0xb672, 0x3001, 0xbf30]);
    for (begin, end, progress, primask, r0) in [
        (CODE, CODE + 6, 3, 0, 3),
        (CODE + 6, CODE + 8, 1, 1, 3),
        (CODE + 8, CODE + 10, 1, 1, 4),
    ] {
        assert!(matches!(
            cpu.run_mprofile_batch(request(begin, 20)).unwrap(),
            Exit::Stopped
        ));
        assert_eq!(cpu.instruction_progress(), Some(progress));
        assert_eq!(get(&cpu, Register::Pc), end);
        assert_eq!(get(&cpu, Register::Arm32(Arm::Primask)), primask);
        assert_eq!(get(&cpu, Register::Arm32(Arm::R(0))), r0);
    }
}

#[test]
fn thumb_budget_preserves_it_state_between_runs() {
    let cpu = Core::new(Model::CortexM4).unwrap();
    install_program(&cpu, IT_DSP_UNALIGNED);
    let xpsr = Register::Arm32(Arm::Xpsr);
    cpu.run(request(CODE, 2)).unwrap(); // CMP and ITE
    assert_eq!(cpu.instruction_progress(), Some(2));
    assert_eq!(get(&cpu, Register::Pc), CODE + 4);
    assert_eq!(get(&cpu, xpsr), 0x6100_0c00);
    cpu.run(request(CODE + 4, 1)).unwrap(); // EQ executes
    assert_eq!(cpu.instruction_progress(), Some(1));
    assert_eq!(get(&cpu, Register::Pc), CODE + 8);
    assert_eq!(get(&cpu, xpsr), 0x6100_1800);
    cpu.run(request(CODE + 8, 1)).unwrap(); // NE skips
    assert_eq!(cpu.instruction_progress(), Some(1));
    assert_eq!(get(&cpu, Register::Pc), CODE + 12);
    assert_eq!(get(&cpu, arm::CORE[2]), 0x120);
    assert_eq!(get(&cpu, xpsr), 0x6100_0000);
}

#[test]
fn register_file_switches_stacks_and_round_trips_snapshot() {
    let cpu = Core::new(Model::CortexM4).unwrap();
    let xpsr = Register::Arm32(Arm::Xpsr);
    assert_eq!(get(&cpu, xpsr), 1 << 24);
    put(&cpu, Arm::Msp, MSP);
    put(&cpu, Arm::Psp, PSP);
    put(&cpu, Arm::Control, 2);
    assert_eq!(get(&cpu, Register::Sp), PSP);
    put(&cpu, Arm::R(13), PSP - 4);
    put(&cpu, Arm::Ipsr, 16);
    assert_eq!(get(&cpu, Register::Sp), MSP);
    let status = 0xae0a_8411; // APSR, both IT fields, exception 17, T clear on input
    put(&cpu, Arm::Xpsr, status);
    assert_eq!(get(&cpu, xpsr), status | (1 << 24));
    assert_eq!(get(&cpu, Register::Arm32(Arm::Ipsr)), 17);
    put(&cpu, Arm::Primask, 1);
    put(&cpu, Arm::Basepri, 0x40);
    put(&cpu, Arm::Faultmask, 1);
    put(&cpu, Arm::S(31), 0x7fc0_1234);
    put(&cpu, Arm::Fpscr, 1 << 22);
    let before: Vec<_> = SPECIAL
        .into_iter()
        .chain([Arm::Xpsr, Arm::S(31)])
        .map(|reg| (reg, get(&cpu, Register::Arm32(reg))))
        .collect();
    let snapshot = cpu.snapshot(None).unwrap();
    for &(reg, _) in &before {
        put(&cpu, reg, 0);
    }
    cpu.restore(snapshot.as_ref()).unwrap();
    for (reg, expected) in before {
        assert_eq!(get(&cpu, Register::Arm32(reg)), expected, "{reg:?}");
    }
    assert_eq!(get(&cpu, Register::Sp), MSP);
    put(&cpu, Arm::Xpsr, 0);
    assert_eq!(get(&cpu, Register::Sp), PSP - 4);
    assert_eq!(get(&cpu, xpsr), 1 << 24);
}

#[test]
fn exchange_to_even_address_stops_at_the_halfword_target() {
    for branch in [&[0x4700][..], &[0xbd00], &[0xf8d1, 0xf000]] {
        let cpu = Core::new(Model::CortexM4).unwrap();
        install_program(&cpu, branch); // BX r0, POP {pc}, LDR.W pc,[r1]
        let target = 0x2002_u32; // Mapped with no execute permission.
        put(&cpu, Arm::R(0), target);
        put(&cpu, Arm::R(1), DATA);
        put(&cpu, Arm::Msp, DATA);
        cpu.write_memory(DATA.into(), &target.to_le_bytes())
            .unwrap();
        let Exit::Trap(trap) = cpu.run(request(CODE, 8)).unwrap() else {
            panic!("even exchange target must raise INVSTATE");
        };
        assert_eq!(trap.syndrome, 1 << 17);
        assert_eq!(trap.virtual_address, u64::from(target));
        assert_eq!(get(&cpu, Register::Pc), target);
        assert_eq!(get(&cpu, Register::Arm32(Arm::Xpsr)) & (1 << 24), 1 << 24);
        assert_eq!(cpu.instruction_progress(), Some(1));
    }
}

#[test]
fn an_a53_store_through_an_alias_invalidates_compiled_thumb_code() {
    use fujimod_emulator_cpu::{code::CodeTracker, memory::Backing};
    use std::ptr::NonNull;

    let tracker = CodeTracker::default();
    let ram = Backing::aligned_zeros(0x2000, 0x1000).unwrap();
    let pointer = NonNull::new(ram.as_ptr()).unwrap();
    let m4 = Core::new_with_code_tracker(Model::CortexM4, tracker.clone()).unwrap();
    let a53 = Library::linked()
        .unwrap()
        .open(Model::ArmV8A, 1 << 20)
        .unwrap()
        .with_code_tracker(tracker)
        .unwrap();
    unsafe {
        m4.map_backing(0x1000, 0x2000, 7, pointer).unwrap();
        a53.map_backing(0x8000, 0x2000, 3, pointer).unwrap();
    }

    // MOVW r0, #1 crosses the backing page boundary. The A53 writes only
    // the second halfword's low byte through a non-executable view.
    m4.write_memory(0x1ffe, &[0x40, 0xf2, 1, 0, 0xfe, 0xe7])
        .unwrap();
    a53.map_zeroed(0x4000, 0x1000, 7).unwrap();
    a53.write_memory(0x4000, &0x3900_0020u32.to_le_bytes()) // strb w0, [x1]
        .unwrap();
    a53.write_register(Register::X(0), &9u64.to_le_bytes())
        .unwrap();
    a53.write_register(Register::X(1), &0x9000u64.to_le_bytes())
        .unwrap();

    m4.run_mprofile_batch(request(0x1ffe, 2)).unwrap();
    assert_eq!(get(&m4, arm::CORE[0]), 1);
    a53.run(Run {
        begin: 0x4000,
        ..request(0x4000, 1)
    })
    .unwrap();
    m4.run_mprofile_batch(request(0x1ffe, 2)).unwrap();
    assert_eq!(get(&m4, arm::CORE[0]), 9);
}
