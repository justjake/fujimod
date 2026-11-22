//! A processor spinning on a stable device status word is charged for its
//! iterations without executing them, and everything the board and the
//! devices record matches the per-instruction hook at every scheduler
//! quantum.
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};
use std::sync::Mutex;

const CBAR: u32 = 0x2000_0000;
const CORE0: u32 = 0x1000;
const CORE1: u32 = 0x2000;
const POLL: u32 = 0x300;
const PACKETS: usize = 0x4000;
const DONE: usize = 0x3100;
const COUNT: usize = 0x3200;
const COUNTS: u32 = 3000;

// The environment is process-wide: sessions are opened one at a time.
static ENVIRONMENT: Mutex<()> = Mutex::new(());

fn movw(register: u32, value: u32) -> u32 {
    0xe300_0000 | ((value >> 12) << 16) | (register << 12) | (value & 0xfff)
}
fn movt(register: u32, value: u32) -> u32 {
    0xe340_0000 | ((value >> 12) << 16) | (register << 12) | (value & 0xfff)
}
fn str_r5(register: u32, offset: u32) -> u32 {
    0xe585_0000 | (register << 12) | offset
}
fn branch(condition: u32, from: u32, to: u32, link: bool) -> u32 {
    let words = (to.wrapping_sub(from + 8) as i32 >> 2) as u32 & 0x00ff_ffff;
    (condition << 28) | 0x0a00_0000 | (u32::from(link) << 24) | words
}

/// Core 0 programs one sensor command (a register write that completes after
/// its latency) and then calls a status routine in a loop while the command
/// is busy: a call, a push and a pop around one load of the busy word. Core 1
/// counts a RAM word. Both end in WFI.
fn program() -> Vec<(u32, Vec<u32>)> {
    let spin = CORE0 + 16 * 4;
    vec![
        (
            POLL,
            vec![
                0xe92d_4010, // push {r4, lr}
                0xe595_4204, // ldr r4, [r5, #0x204]
                0xe1a0_0004, // mov r0, r4
                0xe8bd_8010, // pop {r4, pc}
            ],
        ),
        (
            CORE0,
            vec![
                movw(13, 0x8000), // sp
                movw(5, 0xc000),
                movt(5, 0xfff6), // sensor command window
                movw(1, PACKETS as u32),
                str_r5(1, 0x210),
                movw(1, 0x0401),
                movt(1, 1),
                str_r5(1, 0x208), // one packet
                0xe3a0_1000,      // mov r1, #0
                str_r5(1, 0x20c),
                str_r5(1, 0x214),
                str_r5(1, 0x21c),
                0xe3a0_1008, // mov r1, #8
                str_r5(1, 0x218),
                0xe3a0_1001, // mov r1, #1
                str_r5(1, 0x204), // start
                branch(14, spin, POLL, true),
                0xe350_0001, // cmp r0, #1
                branch(0, spin + 8, spin, false),
                movw(2, DONE as u32),
                0xe3a0_3001, // mov r3, #1
                0xe582_3000, // str r3, [r2]
                0xe320_f003, // wfi
                0xeaff_fffd, // b wfi
            ],
        ),
        (
            CORE1,
            vec![
                movw(2, COUNT as u32),
                movw(4, COUNTS),
                0xe592_3000, // ldr r3, [r2]
                0xe283_3001, // add r3, r3, #1
                0xe582_3000, // str r3, [r2]
                0xe153_0004, // cmp r3, r4
                0xbaff_fffa, // blt loop
                0xe320_f003, // wfi
                0xeaff_fffd, // b wfi
            ],
        ),
    ]
}

fn session(counted: bool, command_ticks: u64) -> Session {
    let definition: Definition = serde_json::from_value(json!({
        "ram_size": 65536, "cores": 2, "discovery": false, "cbar": CBAR,
        "imaging": [
            [0xfff6c000u64, 0xfff69000u64, [300, 301], [302, 303], [304, 305], null],
            [1_000_000, 500_000, command_ticks, 100],
            [0, 0],
            null
        ],
        "initial_registers": [[["Pc", CORE0]], [["Pc", CORE1]]]
    }))
    .unwrap();
    session_of(counted, definition, program())
}

fn session_of(counted: bool, definition: Definition, program: Vec<(u32, Vec<u32>)>) -> Session {
    let session = {
        let _environment = ENVIRONMENT.lock().unwrap();
        // SAFETY: sessions in this binary are opened under the lock above.
        unsafe {
            if counted {
                std::env::set_var("FUJIMOD_XP4_COUNTED", "1");
            } else {
                std::env::remove_var("FUJIMOD_XP4_COUNTED");
            }
        }
        let session = Session::new(definition, None, vec![]).unwrap();
        unsafe { std::env::remove_var("FUJIMOD_XP4_COUNTED") };
        session
    };
    assert_eq!(session.counted_execution(), counted);
    for (address, words) in program {
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        session.write(address as usize, &bytes).unwrap();
    }
    session.write(PACKETS, &[0x81, 0x00, 0x10, 0x01, 0, 0, 0, 0]).unwrap();
    session
}

fn outcome(session: &Session) -> (Value, Vec<u8>) {
    let mut ram = vec![0; session.ram_bytes()];
    session.read(0, &mut ram).unwrap();
    (serde_json::to_value(session.state(true)).unwrap(), ram)
}

#[test]
fn polling_a_busy_status_word_matches_the_per_instruction_hook() {
    for command_ticks in [20_000u64, 20_001] {
        for quantum in [1, 7, 64, 1000, 20_000] {
            let mut outcomes = Vec::new();
            let mut polled = Vec::new();
            for counted in [false, true] {
                let session = session(counted, command_ticks);
                // Several board calls, so slices also end at call boundaries.
                for _ in 0..9 {
                    session.run_board(7000, quantum).unwrap();
                }
                let state = session.state(false);
                assert!(
                    state.failure.is_none(),
                    "quantum {quantum}, counted {counted}: {:?}",
                    state.failure
                );
                polled.push(session.polled_instructions());
                outcomes.push(outcome(&session));
            }
            let (exact, counted) = (&outcomes[0], &outcomes[1]);
            let word = |at: usize| u32::from_le_bytes(exact.1[at..at + 4].try_into().unwrap());
            assert_eq!(word(DONE), 1, "quantum {quantum}: the command never completed");
            assert_eq!(word(COUNT), COUNTS, "quantum {quantum}");
            assert_eq!(exact.0, counted.0, "state at quantum {quantum}");
            assert!(exact.1 == counted.1, "RAM at quantum {quantum}");
            assert_eq!(polled[0], 0, "the hook never skips");
            if quantum >= 1000 {
                assert!(polled[1] > 5000, "quantum {quantum}: skipped {}", polled[1]);
            }
        }
    }
}

/// Core 0 starts a device operation, then spins on a status word in a tight
/// load, test and branch loop until the device clears it, and reports done.
/// Core 1 counts a RAM word, as above.
fn spin_program(start: Vec<u32>, load: u32, test: u32) -> Vec<(u32, Vec<u32>)> {
    let mut core0 = start;
    let spin = CORE0 + 4 * core0.len() as u32;
    core0.extend([
        load,
        test,
        branch(1, spin + 8, spin, false), // bne
        movw(2, DONE as u32),
        0xe3a0_3001, // mov r3, #1
        0xe582_3000, // str r3, [r2]
        0xe320_f003, // wfi
        0xeaff_fffd, // b wfi
    ]);
    let mut program = program();
    program.retain(|(address, _)| *address == CORE1);
    program.push((CORE0, core0));
    program
}

fn assert_spin_matches_the_hook(definition: Value, program: Vec<(u32, Vec<u32>)>) {
    for quantum in [1, 7, 64, 1000, 20_000] {
        let mut outcomes = Vec::new();
        let mut polled = Vec::new();
        for counted in [false, true] {
            let definition: Definition = serde_json::from_value(definition.clone()).unwrap();
            let session = session_of(counted, definition, program.clone());
            for _ in 0..9 {
                session.run_board(7000, quantum).unwrap();
            }
            let state = session.state(false);
            assert!(state.failure.is_none(), "quantum {quantum}, counted {counted}: {:?}", state.failure);
            polled.push(session.polled_instructions());
            outcomes.push(outcome(&session));
        }
        let (exact, counted) = (&outcomes[0], &outcomes[1]);
        let word = |at: usize| u32::from_le_bytes(exact.1[at..at + 4].try_into().unwrap());
        assert_eq!(word(DONE), 1, "quantum {quantum}: the operation never completed");
        assert_eq!(word(COUNT), COUNTS, "quantum {quantum}");
        assert_eq!(exact.0, counted.0, "state at quantum {quantum}");
        assert!(exact.1 == counted.1, "RAM at quantum {quantum}");
        assert_eq!(polled[0], 0, "the hook never skips");
        if quantum >= 1000 {
            assert!(polled[1] > 500, "quantum {quantum}: skipped {}", polled[1]);
        }
    }
}

fn base_definition() -> Value {
    json!({
        "ram_size": 65536, "cores": 2, "discovery": false, "cbar": CBAR,
        "initial_registers": [[["Pc", CORE0]], [["Pc", CORE1]]]
    })
}

/// A control request word stays busy until its acknowledge event.
#[test]
fn polling_a_busy_request_word_matches_the_per_instruction_hook() {
    for ticks in [2000u64, 2001] {
        let mut definition = base_definition();
        definition["control_requests"] = json!([[0xff70_1080u64, ticks]]);
        let start = vec![
            movw(5, 0x1080),
            movt(5, 0xff70),
            0xe3a0_1002, // mov r1, #2
            str_r5(1, 0),
        ];
        // ldr r0, [r5]; cmp r0, #0
        assert_spin_matches_the_hook(definition, spin_program(start, 0xe595_0000, 0xe350_0000));
    }
}

/// An SPI word shifts for bits * divider ticks while its status bit 4 is set.
#[test]
fn polling_a_shifting_spi_status_word_matches_the_per_instruction_hook() {
    for divider in [100u32, 102] {
        let mut definition = base_definition();
        definition["spi_inputs"] = json!([[0xfff3_3000u64, false]]);
        let start = vec![
            movw(5, 0x3000),
            movt(5, 0xfff3),
            0xe3a0_1007, // mov r1, #7 (eight bits)
            str_r5(1, 0),
            0xe3a0_1000 | divider,
            str_r5(1, 0x10),
            0xe3a0_1002, // mov r1, #2 (enable)
            str_r5(1, 4),
            0xe3a0_1041, // mov r1, #0x41
            str_r5(1, 8), // transmit
        ];
        // ldr r0, [r5, #0xc]; tst r0, #0x10
        assert_spin_matches_the_hook(definition, spin_program(start, 0xe595_000c, 0xe310_0010));
    }
}

/// Host call sizes that end slices at different places: a call boundary cuts
/// the scheduler's slice, in both execution modes alike.
const CALLS: [&[u64]; 3] = [
    &[7000; 9],
    &[1, 6999, 2, 4001, 100, 12_345, 3, 9999, 5555, 20_000],
    &[63_000],
];

fn assert_plain_counted_matches_the_hook(definition: Value, program: Vec<(u32, Vec<u32>)>) {
    for calls in CALLS {
        for quantum in [1, 7, 64, 1000, 20_000] {
            let mut outcomes = Vec::new();
            for counted in [false, true] {
                let definition: Definition = serde_json::from_value(definition.clone()).unwrap();
                let session = session_of(counted, definition, program.clone());
                session.set_poll_fast_forward(false).unwrap();
                for &budget in calls {
                    session.run_board(budget, quantum).unwrap();
                }
                let state = session.state(false);
                assert!(state.failure.is_none(), "quantum {quantum}, counted {counted}: {:?}", state.failure);
                assert_eq!(session.polled_instructions(), 0, "plain counted never skips");
                outcomes.push(outcome(&session));
            }
            let (exact, counted) = (&outcomes[0], &outcomes[1]);
            assert_eq!(exact.0, counted.0, "state at quantum {quantum}, calls {calls:?}");
            assert!(exact.1 == counted.1, "RAM at quantum {quantum}, calls {calls:?}");
        }
    }
}

/// Without the polling fast-forward, counted execution cuts its internal
/// runs at device deadlines and still matches the hook at every quantum and
/// every sequence of host calls (the run boundaries a call makes are visible
/// to both modes alike, never to one).
#[test]
fn plain_counted_runs_cut_at_sensor_deadlines_match_the_hook() {
    for command_ticks in [20_000u64, 20_001] {
        let definition = json!({
            "ram_size": 65536, "cores": 2, "discovery": false, "cbar": CBAR,
            "imaging": [
                [0xfff6c000u64, 0xfff69000u64, [300, 301], [302, 303], [304, 305], null],
                [1_000_000, 500_000, command_ticks, 100],
                [0, 0],
                null
            ],
            "initial_registers": [[["Pc", CORE0]], [["Pc", CORE1]]]
        });
        assert_plain_counted_matches_the_hook(definition, program());
    }
}

#[test]
fn plain_counted_runs_cut_at_spi_deadlines_match_the_hook() {
    for divider in [100u32, 102] {
        let mut definition = base_definition();
        definition["spi_inputs"] = json!([[0xfff3_3000u64, false]]);
        let start = vec![
            movw(5, 0x3000),
            movt(5, 0xfff3),
            0xe3a0_1007, // mov r1, #7 (eight bits)
            str_r5(1, 0),
            0xe3a0_1000 | divider,
            str_r5(1, 0x10),
            0xe3a0_1002, // mov r1, #2 (enable)
            str_r5(1, 4),
            0xe3a0_1041, // mov r1, #0x41
            str_r5(1, 8), // transmit
        ];
        assert_plain_counted_matches_the_hook(
            definition,
            spin_program(start, 0xe595_000c, 0xe310_0010),
        );
    }
}

/// Both cores run board instructions that leave a slice running (an MPIDR
/// read, LDREX/STREX, CLREX, SEV and WFE with an event pending) in a loop:
/// the per-instruction hook and counted execution agree at every quantum.
fn board_instruction_loop(base: u32) -> Vec<u32> {
    let mut code = vec![
        movw(2, 0x3400),
        0xe3a0_4000, // mov r4, #0
    ];
    let top = base + 4 * code.len() as u32;
    let at = |code: &Vec<u32>| base + 4 * code.len() as u32;
    code.extend([
        0xee10_0fb0, // mrc p15, 0, r0, c0, c0, 5
        0xe200_0003, // and r0, r0, #3
        0xe082_1100, // add r1, r2, r0, lsl #2
        0xe282_6020, // add r6, r2, #0x20
    ]);
    let retry = at(&code);
    code.extend([
        0xe196_3f9f, // ldrex r3, [r6]
        0xe283_3001, // add r3, r3, #1
        0xe186_7f93, // strex r7, r3, [r6]
        0xe357_0000, // cmp r7, #0
    ]);
    code.push(branch(1, at(&code), retry, false));
    code.extend([
        0xf57f_f01f, // clrex
        0xe320_f004, // sev
        0xe320_f002, // wfe
        0xe591_8000, // ldr r8, [r1]
        0xe288_8001, // add r8, r8, #1
        0xe581_8000, // str r8, [r1]
        0xe284_4001, // add r4, r4, #1
        0xe354_0c01, // cmp r4, #0x100
    ]);
    code.push(branch(11, at(&code), top, false)); // blt
    code.extend([0xe320_f003, 0xeaff_fffd]); // wfi; b wfi
    code
}

#[test]
fn board_instructions_that_leave_the_slice_running_match_the_hook() {
    for quantum in [1, 7, 64, 1000, 20_000] {
        let mut outcomes = Vec::new();
        for counted in [false, true] {
            let definition: Definition = serde_json::from_value(base_definition()).unwrap();
            let program = vec![
                (CORE0, board_instruction_loop(CORE0)),
                (CORE1, board_instruction_loop(CORE1)),
            ];
            let session = session_of(counted, definition, program);
            for _ in 0..6 {
                session.run_board(3000, quantum).unwrap();
            }
            let state = session.state(false);
            assert!(state.failure.is_none(), "quantum {quantum}, counted {counted}: {:?}", state.failure);
            outcomes.push(outcome(&session));
        }
        let word = |at: usize| u32::from_le_bytes(outcomes[0].1[at..at + 4].try_into().unwrap());
        assert!(word(0x3420) > 100, "quantum {quantum}: shared counter {}", word(0x3420));
        assert!(word(0x3400) > 10 && word(0x3404) > 10, "quantum {quantum}: each core ran");
        assert_eq!(outcomes[0].0, outcomes[1].0, "state at quantum {quantum}");
        assert!(outcomes[0].1 == outcomes[1].1, "RAM at quantum {quantum}");
    }
}

/// Once the other core waits, the spinning core is alone in the scheduler's
/// window and a proven poll skips across slice ends. The skip must stop where
/// the hook's slices would, whatever the quantum and the host call sizes.
#[test]
fn polling_alone_across_slice_ends_matches_the_hook_at_odd_call_sizes() {
    for command_ticks in [20_000u64, 20_001] {
        for calls in CALLS {
            for quantum in [1, 7, 64, 1000, 4097, 20_000] {
                let mut outcomes = Vec::new();
                let mut polled = Vec::new();
                for counted in [false, true] {
                    let session = session(counted, command_ticks);
                    for &budget in calls {
                        session.run_board(budget, quantum).unwrap();
                    }
                    let state = session.state(false);
                    assert!(
                        state.failure.is_none(),
                        "quantum {quantum}, counted {counted}: {:?}",
                        state.failure
                    );
                    polled.push(session.polled_instructions());
                    outcomes.push(outcome(&session));
                }
                let (exact, counted) = (&outcomes[0], &outcomes[1]);
                assert_eq!(exact.0, counted.0, "state at quantum {quantum}, calls {calls:?}");
                assert!(exact.1 == counted.1, "RAM at quantum {quantum}, calls {calls:?}");
                assert_eq!(polled[0], 0, "the hook never skips");
                if quantum >= 64 && calls.iter().sum::<u64>() > 60_000 {
                    assert!(polled[1] > 5000, "quantum {quantum}, calls {calls:?}: skipped {}", polled[1]);
                }
            }
        }
    }
}
