//! Block execution of an M-profile processor must run the instructions, take
//! the exceptions and reach the device instants that the per-instruction
//! observer does.
use super::*;
use fujimod_catalog_configuration::RunEngine;
use fujimod_emulator_cpu::Model;

/// Cortex-M4 program at 0x10100 (handler at 0x101b8): SysTick at priority
/// 0x40 with a short period; a loop with stores, loads, a PRIMASK window and
/// a BASEPRI window that each end before a run of plain instructions (so the
/// instruction at which a pending SysTick is taken shows in R9), a call, a
/// store into the page the code runs from (0x10800), an exclusive pair on a
/// word the peer also stores to (failures sum in R10), a store that rewrites
/// an instruction of the loop each round (ADDS R6,#1 or #2; a branch lies
/// between the store and the rewritten instruction, so both ways run them in
/// different translation blocks), an IT block, MRS of
/// PRIMASK (native) and IPSR (emulated), PUSH/POP, and WFI every 16th round.
/// The handler adds R6 to R9 and reads the SysTick control register.
const PROGRAM: [u16; 101] = [
    0xf24e, 0x0010, 0xf2ce, 0x0000, 0xf64e, 0x5123, 0xf2ce, 0x0100, 0x2240, 0x700a, 0x2125,
    0x6041, 0x2107, 0x6001, 0xf242, 0x0400, 0xf2c0, 0x0401, 0x2500, // loop:
    0x3501, 0x6025, 0x6826, 0xb672, 0x3603, 0x6066, 0x3601, 0x3601, 0xb662, 0x3601, 0x3601,
    0x3601, 0xf000, 0xf836, 0x2110, 0xf381, 0x8811, 0x3601, 0x3601, 0x3601, 0x2100, 0xf381,
    0x8811, 0x3601, 0x3601, 0x3601, 0xf640, 0x0100, 0xf2c0, 0x0101, 0x680a, 0x3201, 0x600a,
    0x3601, 0xe854, 0x2f04, 0x3201, 0xe844, 0x2c04, 0x44e2, 0xf20f, 0x0116, 0x07ea, 0xbf0c,
    0xf243, 0x6201, 0xf243, 0x6202, 0x800a, 0xe7ff, // after_patch:
    0x3601, 0x3601, // patch_site:
    0x3601, 0x3601, 0x2d28, 0xbfb4, 0x3701, 0x3702, 0xf3ef, 0x8110, 0xf3ef, 0x8205, 0xb430,
    0xbc30, 0x0729, 0xd1bd, 0xbf30, 0xe7bb, // func:
    0x3301, 0x68a1, 0x3101, 0x60a1, 0x4770, // handler:
    0xf108, 0x0801, 0xeb09, 0x0906, 0x6801, 0x68e1, 0x3105, 0x60e1, 0x4770,
];

struct Outcome {
    state: Value,
    registers: Vec<u64>,
    memory: Vec<u8>,
    observed: u64,
    fast_stores: bool,
}

/// The program and its storing peer on a fresh board.
fn board(blocks: bool) -> Board {
    use factory::*;
    let arm32 = |r| Register::Arm32(fujimod_emulator_cpu::arm::Register::R(r));
    let graph = Graph {
        decoders: vec![],
        processors: vec![
            Processor {
                model: Model::CortexM4,
                registers: vec![],
                released: true,
                rprofile: None,
                mprofile: Some(super::mprofile::Config {
                    clock_hz: 1_000_000_000,
                    board_ticks_per_second: 1_000_000_000,
                    vector_base: 0x10000,
                }),
            },
            Processor {
                rprofile: Some(Default::default()),
                mprofile: None,
                model: Model::CortexR5F,
                registers: vec![(Register::Pc, 0x14000), (arm32(1), 0x12010)],
                released: true,
            },
        ],
        dma_mappings: vec![crate::board::memory::BusMapping::ram(0x10000, 0x8000)],
        ram: vec![Ram {
            base: 0x10000,
            size: 0x8000,
        }],
        mappings: (0..2)
            .map(|core| MemoryMap {
                core,
                address: 0x10000,
                physical: 0x10000,
                size: 0x8000,
                permissions: 7,
            })
            .collect(),
        peripherals: vec![],
        instructions_per_tick: Ratio::new(1, 1).unwrap(),
    };
    let mut board = graph
        .build(
            Options::new(RunEngine::Unicorn).with_translation_cache_size_bytes(1024 * 1024),
            Bus::new(bus::Mode::Strict),
            None,
        )
        .unwrap();
    board.set_block_execution(blocks);
    let words = |words: &[u32]| words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>();
    board.write_physical(0x10000, &words(&[0x17000, 0x10101])).unwrap();
    board.write_physical(0x10000 + 15 * 4, &words(&[0x101b9])).unwrap();
    board
        .write_physical(
            0x10100,
            &PROGRAM.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>(),
        )
        .unwrap();
    // The peer: add r0,r0,#1; str r0,[r1]; b .-8
    board
        .write_physical(0x14000, &words(&[0xe2800001, 0xe5810000, 0xeafffffc]))
        .unwrap();
    board
}

fn run(quantum: usize, blocks: bool, until: u64, windows: &[u64]) -> Outcome {
    let mut board = board(blocks);
    for &window in windows {
        board.run_until(window, quantum).unwrap();
    }
    board.run_until(until, quantum).unwrap();
    outcome(&board)
}

fn outcome(board: &Board) -> Outcome {
    let arm32 = |r| Register::Arm32(fujimod_emulator_cpu::arm::Register::R(r));
    let m4 = board.cpu(0).unwrap();
    let mut registers: Vec<u64> = (0..13).map(|r| scalar(m4, arm32(r)).unwrap()).collect();
    registers.push(scalar(m4, Register::Pc).unwrap());
    let mut memory = vec![0; 0x24];
    board.read_physical(0x12000, &mut memory[..0x20]).unwrap();
    board.read_physical(0x10800, &mut memory[0x20..]).unwrap();
    let (observed, fast_stores) = board.state.borrow().blocks[0]
        .as_ref()
        .map_or((0, false), |blocks| (blocks.observed, blocks.fast_stores));
    Outcome {
        state: board.state(),
        registers,
        memory,
        observed,
        fast_stores,
    }
}

#[test]
fn block_execution_equals_the_instruction_observer() {
    for quantum in [1, 2, 3, 7, 16, 64, 100] {
        for (until, windows) in [(2500, &[][..]), (4000, &[211, 212, 900, 2333][..])] {
            let reference = run(quantum, false, until, windows);
            let blocks = run(quantum, true, until, windows);
            assert_eq!(reference.observed, 0);
            assert!(blocks.observed > 0, "quantum {quantum}: block execution never ran");
            assert!(blocks.fast_stores, "quantum {quantum}: the store hook stayed on every store");
            assert_eq!(blocks.registers, reference.registers, "quantum {quantum} until {until}");
            assert_eq!(blocks.memory, reference.memory, "quantum {quantum} until {until}");
            assert_eq!(blocks.state, reference.state, "quantum {quantum} until {until}");
            // The program took SysTick exceptions and ran its handler.
            assert!(reference.registers[8] > 0, "quantum {quantum}: no exception was taken");
            assert!(reference.registers[5] > 16, "quantum {quantum}: the loop did not pass a WFI");
            // The store into the code page landed every round, and the
            // exclusive pair both succeeded and failed against the peer.
            let rounds = reference.registers[5];
            assert!((rounds - 1..=rounds).contains(&u64::from(reference.memory[0x20])));
            assert!(reference.registers[10] > 0, "quantum {quantum}: STREX never failed");
        }
    }
}

/// Which hooks the execution core holds at an instant is a host choice. A
/// keyframe taken before block execution set its hooks up restores after it,
/// one taken in block execution restores with the instruction observer, and
/// the portable form is the same bytes either way.
#[test]
fn keyframes_do_not_bind_to_the_execution_core_s_own_hooks() {
    let reference = run(100, false, 2500, &[]);
    let mut board = board(true);
    let start = board.checkpoint(None).unwrap();
    let portable_start = start.export().unwrap().to_bytes().unwrap();
    board.run_until(900, 100).unwrap();
    assert!(board.state.borrow().blocks[0].as_ref().unwrap().active);
    let middle = board.checkpoint(None).unwrap();
    board.run_until(1700, 100).unwrap();
    // Back to the start, with the block hooks installed.
    board.restore_keyframe(&start).unwrap();
    assert_eq!(
        board.checkpoint(None).unwrap().export().unwrap().to_bytes().unwrap(),
        portable_start
    );
    board.run_until(1300, 100).unwrap();
    // The middle keyframe, with the instruction observer.
    board.set_block_execution(false);
    board.run_until(1400, 100).unwrap();
    assert!(!board.state.borrow().blocks[0].as_ref().unwrap().active);
    board.restore_keyframe(&middle).unwrap();
    board.run_until(2500, 100).unwrap();
    let instruction = outcome(&board);
    assert_eq!(instruction.registers, reference.registers);
    assert_eq!(instruction.memory, reference.memory);
    assert!(instruction.state == reference.state, "board state differs");
    // A fresh board with the instruction observer accepts the portable form
    // written in block execution.
    board.restore_keyframe(&middle).unwrap();
    board.set_block_execution(true);
    board.run_until(950, 100).unwrap();
    board.restore_keyframe(&middle).unwrap();
    let portable = board.checkpoint(None).unwrap().export().unwrap().to_bytes().unwrap();
    let mut fresh = self::board(false);
    let imported = fresh
        .import_keyframe(
            &fujimod_emulator_devices::checkpoint::Checkpoint::from_bytes(&portable).unwrap(),
        )
        .unwrap();
    fresh.restore_keyframe(&imported).unwrap();
    fresh.run_until(2500, 100).unwrap();
    let fresh = outcome(&fresh);
    assert_eq!(fresh.registers, reference.registers);
    assert!(fresh.state == reference.state, "board state differs after import");
}

/// Two Cortex-M4 processors without aliased views. One runs an exclusive
/// pair in a loop; the other stores twice per round into the same granule.
/// Its second store goes through a cached translation entry, so it reaches
/// the monitor only while its store hook sees every store.
fn run_pair(quantum: usize, blocks: bool) -> (Value, Vec<u64>, bool) {
    use factory::*;
    let arm32 = |r| Register::Arm32(fujimod_emulator_cpu::arm::Register::R(r));
    let graph = Graph {
        decoders: vec![],
        processors: [0x10000, 0x11000]
            .into_iter()
            .map(|vector_base| Processor {
                model: Model::CortexM4,
                registers: vec![],
                released: true,
                rprofile: None,
                mprofile: Some(super::mprofile::Config {
                    clock_hz: 1_000_000_000,
                    board_ticks_per_second: 1_000_000_000,
                    vector_base,
                }),
            })
            .collect(),
        dma_mappings: vec![crate::board::memory::BusMapping::ram(0x10000, 0x8000)],
        ram: vec![Ram {
            base: 0x10000,
            size: 0x8000,
        }],
        mappings: (0..2)
            .map(|core| MemoryMap {
                core,
                address: 0x10000,
                physical: 0x10000,
                size: 0x8000,
                permissions: 7,
            })
            .collect(),
        peripherals: vec![],
        instructions_per_tick: Ratio::new(1, 1).unwrap(),
    };
    let mut board = graph
        .build(
            Options::new(RunEngine::Unicorn).with_translation_cache_size_bytes(1024 * 1024),
            Bus::new(bus::Mode::Strict),
            None,
        )
        .unwrap();
    board.set_block_execution(blocks);
    let words = |words: &[u32]| words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>();
    let halfwords =
        |words: &[u16]| words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>();
    board.write_physical(0x10000, &words(&[0x17000, 0x10101])).unwrap();
    board.write_physical(0x11000, &words(&[0x16000, 0x10201])).unwrap();
    // movw/movt r0,#0x12000; loop: ldrex r1,[r0]; adds r1,#1; nop; nop;
    // strex r2,r1,[r0]; add r3,r2; adds r4,#1; b loop
    board
        .write_physical(
            0x10100,
            &halfwords(&[
                0xf242, 0x0000, 0xf2c0, 0x0001, 0xe850, 0x1f00, 0x3101, 0xbf00, 0xbf00, 0xe840,
                0x1200, 0x4413, 0x3401, 0xe7f5,
            ]),
        )
        .unwrap();
    // movw/movt r0,#0x12000; loop: ldr r1,[r0,#4]; adds r1,#1; str r1,[r0,#4];
    // str r1,[r0,#8]; adds r5,#1; b loop
    board
        .write_physical(
            0x10200,
            &halfwords(&[
                0xf242, 0x0000, 0xf2c0, 0x0001, 0x6841, 0x3101, 0x6041, 0x6081, 0x3501, 0xe7f9,
            ]),
        )
        .unwrap();
    board.run_until(600, quantum).unwrap();
    let registers = [1, 2, 3, 4]
        .map(|r| scalar(board.cpu(0).unwrap(), arm32(r)).unwrap())
        .to_vec();
    let fast = board.state.borrow().blocks[1]
        .as_ref()
        .is_some_and(|blocks| blocks.watching);
    (board.state(), registers, fast)
}

#[test]
fn a_peer_m_profile_store_reaches_a_live_reservation_in_block_execution() {
    for quantum in [1, 2, 3, 5, 64] {
        let (reference, reference_registers, _) = run_pair(quantum, false);
        let (blocks, registers, watched) = run_pair(quantum, true);
        assert!(watched, "quantum {quantum}: the peer never ran with watched stores");
        assert_eq!(registers, reference_registers, "quantum {quantum}");
        assert_eq!(blocks, reference, "quantum {quantum}");
        if quantum < 5 {
            // The peer stores inside the pair: some STREX fail.
            assert!(reference_registers[2] > 0, "quantum {quantum}: STREX never failed");
        }
    }
}

/// Host speed of the two ways (not a correctness test) on a plain loop with
/// loads, stores, an IT block and a call, while the peer waits:
/// `cargo test --release -p fujimod-emulator --lib bench_block_execution -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench_block_execution() {
    use factory::*;
    const LOOP: [u16; 24] = [
        0xf242, 0x0400, 0xf2c0, 0x0401, 0x3501, 0x6826, 0x1976, 0x6066, 0x4077, 0x08f9, 0x1852,
        0x42aa, 0xbf18, 0x3001, 0xf000, 0xf801, 0xe7f2, 0x3301, 0x68a1, 0x3101, 0x60a1, 0x0089,
        0x404b, 0x4770,
    ];
    for blocks in [false, true] {
        let graph = Graph {
            decoders: vec![],
            processors: vec![
                Processor {
                    model: Model::CortexM4,
                    registers: vec![],
                    released: true,
                    rprofile: None,
                    mprofile: Some(super::mprofile::Config {
                        clock_hz: 1_000_000_000,
                        board_ticks_per_second: 1_000_000_000,
                        vector_base: 0x10000,
                    }),
                },
                Processor {
                    rprofile: Some(Default::default()),
                    mprofile: None,
                    model: Model::CortexR5F,
                    registers: vec![(Register::Pc, 0x14000)],
                    released: true,
                },
            ],
            dma_mappings: vec![crate::board::memory::BusMapping::ram(0x10000, 0x8000)],
            ram: vec![Ram {
                base: 0x10000,
                size: 0x8000,
            }],
            mappings: (0..2)
                .map(|core| MemoryMap {
                    core,
                    address: 0x10000,
                    physical: 0x10000,
                    size: 0x8000,
                    permissions: 7,
                })
                .collect(),
            peripherals: vec![],
            instructions_per_tick: Ratio::new(1, 1).unwrap(),
        };
        let mut board = graph
            .build(
                Options::new(RunEngine::Unicorn).with_translation_cache_size_bytes(1024 * 1024),
                Bus::new(bus::Mode::Strict),
                None,
            )
            .unwrap();
        board.set_block_execution(blocks);
        let words =
            |words: &[u32]| words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>();
        board.write_physical(0x10000, &words(&[0x17000, 0x10101])).unwrap();
        board
            .write_physical(
                0x10100,
                &LOOP.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>(),
            )
            .unwrap();
        // The peer: wfi; b .
        board.write_physical(0x14000, &words(&[0xe320f003, 0xeafffffe])).unwrap();
        let started = std::time::Instant::now();
        // One-millisecond run windows, as the boot driver uses.
        let mut until = 0;
        while until < 60_000_000 {
            until += 1_000_000;
            board.run_until(until, 100).unwrap();
        }
        let elapsed = started.elapsed();
        let entries = board.state()["instructions"][0].as_u64().unwrap();
        eprintln!(
            "blocks={blocks}: {entries} M-profile entries in {:.2} s, {:.1} ns per entry",
            elapsed.as_secs_f64(),
            elapsed.as_nanos() as f64 / entries as f64
        );
    }
}
