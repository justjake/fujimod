use super::super::tests::{get, processor_with_options, set, words};
use super::*;

fn execute(code: &[u32], mode: LoopMode, budget: usize, counter: u32) -> (Vec<u32>, u64, u64) {
    let cpu = processor_with_options(&Memory::default(), Options::default());
    cpu.map_zeroed(0, 0x4000, 7).unwrap();
    words(&cpu, 0x1000, code);
    set(&cpu, arm::CPSR, 0x13 | 0xa000_0000);
    set(&cpu, arm::CORE[0], counter);
    set(&cpu, arm::CORE[2], 0x2000);
    set(&cpu, arm::CORE[3], 200);
    cpu.set_loop_mode(mode).unwrap();
    cpu.set_loop_window(true).unwrap();
    cpu.run(Run {
        begin: 0x1000,
        until: None,
        watchdog_us: 0,
        instructions: Some(budget),
    })
    .unwrap();
    (
        arm::CORE
            .iter()
            .chain([arm::CPSR].iter())
            .map(|&r| get(&cpu, r))
            .collect(),
        cpu.instruction_progress().unwrap(),
        cpu.exit_counters().unwrap()["loop_accelerated_instructions"],
    )
}

#[test]
fn every_counted_loop_budget_matches_instruction_execution() {
    for code in [
        vec![0xe2500001, 0x1afffffd, 0xe320f003], // SUBS; BNE
        vec![0xe2800001, 0xe35000c8, 0x3afffffc, 0xe320f003],
        vec![0xe2800001, 0xe1500003, 0x3afffffc, 0xe320f003], // ADD; CMP; BLO
        vec![0xe2400001, 0xe3500009, 0x8afffffc, 0xe320f003], // SUB; CMP; BHI
    ] {
        for budget in (1..40).chain([100, 101, 199, 200, 201, 399, 600]) {
            let off = execute(&code, LoopMode::Disabled, budget, 100);
            let on = execute(&code, LoopMode::Accelerate, budget, 100);
            assert_eq!((&off.0, off.1), (&on.0, on.1), "{code:x?}, budget={budget}");
            if budget > 100 {
                assert!(on.2 > 0);
            }
        }
    }
}
#[test]
fn ram_poll_and_unconditional_fixed_point_match_at_every_instruction() {
    for code in [
        vec![0xe5921000, 0xe3110001, 0x0afffffc], // LDR; TST; BEQ
        vec![0xeafffffe],                         // B .
    ] {
        for budget in 1..40 {
            let off = execute(&code, LoopMode::Disabled, budget, 1);
            let on = execute(&code, LoopMode::Accelerate, budget, 1);
            assert_eq!((&off.0, off.1), (&on.0, on.1), "budget={budget}");
            if budget > 10 {
                assert!(on.2 > 0);
            }
        }
    }
}
#[test]
fn wrapping_counters_and_false_conditions_are_conservative() {
    for value in [0, 1, 2, 0x7fff_ffff, 0x8000_0000, u32::MAX] {
        let code = [0xe2500001, 0x1afffffd, 0xe320f003];
        let off = execute(&code, LoopMode::Disabled, 101, value);
        let on = execute(&code, LoopMode::Accelerate, 101, value);
        assert_eq!((&off.0, off.1), (&on.0, on.1));
    }
}
#[test]
fn shifted_operands_and_flags_follow_the_cpu() {
    for code in [
        vec![0xe1b010a0, 0xeafffffd], // MOVS r1,r0,LSR #1; B
        vec![0xe2101102, 0xeafffffd], // ANDS immediate with rotated carry
    ] {
        for value in [0, 1, 2, 0x8000_0001, u32::MAX] {
            let off = execute(&code, LoopMode::Disabled, 100, value);
            let on = execute(&code, LoopMode::Accelerate, 100, value);
            assert_eq!((&off.0, off.1), (&on.0, on.1));
        }
    }
}

#[test]
fn stable_permission_expires_and_observers_keep_every_instruction() {
    let cpu = processor_with_options(&Memory::default(), Options::default());
    cpu.map_zeroed(0, 0x4000, 7).unwrap();
    words(&cpu, 0x1000, &[0xe2500001, 0x1afffffd, 0xe320f003]);
    set(&cpu, arm::CPSR, 0xd3);
    cpu.set_loop_mode(LoopMode::Accelerate).unwrap();
    for stable in [true, false] {
        set(&cpu, arm::CORE[0], 1000);
        if stable {
            cpu.set_loop_window(true).unwrap();
        }
        let before = cpu.exit_counters().unwrap()["loop_accelerated_instructions"];
        super::super::tests::run(&cpu, 0x1000, 1500).unwrap();
        let after = cpu.exit_counters().unwrap()["loop_accelerated_instructions"];
        assert_eq!(after > before, stable);
    }
    unsafe fn observe(_: u64, _: u32, data: *mut c_void) {
        unsafe {
            *data.cast::<usize>() += 1;
        }
    }
    let mut hits = 0usize;
    let hook = unsafe {
        cpu.install_hook(Hook::Instruction {
            callback: observe,
            data: (&mut hits as *mut usize).cast(),
            range: HookRange::Inclusive {
                begin: 0x1000,
                end: 0x1004,
            },
        })
        .unwrap()
    };
    set(&cpu, arm::CORE[0], 1000);
    cpu.set_loop_window(true).unwrap();
    let before = cpu.exit_counters().unwrap()["loop_accelerated_instructions"];
    super::super::tests::run(&cpu, 0x1000, 1500).unwrap();
    assert_eq!(hits, 1500);
    assert_eq!(
        cpu.exit_counters().unwrap()["loop_accelerated_instructions"],
        before
    );
    cpu.remove_hook(hook).unwrap();
}

#[test]
fn rewritten_code_and_ram_are_reproved_after_a_resume() {
    for mode in [LoopMode::Disabled, LoopMode::Measure, LoopMode::Accelerate] {
        let cpu = processor_with_options(&Memory::default(), Options::default());
        cpu.map_zeroed(0, 0x4000, 7).unwrap();
        words(
            &cpu,
            0x1000,
            &[0xe5921000, 0xe3110001, 0x0afffffc, 0xe320f003],
        );
        set(&cpu, arm::CPSR, 0xd3);
        set(&cpu, arm::CORE[2], 0x2000);
        cpu.set_loop_mode(mode).unwrap();
        cpu.set_loop_window(true).unwrap();
        super::super::tests::run(&cpu, 0x1000, 99).unwrap();
        words(&cpu, 0x2000, &[1]);
        cpu.set_loop_window(true).unwrap();
        super::super::tests::resume(&cpu, 100).unwrap();
        assert_eq!(cpu.instruction_progress(), Some(4));
        assert_eq!(get(&cpu, arm::CORE[1]), 1);
        assert_eq!(get(&cpu, arm::PC), 0x1010);
        // Replace the body with a store; it must execute once per iteration.
        words(&cpu, 0x1000, &[0xe5821000, 0xe2811001, 0xeafffffc]);
        cpu.set_loop_window(true).unwrap();
        super::super::tests::run(&cpu, 0x1000, 300).unwrap();
        assert_eq!(get(&cpu, arm::CORE[1]), 101);
        let mut value = [0; 4];
        cpu.read_memory(0x2000, &mut value).unwrap();
        assert_eq!(u32::from_le_bytes(value), 100);
    }
}

#[test]
fn measurement_executes_the_complete_counted_and_poll_loops() {
    for (code, counter) in [
        (vec![0xe2500001, 0x1afffffd, 0xe320f003], 100),
        (vec![0xe5921000, 0xe3110001, 0x0afffffc], 0),
    ] {
        for budget in [1, 2, 3, 4, 99, 199, 200, 201, 300] {
            let off = execute(&code, LoopMode::Disabled, budget, counter);
            let measured = execute(&code, LoopMode::Measure, budget, counter);
            assert_eq!(
                (&off.0, off.1),
                (&measured.0, measured.1),
                "code={code:x?} budget={budget}"
            );
            assert_eq!(measured.2, 0);
        }
    }
}

#[test]
fn address_stops_and_unbounded_runs_do_not_skip_loops() {
    for (until, instructions, expected) in [(Some(0x1004), Some(1000), 1), (None, None, 201)] {
        let cpu = processor_with_options(&Memory::default(), Options::default());
        cpu.map_zeroed(0, 0x4000, 7).unwrap();
        words(&cpu, 0x1000, &[0xe2500001, 0x1afffffd, 0xe320f003]);
        set(&cpu, arm::CPSR, 0xd3);
        set(&cpu, arm::CORE[0], 100);
        cpu.set_loop_mode(LoopMode::Accelerate).unwrap();
        cpu.set_loop_window(true).unwrap();
        cpu.run(Run {
            begin: 0x1000,
            until,
            watchdog_us: 0,
            instructions,
        })
        .unwrap();
        assert_eq!(cpu.instruction_progress(), Some(expected));
        assert_eq!(
            cpu.exit_counters().unwrap()["loop_accelerated_instructions"],
            0
        );
    }
}

#[test]
fn counted_loops_preserve_guest_pmu_time_and_complete_cpu_state() {
    let simulate = |mode, budget| {
        let cpu = processor_with_options(&Memory::default(), Options::default());
        cpu.map_zeroed(0, 0x4000, 7).unwrap();
        words(
            &cpu,
            0x1000,
            &[0xe2500001, 0x1afffffd, 0xee194f1d, 0xe320f003],
        );
        set(&cpu, arm::CPSR, 0xd3);
        set(&cpu, arm::CORE[0], 100);
        cpu.set_guest_clock(Clock {
            now: 100,
            phase: 2,
            ratio: fujimod_emulator_cpu::timeline::Ratio::new(7, 3).unwrap(),
        })
        .unwrap();
        let pmu = |opc2| arm::Coprocessor {
            cp: 15,
            crn: 9,
            crm: 12,
            opc1: 0,
            opc2,
            secure: 0,
            wide: false,
        };
        cpu.coprocessor(pmu(0), Some(1)).unwrap();
        cpu.coprocessor(pmu(1), Some(0x80000000)).unwrap();
        cpu.set_loop_mode(mode).unwrap();
        cpu.set_loop_window(true).unwrap();
        super::super::tests::run(&cpu, 0x1000, budget).unwrap();
        (
            cpu.state().unwrap(),
            cpu.instruction_progress(),
            cpu.exit_counters().unwrap()["loop_accelerated_instructions"],
        )
    };
    for budget in [1, 2, 3, 4, 7, 8, 99, 199, 200, 201, 202] {
        let off = simulate(LoopMode::Disabled, budget);
        let on = simulate(LoopMode::Accelerate, budget);
        assert_eq!((&off.0, off.1), (&on.0, on.1), "budget={budget}");
        if budget > 8 {
            assert!(on.2 > 0);
        }
    }
}

#[test]
fn policy_switches_and_restores_reclassify_cached_loop_branches() {
    let cpu = processor_with_options(&Memory::default(), Options::default());
    cpu.map_zeroed(0, 0x4000, 7).unwrap();
    words(&cpu, 0x1000, &[0xe2500001, 0x1afffffd, 0xe320f003]);
    cpu.set_loop_mode(LoopMode::Accelerate).unwrap();
    let exercise = |policy| {
        assert_eq!(cpu.execution.policy.get(), policy);
        // Interactive must translate this ordinary branch without a loop
        // observer. Exact must regain its proof even after a cached fast run.
        let observed = matches!(
            cpu.execution
                .classify(0x1004, 0x1afffffd)
                .unwrap()
                .instruction,
            InstructionDisposition::ObserveAndEndBlock
        );
        assert_eq!(observed, policy == ExecutionPolicy::Exact);
        set(&cpu, arm::CPSR, 0xd3);
        set(&cpu, arm::CORE[0], 1000);
        cpu.set_loop_window(true).unwrap();
        let before = cpu.exit_counters().unwrap()["loop_accelerated_instructions"];
        super::super::tests::run(&cpu, 0x1000, 1000).unwrap();
        assert_eq!(
            (
                get(&cpu, arm::PC),
                get(&cpu, arm::CORE[0]),
                get(&cpu, arm::CPSR)
            ),
            (0x1000, 500, 0x200000d3)
        );
        assert_eq!(cpu.instruction_progress(), Some(1000));
        assert_eq!(
            cpu.exit_counters().unwrap()["loop_accelerated_instructions"] > before,
            policy == ExecutionPolicy::Exact
        );
    };
    for (policy, other) in [
        (ExecutionPolicy::Interactive, ExecutionPolicy::Exact),
        (ExecutionPolicy::Exact, ExecutionPolicy::Interactive),
    ] {
        cpu.set_execution_policy(policy).unwrap();
        exercise(policy);
        let snapshot = cpu.snapshot(None).unwrap();
        cpu.set_execution_policy(other).unwrap();
        exercise(other);
        cpu.restore(snapshot.as_ref()).unwrap();
        exercise(policy);
    }
}

const WFI: u16 = 0xbf30;

struct Thumb {
    state: fujimod_emulator_cpu::CpuState,
    progress: Option<u64>,
    counters: std::collections::BTreeMap<&'static str, u64>,
}

/// Run Thumb code at 0x1000 with R2 addressing the RAM word `flag` at 0x2000.
fn thumb(code: &[u16], mode: LoopMode, budget: usize, counter: u32, flag: u32) -> Thumb {
    let cpu = processor_with_options(&Memory::default(), Options::default());
    cpu.map_zeroed(0, 0x4000, 7).unwrap();
    cpu.write_memory(
        0x1000,
        &code.iter().flat_map(|h| h.to_le_bytes()).collect::<Vec<_>>(),
    )
    .unwrap();
    words(&cpu, 0x2000, &[flag]);
    set(&cpu, arm::CPSR, 0x33 | 0xa000_0000);
    set(&cpu, arm::CORE[0], counter);
    set(&cpu, arm::CORE[2], 0x2000);
    set(&cpu, arm::CORE[3], 200);
    cpu.set_guest_clock(Clock {
        now: 100,
        phase: 2,
        ratio: fujimod_emulator_cpu::timeline::Ratio::new(7, 3).unwrap(),
    })
    .unwrap();
    cpu.set_loop_mode(mode).unwrap();
    cpu.set_loop_window(true).unwrap();
    super::super::tests::run(&cpu, 0x1001, budget).unwrap();
    Thumb {
        state: cpu.state().unwrap(),
        progress: cpu.instruction_progress(),
        counters: cpu.exit_counters().unwrap(),
    }
}
/// Disabled, Measure and Accelerate leave identical state and instruction
/// counts (the guest clock position follows the count); returns whether
/// Accelerate skipped instructions.
fn compare_thumb(code: &[u16], budget: usize, counter: u32, flag: u32) -> bool {
    let off = thumb(code, LoopMode::Disabled, budget, counter, flag);
    let measured = thumb(code, LoopMode::Measure, budget, counter, flag);
    let on = thumb(code, LoopMode::Accelerate, budget, counter, flag);
    let context = format!("code={code:04x?} budget={budget} counter={counter} flag={flag}");
    assert_eq!((&off.state, off.progress), (&on.state, on.progress), "{context}");
    assert_eq!(
        (&off.state, off.progress),
        (&measured.state, measured.progress),
        "{context}"
    );
    assert_eq!(measured.counters["loop_accelerated_instructions"], 0, "{context}");
    on.counters["loop_accelerated_instructions"] > 0
}

#[test]
fn thumb_status_polls_match_instruction_execution() {
    for code in [
        vec![0x6811, 0x07c9, 0xd5fc, WFI], // LDR; LSLS #31; BPL
        vec![0xf8d2, 0x1000, 0xf011, 0x0f01, 0xd0fa, WFI], // LDR.W; TST.W #1; BEQ
        vec![0x6811, 0xb901, 0xe7fc, WFI], // LDR; CBNZ out; B
        vec![0x6811, 0xb101, 0xe7fc, WFI], // LDR; CBZ out; B
        vec![0x6811, 0x2900, 0xbf0c, 0x2301, 0x2302, 0x2b02, 0xd0f8, WFI], // ITE EQ
        vec![0xe7fe],                      // B .
        vec![0x8815, 0xf005, 0x0504, 0xb2ad, 0x2d00, 0xd0f9, WFI], // LDRH; AND.W; UXTH; CMP; BEQ
    ] {
        let mut accelerated = false;
        for flag in [0, 1, 4] {
            for budget in (1..40).chain([100, 1000]) {
                accelerated |= compare_thumb(&code, budget, 1, flag);
            }
        }
        assert!(accelerated, "{code:04x?}");
    }
}
#[test]
fn thumb_counted_delay_loops_match_instruction_execution() {
    for code in [
        vec![0x3801, 0xd1fd, WFI],                 // SUBS #1; BNE
        vec![0x3001, 0x28c8, 0xd3fc, WFI],         // ADDS #1; CMP #200; BCC
        vec![0x3001, 0x4298, 0xd3fc, WFI],         // ADDS #1; CMP R3; BCC
        vec![0xf1b0, 0x0001, 0xf47f, 0xaffc, WFI], // SUBS.W #1; BNE.W
    ] {
        for budget in (1..40).chain([100, 101, 199, 200, 201, 399, 600]) {
            let accelerated = compare_thumb(&code, budget, 100, 0);
            if budget > 100 {
                assert!(accelerated, "{code:04x?} budget={budget}");
            }
        }
    }
    for value in [0, 1, 2, 0x7fff_ffff, 0x8000_0000, u32::MAX] {
        compare_thumb(&[0x3801, 0xd1fd, WFI], 101, value, 0);
    }
}
#[test]
fn thumb_stores_and_unsupported_instructions_never_accelerate() {
    for code in [
        vec![0x6811, 0x6051, 0xe7fc, WFI],         // LDR; STR; B
        vec![0x6811, 0xf8c2, 0x1004, 0xe7fb, WFI], // LDR; STR.W; B
        vec![0x6811, 0x4349, 0xe7fc, WFI],         // LDR; MULS; B
        vec![0x4800, 0xe7fd, WFI, WFI],            // LDR literal; B
        vec![0x6811, 0xbf08, 0x6051, 0xe7fb, WFI], // LDR; IT EQ; STREQ; B
    ] {
        for budget in [7, 200] {
            assert!(!compare_thumb(&code, budget, 1, 0), "{code:04x?}");
        }
        let on = thumb(&code, LoopMode::Accelerate, 200, 1, 0);
        assert!(on.counters["loop_proofs_failed_structural"] > 0, "{code:04x?}");
    }
}
