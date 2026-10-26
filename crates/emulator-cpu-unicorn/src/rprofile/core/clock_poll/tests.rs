use super::super::tests::{get, processor_with_options, set, words};
use super::*;

fn register(crm: u32, opc2: u32) -> arm::Coprocessor {
    arm::Coprocessor {
        cp: 15,
        opc1: 0,
        crn: 9,
        crm,
        opc2,
        secure: 0,
        wide: false,
    }
}
fn setup(
    policy: ExecutionPolicy,
    mode: LoopMode,
    phase: u64,
    compare: u32,
    frozen: bool,
    value: u32,
) -> Core {
    let cpu = processor_with_options(&Memory::default(), Options::default());
    cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
    words(&cpu, 0x1000, &[0xee190f1d, compare, 0x3afffffc, 0xe320f003]);
    set(&cpu, arm::CPSR, 0x900000d3);
    set(&cpu, arm::CORE[1], 20);
    cpu.set_execution_policy(policy).unwrap();
    cpu.set_loop_mode(mode).unwrap();
    cpu.set_guest_clock(Clock {
        now: 100,
        phase,
        ratio: timeline::Ratio::new(24, 1).unwrap(),
    })
    .unwrap();
    cpu.coprocessor(register(12, 0), Some(1)).unwrap();
    cpu.coprocessor(register(12, 1), Some(if frozen { 0 } else { 0x80000000 }))
        .unwrap();
    cpu.coprocessor(register(13, 0), Some(u64::from(value)))
        .unwrap();
    cpu
}
fn execute(cpu: &Core, begin: u64, budget: usize) {
    cpu.set_loop_window(true).unwrap();
    cpu.run(Run {
        begin,
        until: None,
        watchdog_us: 0,
        instructions: Some(budget),
    })
    .unwrap();
}
fn state(cpu: &Core) -> (serde_json::Value, u64) {
    (
        cpu.fingerprint().unwrap(),
        cpu.instruction_progress().unwrap(),
    )
}

#[test]
fn clock_poll_matches_every_budget_boundary_flags_and_retained_counter_state() {
    for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
        for phase in [0, 7, 23] {
            for compare in [0xe3500014, 0xe1500001] {
                for budget in [
                    1, 2, 3, 4, 5, 6, 7, 17, 64, 470, 471, 472, 473, 474, 475, 480, 481, 1000,
                ] {
                    let off = setup(policy, LoopMode::Disabled, phase, compare, false, 0);
                    let on = setup(policy, LoopMode::Accelerate, phase, compare, false, 0);
                    execute(&off, 0x1000, budget);
                    execute(&on, 0x1000, budget);
                    assert_eq!(
                        state(&on),
                        state(&off),
                        "{policy:?} phase={phase} compare={compare:x} budget={budget}"
                    );
                }
            }
        }
    }
}

#[test]
fn frozen_and_wrapping_counters_preserve_the_complete_pmu_state() {
    for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
        for (frozen, value) in [(true, 0), (true, 20), (false, u32::MAX - 2)] {
            let off = setup(policy, LoopMode::Disabled, 7, 0xe3500014, frozen, value);
            let on = setup(policy, LoopMode::Accelerate, 7, 0xe3500014, frozen, value);
            execute(&off, 0x1000, 1000);
            execute(&on, 0x1000, 1000);
            assert_eq!(state(&on), state(&off));
        }
    }
}

#[test]
fn clock_poll_rechecks_code_after_pauses_and_counts_accelerated_instructions() {
    let off = setup(
        ExecutionPolicy::Interactive,
        LoopMode::Disabled,
        7,
        0xe3500014,
        false,
        0,
    );
    let on = setup(
        ExecutionPolicy::Interactive,
        LoopMode::Accelerate,
        7,
        0xe3500014,
        false,
        0,
    );
    for cpu in [&off, &on] {
        execute(cpu, 0x1000, 120);
        let clock = cpu
            .execution
            .clock
            .get()
            .unwrap()
            .after(cpu.instruction_progress().unwrap())
            .unwrap();
        cpu.set_guest_clock(clock).unwrap();
        words(cpu, 0x1004, &[0xe3500008]);
        execute(cpu, u64::from(get(cpu, arm::PC)), 1000);
    }
    assert_eq!(state(&on), state(&off));
    assert!(on.exit_counters().unwrap()["loop_clock_poll_instructions"] > 0);
}

#[test]
fn instruction_observers_and_explicit_address_stops_keep_every_iteration() {
    unsafe fn observed(_: u64, _: u32, data: *mut c_void) {
        let count = unsafe { &*data.cast::<Cell<usize>>() };
        count.set(count.get() + 1);
    }
    for observer in [true, false] {
        let cpu = setup(
            ExecutionPolicy::Exact,
            LoopMode::Accelerate,
            0,
            0xe3500014,
            false,
            0,
        );
        let hits = Cell::new(0usize);
        if observer {
            unsafe {
                cpu.install_hook(Hook::Instruction {
                    callback: observed,
                    data: (&hits as *const Cell<usize>).cast_mut().cast(),
                    range: HookRange::Inclusive {
                        begin: 0x1004,
                        end: 0x1004,
                    },
                })
            }
            .unwrap();
        }
        cpu.set_loop_window(true).unwrap();
        cpu.run(Run {
            begin: 0x1000,
            until: (!observer).then_some(0x1004),
            watchdog_us: 0,
            instructions: Some(1000),
        })
        .unwrap();
        assert_eq!(
            cpu.exit_counters().unwrap()["loop_clock_poll_instructions"],
            0
        );
        if observer {
            assert!(hits.get() > 100);
        } else {
            assert_eq!(get(&cpu, arm::PC), 0x1004);
        }
    }
}

#[test]
fn signed_compare_flags_wrap_windows_and_measurement_mode_match_execution() {
    for (frozen, initial, limit, mode) in [
        (true, 0x7fffffff, 0x80000010, LoopMode::Accelerate),
        (false, u32::MAX - 2, u32::MAX, LoopMode::Accelerate),
        (false, 0, 20, LoopMode::Measure),
    ] {
        let off = setup(
            ExecutionPolicy::Exact,
            LoopMode::Disabled,
            7,
            0xe1500001,
            frozen,
            initial,
        );
        let on = setup(ExecutionPolicy::Exact, mode, 7, 0xe1500001, frozen, initial);
        for cpu in [&off, &on] {
            set(cpu, arm::CORE[1], limit);
            execute(cpu, 0x1000, 1000);
        }
        assert_eq!(state(&on), state(&off));
        if mode == LoopMode::Measure || initial == u32::MAX - 2 {
            assert_eq!(
                on.exit_counters().unwrap()["loop_clock_poll_instructions"],
                0
            );
        }
    }
}

#[test]
fn lookahead_does_not_skip_instruction_fetch_permissions() {
    let off = setup(
        ExecutionPolicy::Exact,
        LoopMode::Disabled,
        0,
        0xe3500014,
        false,
        0,
    );
    let on = setup(
        ExecutionPolicy::Exact,
        LoopMode::Accelerate,
        0,
        0xe3500014,
        false,
        0,
    );
    for cpu in [&off, &on] {
        cpu.map_zeroed(0x2000, 0x1000, 3).unwrap();
        words(cpu, 0x1ffc, &[0xee190f1d, 0xe3500014, 0x3afffffc]);
        cpu.set_loop_window(true).unwrap();
        let error = cpu
            .run(Run {
                begin: 0x1ffc,
                until: None,
                watchdog_us: 0,
                instructions: Some(1000),
            })
            .unwrap_err();
        assert!(error.to_string().contains("non-executable"));
    }
    assert_eq!(state(&on), state(&off));
    assert_eq!(
        on.exit_counters().unwrap()["loop_clock_poll_instructions"],
        0
    );
}
