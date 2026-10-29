#![cfg(not(target_os = "emscripten"))]

use crate::board::{
    Board,
    bus::{self, Bus},
    memory::BusMapping,
    set,
};
use fujimod_emulator_cpu::{Cpu, ExecutionPolicy, Register, memory::Backing, timeline::Ratio};
use fujimod_emulator_cpu_unicorn::{memory as cpu_memory, rprofile::worker};
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint};
use serde::{Deserialize, Serialize};

const MAX_INTERACTIVE_WINDOWS: u64 = 10;

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
        let before = board.fingerprint().unwrap();
        let saved = board.checkpoint(None).unwrap();

        board
            .write_physical(0x3000, &0xdead_beefu32.to_le_bytes())
            .unwrap();
        board.restore_keyframe(&saved).unwrap();
        assert_eq!(board.fingerprint().unwrap(), before);
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
    let share = if policy == ExecutionPolicy::Exact && tick_limit {
        50_000
    } else {
        100_000
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
                    .all(|&count| (share..=share + 256).contains(&count)),
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
    let before = board.fingerprint().unwrap();
    let saved = board.checkpoint(None).unwrap();
    board.run_for_instructions(1000, 1000).unwrap();
    board.restore_keyframe(&saved).unwrap();
    assert_eq!(board.fingerprint().unwrap(), before);
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
    let before = board.fingerprint().unwrap();
    board.run_until(20_000, 1000).unwrap();
    board.restore_keyframe(&saved).unwrap();
    assert_eq!(board.fingerprint().unwrap(), before);
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
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
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
