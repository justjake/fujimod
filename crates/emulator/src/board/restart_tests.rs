//! A counted guest loop must finish however the board splits its instruction
//! budget, and whichever block-accounting path the engine runs.
use super::*;
use fujimod_emulator_cpu::arm;
use fujimod_emulator_cpu::Model;
use fujimod_emulator_cpu_unicorn::Library;

const ITERATIONS: u32 = 20;

fn set(cpu: &dyn Cpu, register: Register, value: u32) {
    cpu.write_register(register, &value.to_le_bytes()).unwrap();
}

fn get(cpu: &dyn Cpu, register: Register) -> u32 {
    let mut bytes = [0; 4];
    cpu.read_register(register, &mut bytes).unwrap();
    u32::from_le_bytes(bytes)
}

fn words(cpu: &dyn Cpu, address: u64, values: &[u32]) {
    cpu.write_memory(
        address,
        &values.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>(),
    )
    .unwrap();
}

#[derive(Clone, Copy)]
struct Case {
    /// Advance the store base each iteration instead of reusing one address.
    moving_store: bool,
    legacy_block_accounting: bool,
    policy: ExecutionPolicy,
}

impl std::fmt::Debug for Case {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "moving_store={} legacy={} {:?}",
            self.moving_store, self.legacy_block_accounting, self.policy
        )
    }
}

fn cases() -> impl Iterator<Item = Case> {
    [false, true].into_iter().flat_map(|moving_store| {
        [false, true].into_iter().flat_map(move |legacy_block_accounting| {
            [ExecutionPolicy::Interactive, ExecutionPolicy::Exact]
                .into_iter()
                .map(move |policy| Case {
                    moving_store,
                    legacy_block_accounting,
                    policy,
                })
        })
    })
}

fn cpu_only(case: Case) -> Box<dyn Cpu> {
    let library = Library::linked().unwrap();
    let raw = library.open(Model::CortexR5F, 4 << 20).unwrap();
    let cpu: Box<dyn Cpu> = Box::new(
        fujimod_emulator_cpu_unicorn::rprofile::core::Core::new(
            raw,
            fujimod_emulator_cpu_unicorn::rprofile::core::Options {
                legacy_block_accounting: case.legacy_block_accounting,
                ..Default::default()
            },
            fujimod_emulator_cpu_unicorn::rprofile::core::Wiring {
                memory: Default::default(),
            },
        )
        .unwrap(),
    );
    cpu.map_zeroed(0x10000, 0x1000, 7).unwrap();
    cpu.map_zeroed(0x20000, 0x1000, 7).unwrap();
    words(
        &*cpu,
        0x10000,
        &[
            0xe3a0_0000, // mov r0, #0
            0xe584_0000, // str r0, [r4]
            if case.moving_store { 0xe284_0004 } else { 0xe1a0_0000 }, // add r4, r4, #4
            0xe256_6001, // subs r6, r6, #1
            0x1aff_fffb, // bne 0x10004
            0xe584_6000, // str r6, [r4]
            0xeaff_fffe, // b .
        ],
    );
    set(&*cpu, arm::CORE[4], 0x20000);
    set(&*cpu, arm::CORE[6], ITERATIONS);
    set(&*cpu, arm::CPSR, 0xd3);
    set(&*cpu, Register::Pc, 0x10000);
    cpu
}

fn build(case: Case) -> Board {
    let cpu = cpu_only(case);
    let mut bus = Bus::new(bus::Mode::Strict);
    bus.register(0x8000000, 4, Box::new(Zero)).unwrap();
    let mut board = Board::new(
        vec![cpu],
        bus,
        Ratio::new(1, 1).unwrap(),
        None,
        vec![true],
        memory::Memory::default(),
    )
    .unwrap();
    board.map_mmio(0, 0x8000000, 0x1000, 0x8000000).unwrap();
    board.set_execution_policy(case.policy).unwrap();
    board
}

struct Zero;
impl fujimod_emulator_devices::Device for Zero {
    fn name(&self) -> &'static str {
        "zero"
    }
    fn read(&mut self, _: &mut fujimod_emulator_devices::Cx<'_>, _: u64, _: u8) -> u64 {
        0
    }
    fn write(&mut self, _: &mut fujimod_emulator_devices::Cx<'_>, _: u64, _: u8, _: u64) {}
    fn save(&self) -> Value {
        json!({})
    }
}

fn assert_finished(board: &Board) {
    assert_eq!(
        get(board.cpu(0).unwrap(), arm::CORE[6]),
        0,
        "the loop must run exactly {ITERATIONS} iterations"
    );
    assert_eq!(
        get(board.cpu(0).unwrap(), Register::Pc),
        0x10018,
        "execution must rest in the trailing branch"
    );
}

#[test]
fn one_long_run_finishes_the_loop() {
    for case in cases() {
        let mut board = build(case);
        board.run_for_instructions(1000, 1000).unwrap();
        assert_finished(&board);
    }
}

#[test]
fn one_instruction_runs_finish_the_loop() {
    for case in cases() {
        let mut board = build(case);
        for _ in 0..1000 {
            board
                .run_for_instructions(1, 1000)
                .unwrap_or_else(|error| panic!("{case:?}: {error}"));
        }
        assert_finished(&board);
    }
}

#[test]
fn uneven_runs_finish_the_loop() {
    // The split that first showed the failure: every budget lands inside a
    // different part of the block.
    for case in cases() {
        let mut board = build(case);
        for count in [1, 7, 13, 29, 41, 2, 3] {
            board
                .run_for_instructions(count, 1000)
                .unwrap_or_else(|error| panic!("{case:?} count={count}: {error}"));
        }
        board.run_for_instructions(1000, 1000).unwrap();
        assert_finished(&board);
    }
}

#[test]
fn the_core_alone_finishes_the_loop() {
    for case in cases() {
        let cpu = cpu_only(case);
        cpu.set_execution_policy(case.policy).unwrap();
        let mut done = 0u64;
        while done < 1000 {
            let pc = get(&*cpu, Register::Pc) as u64 | u64::from(get(&*cpu, arm::CPSR) & 32 != 0);
            cpu.run(Run {
                begin: pc,
                until: None,
                watchdog_us: 0,
                instructions: Some(1000 - done as usize),
            })
            .unwrap();
            let progress = cpu.instruction_progress().unwrap_or(0);
            assert_ne!(progress, 0, "the core must make progress");
            done += progress;
        }
        assert_eq!(get(&*cpu, arm::CORE[6]), 0, "the core alone must finish the loop");
    }
}
