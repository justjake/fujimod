#![cfg(not(target_os = "emscripten"))]

use crate::board::{
    Board,
    bus::{self, Bus},
    memory::BusMapping,
    set,
};
use fujimod_emulator_cpu::{Cpu, ExecutionPolicy, Register, memory::Backing, timeline::Ratio};
use fujimod_emulator_cpu_unicorn::{memory as cpu_memory, worker};
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint};
use serde::{Deserialize, Serialize};

const MAX_INTERACTIVE_WINDOWS: u64 = 64;

#[derive(Serialize, Deserialize)]
struct DeadlineSample {
    due: Option<u64>,
    armed_at: Option<u64>,
    samples: Vec<(u64, u32)>,
}

impl Device for DeadlineSample {
    fn name(&self) -> &'static str {
        "interactive-deadline-sample"
    }

    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), self)
    }

    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let _: Self = saved.decode(self.name())?;
        Ok(())
    }

    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        *self = saved.decode(self.name())?;
        Ok(())
    }

    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(self).map_err(|error| error.to_string())
    }

    fn read(&mut self, _: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        u64::from(!self.samples.is_empty())
    }

    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, value: u64) {
        self.armed_at = Some(cx.now());
        match cx.now().checked_add(value) {
            Some(due) => self.due = Some(due),
            None => cx.fail("sample deadline overflow"),
        }
    }

    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.due
    }

    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.due.is_some_and(|due| due <= cx.now()) {
            self.due = None;
            let mut bytes = [0; 4];
            if let Err(error) = cx.ram_read(0x3000, &mut bytes) {
                cx.fail(error);
                return;
            }
            self.samples.push((cx.now(), u32::from_le_bytes(bytes)));
        }
    }
}

fn board(policy: ExecutionPolicy, due: Option<u64>) -> (Board, usize) {
    let backing = Backing::aligned_zeros(0x4000, 4096).unwrap();
    let shared_memory = cpu_memory::Memory::default().shared();
    let cpus = (0..2)
        .map(|core| {
            let cpu = worker::spawn(worker::Config {
                translation_cache_size_bytes: 1024 * 1024,
                options: Default::default(),
                memory: shared_memory.clone(),
            })
            .unwrap();
            unsafe {
                cpu.map_physical(
                    0,
                    0,
                    0x4000,
                    7,
                    std::ptr::NonNull::new(backing.as_ptr()).unwrap(),
                )
                .unwrap();
            }
            set(&cpu, Register::Pc, 0x1000 + core * 0x1000).unwrap();
            set(&cpu, Register::Pstate, 0xd3).unwrap();
            Box::new(cpu) as Box<dyn fujimod_emulator_cpu::Cpu>
        })
        .collect();
    let mut bus = Bus::new(bus::Mode::Strict);
    let device = bus
        .register(
            0x8000000,
            0x1000,
            Box::new(DeadlineSample {
                due,
                armed_at: None,
                samples: Vec::new(),
            }),
        )
        .unwrap();
    let mut board = Board::new(
        cpus,
        bus,
        Ratio::new(1, 1).unwrap(),
        None,
        vec![true; 2],
        crate::board::memory::Memory::new(vec![(0, backing)], vec![BusMapping::ram(0, 0x4000)])
            .unwrap(),
    )
    .unwrap();
    for core in 0..2 {
        board.map_mmio(core, 0x8000000, 0x1000, 0x8000000).unwrap();
    }
    let code = [0xe3a0005au32, 0xe3a01a03, 0xe5810000, 0xeafffffe];
    board
        .write_memory(
            0,
            0x1000,
            &code
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    // Both processors complete the same store so the sample does not depend
    // on which worker reaches the owner barrier first.
    board
        .write_memory(
            0,
            0x2000,
            &code
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    if policy != ExecutionPolicy::Exact {
        board.set_execution_policy(policy).unwrap();
    }
    assert!(
        board
            .cpus
            .iter()
            .all(|cpu| cpu.features().asynchronous_runs)
    );
    (board, device)
}

fn samples(board: &Board, device: usize) -> Vec<(u64, u32)> {
    board
        .inspect_device::<DeadlineSample, _>(device, |sample| Ok(sample.samples.clone()))
        .unwrap()
}

#[test]
fn deadline_samples_policy_boundary_and_checkpoint_restores_the_observed_state() {
    for (policy, expected) in [
        (ExecutionPolicy::Exact, 0),
        (ExecutionPolicy::Interactive, 0x5a),
    ] {
        let (mut board, device) = board(policy, Some(2));
        board.run_until(2, 16).unwrap();

        assert_eq!(board.state()["timeline"]["now"], 2);
        assert_eq!(samples(&board, device), [(2, expected)]);
        let before = board.complete_state().unwrap();
        let saved = board.checkpoint(None).unwrap();

        board
            .write_physical(0x3000, &0xdead_beefu32.to_le_bytes())
            .unwrap();
        board.restore_keyframe(&saved).unwrap();
        assert_eq!(board.complete_state().unwrap(), before);
        assert_eq!(samples(&board, device), [(2, expected)]);

        board.run_until(4, 16).unwrap();
        assert_eq!(samples(&board, device), [(2, expected)]);
    }
}

#[test]
fn interactive_instruction_limit_allows_only_bounded_block_overshoot() {
    let (mut board, _) = board(ExecutionPolicy::Interactive, None);
    board.run_for_instructions(2, 16).unwrap();

    let retired = board.instruction_counts().into_iter().sum::<u64>();
    assert!(
        retired > 2,
        "interactive execution may retire a whole block"
    );
    assert!(
        retired <= 8,
        "two one-instruction shares can each overrun by at most one four-instruction block; retired {retired}"
    );
}

fn check_mmio_progress(policy: ExecutionPolicy, tick_limit: bool) {
    let (mut board, device) = board(policy, tick_limit.then_some(100_000));
    // Core 0 reads the immediate MMIO peripheral 64 times, then publishes
    // a flag. Core 1 only spins. Host callback latency is not guest work
    // and must not let the spinner consume each newly divided remainder.
    let code = [
        0xe3a00040u32, // mov r0, #64
        0xe59f1018,    // ldr r1, [pc, #24]
        0xe5912000,    // ldr r2, [r1]
        0xe2500001,    // subs r0, r0, #1
        0x1afffffc,    // bne to the MMIO load
        0xe3a03a03,    // mov r3, #0x3000
        0xe3a0205a,    // mov r2, #0x5a
        0xe5832000,    // str r2, [r3]
        0xeafffffe,    // b .
        0x08000000,
    ];
    board
        .write_memory(
            0,
            0x1000,
            &code
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    board
        .write_memory(0, 0x2000, &0xeafffffeu32.to_le_bytes())
        .unwrap();

    // Start the spinner first so callback arrival order cannot grant it
    // a fresh allowance at the expense of the MMIO core.
    board.next_core.set(1);
    let instruction_budget: u64 = if tick_limit { 100_000 } else { 200_000 };
    // Exact cores advance time in parallel; at a tick limit the peer that did
    // not reach it lags by up to one quantum.
    let share = 100_000;
    let lag = if policy == ExecutionPolicy::Exact && tick_limit {
        1000
    } else {
        0
    };
    let mut windows = 0;
    let flag = loop {
        let next_window = windows + 1;
        if tick_limit {
            board.run_until(100_000 * next_window, 1000).unwrap();
        } else {
            board
                .run_for_instructions(instruction_budget, 1000)
                .unwrap();
        }
        windows = next_window;

        let counts = board.instruction_counts();
        if policy != ExecutionPolicy::Interactive {
            assert!(
                counts
                    .iter()
                    .all(|&count| (share - lag..=share + 256).contains(&count)),
                "{policy:?}: callback restarts must retain each core's share: {counts:?}"
            );
        }

        let mut bytes = [0; 4];
        board.read_memory(0, 0x3000, &mut bytes).unwrap();
        let flag = u32::from_le_bytes(bytes);
        if flag == 0x5a {
            break flag;
        }
        if policy != ExecutionPolicy::Interactive {
            assert_eq!(
                flag, 0x5a,
                "{policy:?}: MMIO work did not complete: {counts:?}"
            );
        }
        assert!(
            windows < MAX_INTERACTIVE_WINDOWS,
            "Interactive MMIO work did not complete within {MAX_INTERACTIVE_WINDOWS} windows: flag={flag:#x}, counts={counts:?}"
        );
    };
    assert_eq!(flag, 0x5a, "{policy:?}: MMIO work did not complete");
    let tick = board.state.borrow().timeline.now();
    if tick_limit {
        assert_eq!(tick, 100_000 * windows);
        assert_eq!(
            samples(&board, device)
                .iter()
                .map(|&(at, _)| at)
                .collect::<Vec<_>>(),
            [100_000]
        );
    }
    let before = board.complete_state().unwrap();
    let saved = board.checkpoint(None).unwrap();
    board.run_for_instructions(1000, 1000).unwrap();
    board.restore_keyframe(&saved).unwrap();
    assert_eq!(board.complete_state().unwrap(), before);
}

#[test]
fn spinning_peer_and_mmio_work_follow_the_selected_execution_policy() {
    for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
        check_mmio_progress(policy, false);
    }
}

#[test]
fn device_deadline_window_respects_peer_exhaustion_after_mmio_stops() {
    for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
        check_mmio_progress(policy, true);
    }
}

#[test]
fn mmio_created_deadline_survives_callback_restarts_and_checkpoint_restore() {
    let (mut board, device) = board(ExecutionPolicy::Interactive, None);
    // Arm a real one-shot event after 32 ticks, then keep taking MMIO reads.
    let code = [
        0xe3a00020u32, // mov r0, #32
        0xe3a01302,    // mov r1, #0x08000000
        0xe5810000,    // str r0, [r1]
        0xe5912000,    // ldr r2, [r1]
        0xeafffffd,    // b to the MMIO load
    ];
    board
        .write_memory(
            0,
            0x1000,
            &code
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    board
        .write_memory(0, 0x2000, &0xeafffffeu32.to_le_bytes())
        .unwrap();
    board.run_until(10_000, 1000).unwrap();
    let armed = board
        .inspect_device::<DeadlineSample, _>(device, |sample| Ok(sample.armed_at.unwrap()))
        .unwrap();
    // A very late first callback can arm beyond this host boundary. Continue
    // to that actual device deadline, without inventing a polling deadline.
    board.run_until(armed + 32, 1000).unwrap();
    assert_eq!(samples(&board, device), [(armed + 32, 0)]);
    let saved = board.checkpoint(None).unwrap();
    let before = board.complete_state().unwrap();
    board.run_until(20_000, 1000).unwrap();
    board.restore_keyframe(&saved).unwrap();
    assert_eq!(board.complete_state().unwrap(), before);
    assert_eq!(samples(&board, device), [(armed + 32, 0)]);
}

#[test]
fn serviced_deadline_precedes_interactive_mmio_catch_up() {
    let (mut board, device) = board(ExecutionPolicy::Interactive, Some(100_000));
    let counter = |crm, opc2| fujimod_emulator_cpu::arm::Coprocessor {
        cp: 15,
        opc1: 0,
        crn: 9,
        crm,
        opc2,
        secure: 0,
        wide: false,
    };
    let cpu = board.cpu(0).unwrap();
    cpu.set_guest_clock(fujimod_emulator_cpu::Clock {
        now: 0,
        phase: 0,
        ratio: Ratio::new(1, 1).unwrap(),
    })
    .unwrap();
    cpu.coprocessor(counter(12, 0), Some(1)).unwrap();
    cpu.coprocessor(counter(12, 1), Some(0x80000000)).unwrap();
    let code = [
        0xe59f1028u32, // ldr r1, address literal
        0xe3a03a03,    // mov r3, #0x3000
        0xee194f1d,    // mrc pmccntr, r4
        0xe5834004,    // str r4, [r3, #4]
        0xe5912000,    // ldr r2, [r1]: 1 after the event
        0xe3520000,    // cmp r2, #0
        0x0afffffa,    // beq to the PMU read
        0xee194f1d,    // mrc pmccntr, r4
        0xe5834008,    // str r4, [r3, #8]
        0xe5832000,    // str r2, [r3]
        0xeafffffe,    // b .
        0xe1a00000,    // padding
        0x08000000,
    ];
    board
        .write_memory(
            0,
            0x1000,
            &code
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    board
        .write_memory(0, 0x2000, &0xeafffffeu32.to_le_bytes())
        .unwrap();
    board.next_core.set(1);
    let window_budget = 100_064;
    let mut window = 1;
    let words = loop {
        board.run_until(window_budget * window, 1000).unwrap();
        // The event samples RAM before the successful read can publish its flag.
        assert_eq!(samples(&board, device), [(100_000, 0)]);
        let mut bytes = [0; 12];
        board.read_physical(0x3000, &mut bytes).unwrap();
        let words: Vec<_> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect();
        if words[0] == 1 {
            break words;
        }
        assert!(
            window < MAX_INTERACTIVE_WINDOWS,
            "MMIO core did not finish within {MAX_INTERACTIVE_WINDOWS} windows: counts={:?}, words={words:?}",
            board.instruction_counts()
        );
        window += 1;
    };
    assert!(words[1] <= words[2], "local PMU moved backward: {words:?}");
    assert!(
        words[2] <= 100_256,
        "local PMU counted restarts twice: {words:?}"
    );
}

/// Three cores over shared RAM `0..0x8000`, without devices: an initiator
/// at `0x1000` and two responders at `0x2000` whose flag address is in r6.
fn rendezvous_board() -> Board {
    let backing = Backing::aligned_zeros(0x8000, 4096).unwrap();
    let shared_memory = cpu_memory::Memory::default().shared();
    let cpus = (0..3)
        .map(|core| {
            let cpu = worker::spawn(worker::Config {
                translation_cache_size_bytes: 1024 * 1024,
                options: Default::default(),
                memory: shared_memory.clone(),
            })
            .unwrap();
            unsafe {
                cpu.map_physical(
                    0,
                    0,
                    0x8000,
                    7,
                    std::ptr::NonNull::new(backing.as_ptr()).unwrap(),
                )
                .unwrap();
            }
            set(&cpu, Register::Pc, if core == 0 { 0x1000 } else { 0x2000 }).unwrap();
            set(&cpu, Register::Pstate, 0xd3).unwrap();
            set(
                &cpu,
                Register::Arm32(fujimod_emulator_cpu::arm::Register::R(6)),
                0x4000 + 4 * core as u64,
            )
            .unwrap();
            Box::new(cpu) as Box<dyn fujimod_emulator_cpu::Cpu>
        })
        .collect();
    Board::new(
        cpus,
        Bus::new(bus::Mode::Strict),
        Ratio::new(1, 1).unwrap(),
        None,
        vec![true; 3],
        crate::board::memory::Memory::new(vec![(0, backing)], vec![BusMapping::ram(0, 0x8000)])
            .unwrap(),
    )
    .unwrap()
}

/// A flag rendezvous of the shape firmware can use for cross-core cache
/// maintenance. The initiator clears every flag, announces a round, works, sets its own flag and waits until all flags are set; each
/// responder works, sets its flag once per round and waits until all flags
/// are set. Only the next round clears a flag. If the initiator sees the last
/// flag, leaves and clears the flags before another responder has read them,
/// that responder spins forever on a flag it already set. Whether that
/// happens depends only on how far one core may run while another does not.
/// Returns the rounds each responder completed and the three PCs.
fn rendezvous_rounds(quantum: usize, instructions: u64) -> ([u32; 2], [u64; 3]) {
    let mut board = rendezvous_board();
    let initiator = [
        0xe3a04a04u32, // mov r4, #0x4000: flags 0..2, round at +16
        0xe3a00000,    // round: mov r0, #0
        0xe5840004,    // str r0, [r4, #4]: clear the responders' flags
        0xe5840008,    // str r0, [r4, #8]
        0xe5840000,    // str r0, [r4]: and its own
        0xe5941010,    // ldr r1, [r4, #16]
        0xe2811001,    // add r1, r1, #1
        0xe5841010,    // str r1, [r4, #16]: announce the round
        0xe3a02040,    // mov r2, #64: work before its own flag
        0xe2522001,    // subs r2, r2, #1
        0x1afffffd,    // bne to subs
        0xe3a00001,    // mov r0, #1
        0xe5840000,    // str r0, [r4]: its own flag
        0xe5943000,    // wait: ldr r3, [r4]
        0xe3530000,    // cmp r3, #0
        0x0afffffc,    // beq wait
        0xe5943004,    // ldr r3, [r4, #4]
        0xe3530000,    // cmp r3, #0
        0x0afffff9,    // beq wait (restart at flag 0)
        0xe5943008,    // ldr r3, [r4, #8]
        0xe3530000,    // cmp r3, #0
        0x0afffff6,    // beq wait
        0xe3a02010,    // mov r2, #16: path to the next round's clear
        0xe2522001,    // subs r2, r2, #1
        0x1afffffd,    // bne to subs
        0xeaffffe6,    // b round
    ];
    let responder = [
        0xe3a04a04u32, // mov r4, #0x4000
        0xe3a05000,    // mov r5, #0: last round seen
        0xe5941010,    // idle: ldr r1, [r4, #16]
        0xe1510005,    // cmp r1, r5
        0x0afffffc,    // beq idle
        0xe1a05001,    // mov r5, r1
        0xe3a02040,    // mov r2, #64: work before its own flag
        0xe2522001,    // subs r2, r2, #1
        0x1afffffd,    // bne to subs
        0xe3a00001,    // mov r0, #1
        0xe5860000,    // str r0, [r6]: its own flag
        0xe5943000,    // wait: ldr r3, [r4]
        0xe3530000,    // cmp r3, #0
        0x0afffffc,    // beq wait
        0xe5943004,    // ldr r3, [r4, #4]
        0xe3530000,    // cmp r3, #0
        0x0afffff9,    // beq wait (restart at flag 0)
        0xe5943008,    // ldr r3, [r4, #8]
        0xe3530000,    // cmp r3, #0
        0x0afffff6,    // beq wait
        0xe5963020,    // ldr r3, [r6, #32]
        0xe2833001,    // add r3, r3, #1
        0xe5863020,    // str r3, [r6, #32]: rounds completed
        0xeaffffe9,    // b idle
    ];
    let bytes = |words: &[u32]| {
        words
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect::<Vec<_>>()
    };
    board.write_memory(0, 0x1000, &bytes(&initiator)).unwrap();
    board.write_memory(0, 0x2000, &bytes(&responder)).unwrap();
    board.run_for_instructions(instructions, quantum).unwrap();
    let completed = |core: u64| {
        let mut word = [0; 4];
        board.read_memory(0, 0x4020 + 4 * core, &mut word).unwrap();
        u32::from_le_bytes(word)
    };
    let pc = |core| board.read_register(core, Register::Pc).unwrap();
    ([completed(1), completed(2)], [pc(0), pc(1), pc(2)])
}

#[test]
fn flag_rendezvous_continues_with_short_turns_and_livelocks_when_one_core_runs_a_round_ahead() {
    // Turns shorter than the initiator's path from its wait to the next
    // clear (about 40 instructions): every responder reads the last flag.
    let (early, _) = rendezvous_rounds(8, 100_000);
    let (later, _) = rendezvous_rounds(8, 200_000);
    assert!(
        early.iter().all(|&rounds| rounds > 50) && later[0] > early[0] && later[1] > early[1],
        "{early:?} then {later:?}"
    );

    // With 1000-instruction turns the last responder to set its flag (core
    // 2) is followed by the initiator, which leaves, clears the flags and
    // starts the next round before core 1 runs again. Core 1 never completes
    // that round; every core then waits in its flag loop.
    let (stalled, pcs) = rendezvous_rounds(1000, 100_000);
    let (still, _) = rendezvous_rounds(1000, 200_000);
    assert_eq!(
        stalled, still,
        "rounds continued with a 1000-instruction quantum"
    );
    assert_eq!(stalled[0], 0, "{stalled:?}");
    assert!((0x1034..=0x1054).contains(&pcs[0]), "{:#x}", pcs[0]);
    assert!(
        pcs[1..].iter().all(|pc| (0x202c..=0x204c).contains(pc)),
        "{pcs:x?}"
    );
}

#[test]
fn cohort_ends_when_its_first_core_reaches_the_limit() {
    let clock = fujimod_emulator_cpu::Clock {
        now: 1000,
        phase: 0,
        ratio: Ratio::new(1, 1).unwrap(),
    };
    let mut epoch = super::Epoch::new(3, clock, 100, fujimod_perf::EngineEndReason::OtherExit);
    let cohort = [0, 1, 2];
    assert!(epoch.open(&cohort));
    // Owner callbacks split the epoch into segments; each core keeps its
    // remaining share while every core has some left.
    assert_eq!(epoch.advance(1, 30).unwrap(), 30);
    assert_eq!(epoch.advance(2, 60).unwrap(), 30);
    assert!(epoch.open(&cohort));
    assert_eq!((epoch.room(0), epoch.room(1), epoch.room(2)), (100, 70, 40));
    // Core 2 reaches the limit. Cores 0 and 1 still have room, but they
    // would run alone against core 2's stores, so the cohort ends and time
    // advances to the furthest core only.
    assert_eq!(epoch.advance(2, 40).unwrap(), 40);
    assert_eq!(epoch.advance(0, 10).unwrap(), 0);
    assert!(!epoch.open(&cohort));
    assert_eq!(epoch.clock(2).unwrap().now, 1100);
    // A core outside the cohort (waiting or not released) does not end it.
    assert!(epoch.open(&[0, 1]));
}

const M4: usize = 4;
const MMIO: u64 = 0x0800_0000;

fn mixed_board(policy: ExecutionPolicy, bus: Bus, m4_registers: &[(Register, u64)]) -> Board {
    use crate::board::{factory, mprofile};
    use fujimod_catalog_configuration::RunEngine;
    use fujimod_emulator_cpu::{Model, arm};

    let processors = (0..5)
        .map(|core| {
            let (model, registers, mprofile) = if core == M4 {
                (
                    Model::CortexM4,
                    vec![
                        (Register::Pc, 0x1000),
                        (arm::CORE[13], 0x3f00),
                        (Register::Arm32(arm::Register::Xpsr), 1 << 24),
                    ],
                    Some(mprofile::Config {
                        clock_hz: 1_000_000,
                        board_ticks_per_second: 1_000_000,
                        vector_base: 0,
                    }),
                )
            } else {
                (
                    Model::ArmV8A,
                    vec![
                        (Register::Pc, 0x100 + core as u64 * 0x100),
                        (Register::Pstate, 0x3c5),
                    ],
                    None,
                )
            };
            let mut registers = registers;
            if core == M4 {
                registers.extend_from_slice(m4_registers);
            }
            factory::Processor {
                cpu_backend: Default::default(),
                model,
                registers,
                released: true,
                parked_until_sgi: false,
                mprofile,
                rprofile: None,
            }
        })
        .collect();
    let mut board = factory::Graph {
        processors,
        ram: vec![factory::Ram {
            base: 0,
            size: 0x4000,
        }],
        dma_mappings: vec![BusMapping::ram(0, 0x4000)],
        decoders: vec![],
        mappings: (0..5)
            .map(|core| factory::MemoryMap {
                core,
                address: 0,
                physical: 0,
                size: 0x4000,
                permissions: 7,
            })
            .collect(),
        peripherals: (0..5)
            .map(|core| factory::Peripheral {
                core,
                address: MMIO,
                physical: MMIO,
                size: 0x1000,
            })
            .collect(),
        instructions_per_tick: Ratio::new(1, 1).unwrap(),
    }
    .build(
        factory::Options::new(RunEngine::Unicorn)
            .with_translation_cache_size_bytes(1 << 20)
            .with_unicorn_workers(true),
        bus,
        None,
    )
    .unwrap();
    board.write_physical(0, &0x3f00u32.to_le_bytes()).unwrap();
    board.write_physical(4, &0x1001u32.to_le_bytes()).unwrap();
    board.set_block_execution(true);
    board.set_execution_policy(policy).unwrap();
    assert!(
        board
            .cpus
            .iter()
            .all(|cpu| cpu.features().asynchronous_runs)
    );
    board
}

fn a64_code(board: &Board, core: usize, words: &[u32]) {
    let bytes: Vec<_> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    board
        .write_physical(0x100 + core as u64 * 0x100, &bytes)
        .unwrap();
}

fn thumb_code(board: &Board, address: u64, words: &[u16]) {
    let bytes: Vec<_> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    board.write_physical(address, &bytes).unwrap();
}

#[test]
fn five_unicorn_cores_share_ram_and_service_the_same_deadline_as_exact() {
    use fujimod_emulator_cpu::arm;

    let run = |policy| {
        let mut bus = Bus::new(bus::Mode::Strict);
        let device = bus
            .register(
                MMIO,
                0x1000,
                Box::new(DeadlineSample {
                    due: Some(200),
                    armed_at: None,
                    samples: vec![],
                }),
            )
            .unwrap();
        let mut board = mixed_board(policy, bus, &[(arm::CORE[1], 0x3010)]);
        for core in 0..M4 {
            board
                .write_register(core, Register::X(1), 0x3000 + core as u64 * 4)
                .unwrap();
            board.write_register(core, Register::X(2), MMIO).unwrap();
            a64_code(
                &board,
                core,
                &[
                    0xd280_0000 | (((core + 1) as u32) << 5), // mov x0,#core+1
                    0xb900_0020,                              // str w0,[x1]
                    0xb940_0042,                              // ldr w2,[x2]
                    0xd503_207f,                              // wfi
                ],
            );
        }
        thumb_code(&board, 0x1000, &[0x2005, 0x6008, 0xbf30]); // movs; str; wfi

        board.run_until(220, 64).unwrap();
        assert_eq!(board.now(), 220);
        assert!(board.quiescent());
        assert_eq!(
            board.interactive_windows.get() != 0,
            policy == ExecutionPolicy::Interactive
        );

        let mut bytes = [0; 20];
        board.read_physical(0x3000, &mut bytes).unwrap();
        let words: Vec<_> = bytes
            .chunks_exact(4)
            .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
            .collect();
        assert_eq!(words, [1, 2, 3, 4, 5]);
        let state = board.state.borrow();
        assert_eq!(state.instructions, vec![4, 4, 4, 4, 3]);
        let sample = samples(&board, device);
        assert_eq!(sample, [(200, 1)]);
        let pcs: Vec<_> = (0..5)
            .map(|core| board.read_register(core, Register::Pc).unwrap())
            .collect();
        (state.instructions.clone(), words, sample, pcs)
    };

    assert_eq!(
        run(ExecutionPolicy::Exact),
        run(ExecutionPolicy::Interactive)
    );
}

struct MprofileDoorbell;
impl Device for MprofileDoorbell {
    fn name(&self) -> &'static str {
        "mprofile-doorbell"
    }

    fn read(&mut self, _: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        0
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, _: u8, _: u64) {
        if let Err(error) = cx.set_core_irq(M4, 0, address == MMIO) {
            cx.fail(error);
        }
    }
}

#[test]
fn a53_mmio_interrupts_the_running_m4_without_losing_thumb_entries() {
    use fujimod_emulator_cpu::arm;

    let mut bus = Bus::new(bus::Mode::Strict);
    bus.register(MMIO, 0x1000, Box::new(MprofileDoorbell))
        .unwrap();
    let mut board = mixed_board(
        ExecutionPolicy::Interactive,
        bus,
        &[(arm::CORE[1], 0xe000_e100), (arm::CORE[2], MMIO)],
    );
    board.write_register(0, Register::X(1), MMIO).unwrap();
    a64_code(&board, 0, &[0xd503_201f, 0xb900_0020, 0xd503_207f]);
    for core in 1..M4 {
        a64_code(&board, core, &[0xd503_207f]);
    }

    board
        .write_physical(0x40, &0x1201u32.to_le_bytes())
        .unwrap();
    thumb_code(
        &board,
        0x1000,
        &[
            0x2001, // movs r0,#1
            0x6008, // str r0,[r1] (enable line 0)
            0x3301, // add r3,#1
            0x2c00, // cmp r4,#0
            0xd0fc, // beq to the add
            0xbf30, // wfi
        ],
    );
    thumb_code(
        &board,
        0x1200,
        &[
            0x3401, // add r4,#1
            0x6050, // str r0,[r2,#4] (deassert the line)
            0x4770, // bx lr
        ],
    );
    board.run_until(200, 64).unwrap();
    assert!(board.quiescent());
    assert!(board.interactive_windows.get() > 0);
    assert_eq!(board.read_register(M4, arm::CORE[4]).unwrap(), 1);
    let rounds = board.read_register(M4, arm::CORE[3]).unwrap();
    assert!(rounds > 0);
    assert_eq!(
        board.state.borrow().instructions[M4],
        2 + rounds * 3 + 1 + 3
    );
}

/// Every core reads a device register each round and stores the result, so
/// each window parks a callback on all five cores while peers are still
/// running. The window must still close: the owner pauses the stragglers and
/// services the parked callbacks, which is exactly the handoff that stalled.
const STRESS_WINDOWS: u64 = 512;
const STRESS_ROUNDS: u64 = 8;
const STRESS_CHILD: &str = "FUJIMOD_INTERACTIVE_PAUSE_STRESS_CHILD";

fn pause_stress_board() -> Board {
    use fujimod_emulator_cpu::arm;
    // Each core adds one to a shared counter through a device read, then
    // spins. Four cores read the same register, so every window crosses at
    // least four owner callbacks while the other cores are mid-run.
    let mut bus = Bus::new(bus::Mode::Strict);
    bus.register(MMIO, 0x1000, Box::new(DeadlineSample {
        due: None,
        armed_at: None,
        samples: vec![],
    }))
    .unwrap();
    let mut board = mixed_board(ExecutionPolicy::Interactive, bus, &[(arm::CORE[1], 0x3000)]);
    for core in 0..M4 {
        board.write_register(core, Register::X(1), MMIO).unwrap();
        board
            .write_register(core, Register::X(2), 0x3000 + core as u64 * 4)
            .unwrap();
        a64_code(
            &board,
            core,
            &[
                0xb940_0020, // ldr w0,[x1]
                0xb900_0040, // str w0,[x2]
                0x1400_0000, // b . (chain the same core's next block)
            ],
        );
    }
    // The M-profile companion reads the same device and stores its result.
    thumb_code(&board, 0x1000, &[0x6808, 0x6048, 0xe7fe]); // ldr r0,[r1]; str r0,[r1,#4]; b .
    board.write_physical(0x3000, &0u32.to_le_bytes()).unwrap();
    board
}

fn run_pause_stress() {
    // Fresh boards reuse nothing, so a stall cannot come from leftover state.
    for round in 0..STRESS_ROUNDS {
        let mut board = pause_stress_board();
        let mut windows = 0u64;
        let mut boarded = vec![0u64; 5];
        while windows < STRESS_WINDOWS {
            // A short guest budget makes the cohort boundary land inside the
            // cores' block loops, so almost every window ends with pauses.
            board.run_for_instructions(64, 64).unwrap();
            windows += 1;
            // A core may sit out one window (the M-profile companion waits for
            // its interrupt), but every core must make progress across the run.
            for (total, count) in boarded
                .iter_mut()
                .zip(board.state.borrow().instructions.iter())
            {
                *total += count;
            }
        }
        assert!(
            boarded.iter().all(|&n| n > 0),
            "round {round} stranded a core: {boarded:?}"
        );
        assert!(
            board.interactive_windows.get() >= windows,
            "a window opened without reporting one"
        );
        // Every core must still be runnable and have retired work: a stall
        // leaves a core's progress frozen while the others keep running.
        assert!(
            board.state.borrow().waiting.iter().all(|&waiting| !waiting),
            "round {round} left a core waiting after {windows} windows"
        );
    }
}

#[test]
fn interactive_windows_close_while_every_core_is_parked_in_a_callback() {
    if std::env::var_os(STRESS_CHILD).is_some() {
        run_pause_stress();
        return;
    }
    // Run the stress in a child so a stranded window fails the test instead
    // of hanging the whole suite.
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("board::interactive::tests::interactive_windows_close_while_every_core_is_parked_in_a_callback")
        .arg("--include-ignored")
        .arg("--nocapture")
        .env(STRESS_CHILD, "1")
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    eprintln!("FUJIMOD stress child pid {}", child.id());
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "pause stress child failed: {status}");
            return;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("interactive window stalled while every core was parked or running");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
