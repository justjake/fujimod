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
                released: true, parked_until_sgi: false,
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
                released: true, parked_until_sgi: false,
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
    let board = graph
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

fn run(
    quantum: usize,
    blocks: bool,
    until: u64,
    windows: &[u64],
    policy: ExecutionPolicy,
) -> Outcome {
    let mut board = board(blocks);
    board.set_execution_policy(policy).unwrap();
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
            let reference = run(quantum, false, until, windows, ExecutionPolicy::Exact);
            let blocks = run(quantum, true, until, windows, ExecutionPolicy::Exact);
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

#[test]
fn interactive_ordered_blocks_equal_the_instruction_observer() {
    let policy = ExecutionPolicy::Interactive;
    let windows = &[211, 212, 900, 2333];
    let reference = run(16, false, 4000, windows, policy);
    let blocks = run(16, true, 4000, windows, policy);
    assert!(blocks.observed > 0);
    assert_eq!(blocks.registers, reference.registers);
    assert_eq!(blocks.memory, reference.memory);
    assert_eq!(blocks.state, reference.state);
}

/// Repeated mixed-width Thumb blocks end partway through a guest loop when a
/// peer gets its turn or the caller's board-clock deadline expires.
#[test]
fn translated_thumb_budget_matches_the_observer_at_each_stop() {
    let mut reference = board(false);
    let mut translated = board(true);
    for b in [&mut reference, &mut translated] {
        b.set_execution_policy(ExecutionPolicy::Interactive).unwrap();
        b.state.borrow_mut().settled.verify = 0;
        // adds r0,#1; and.w r1,r0,#7; eors r2,r0; b 0x10100
        let code = [0x3001u16, 0xf000, 0x0107, 0x4042, 0xe7fa];
        let bytes: Vec<u8> = code.iter().flat_map(|h| h.to_le_bytes()).collect();
        b.write_physical(0x10100, &bytes).unwrap();
    }
    for stop in [61, 64, 127, 211, 257, 2047] {
        reference.run_until(stop, 63).unwrap();
        translated.run_until(stop, 63).unwrap();
        let a = outcome(&reference);
        let b = outcome(&translated);
        assert_eq!(a.registers, b.registers, "registers at {stop}");
        assert_eq!(a.state, b.state, "board at {stop}");
        assert_eq!(a.memory, b.memory, "memory at {stop}");
    }
    let s = translated.state.borrow();
    assert!(s.blocks[0].as_ref().unwrap().inline.entries > 0);
}

/// The same mixed-width loop through ordinary RAM, a watched code page, the
/// NVIC enable register, or the end of mapped RAM. The last case faults partway through
/// a translated block after several successful iterations.
fn transfer_loop(blocks: bool, address: u32, advance: bool) -> Board {
    let mut b = board(blocks);
    b.set_execution_policy(ExecutionPolicy::Interactive).unwrap();
    b.state.borrow_mut().settled.verify = 0;
    // ldr r1,literal; b loop; loop: adds r0,#1; str r0,[r1];
    // and.w r2,r0,#7; ldr r3,[r1]; adds r1,#0/4; b loop; literal (aligned)
    let code: [u16; 10] = [
        0x4904, 0xe7ff, 0x3001, 0x6008, 0xf000, 0x0207, 0x680b,
        if advance { 0x3104 } else { 0x3100 }, 0xe7f8, 0xbf00,
    ];
    let mut bytes: Vec<u8> = code.iter().flat_map(|word| word.to_le_bytes()).collect();
    bytes.extend_from_slice(&address.to_le_bytes());
    b.write_physical(0x10100, &bytes).unwrap();
    b
}

#[test]
fn translated_transfers_match_at_watched_stores_and_data_faults() {
    for (address, advance) in [(0x12000, false), (0x10800, false), (0xe000_e100, false), (0x17ff0, true)] {
        let mut reference = transfer_loop(false, address, advance);
        let mut translated = transfer_loop(true, address, advance);
        for stop in [11, 51, 139, 2031] {
            let a = reference.run_until(stop, 63);
            let b = translated.run_until(stop, 63);
            assert_eq!(a.is_err(), b.is_err(), "{address:#x} at {stop}: {a:?} {b:?}");
            let expected = outcome(&reference);
            let actual = outcome(&translated);
            assert_eq!(actual.registers, expected.registers, "{address:#x} registers at {stop}");
            assert_eq!(actual.memory, expected.memory, "{address:#x} RAM at {stop}");
            assert_eq!(translated.state.borrow().instructions, reference.state.borrow().instructions);
            assert_eq!(translated.state.borrow().timeline.now(), reference.state.borrow().timeline.now());
            if let Err(error) = b {
                assert!(advance && error.to_ascii_lowercase().contains("unmapped"), "{error}");
                break;
            }
            assert_eq!(actual.state, expected.state, "{address:#x} at {stop}");
        }
        if !advance && address == 0x12000 {
            assert!(translated.state.borrow().blocks[0].as_ref().unwrap().inline.entries > 500);
        }
        if address == 0x10800 {
            // Only the first nonterminal store at this site needs a retry.
            // Other cached block shapes may encounter it before retranslation.
            let counters = translated.cpu(0).unwrap().exit_counters().unwrap();
            assert_eq!(counters["watched_store_sites"], 1);
            assert!(counters["restart_requests"] < 16, "{counters:?}");
            assert_eq!(counters["watched_store_cap_misses"], 0);
        }
        if advance {
            assert!(reference.state.borrow().timeline.now() < 2031, "loop never reached unmapped memory");
        }
    }
}

/// A sampled M-core table scan uses `LDRH.W Rt,[Rn,Rm,LSL #1]`. Exercise its
/// register-offset and scaled-address form in RAM, then at the end of mapped
/// RAM where the second load faults after one successful iteration.
fn register_offset_load_loop(blocks: bool, address: u32) -> Board {
    let mut b = board(blocks);
    b.set_execution_policy(ExecutionPolicy::Interactive).unwrap();
    b.state.borrow_mut().settled.verify = 0;
    // ldr r0,literal; movs r1,#0; loop: ldrh.w r2,[r0,r1,LSL #1];
    // adds r1,#1; cmp r1,#8; bne loop; wfi; literal address.
    let code = [
        0x4803u16, 0x2100, 0xf830, 0x2011, 0x3101, 0x2908, 0xd1fa, 0xbf30,
    ];
    let mut bytes: Vec<u8> = code.iter().flat_map(|half| half.to_le_bytes()).collect();
    bytes.extend_from_slice(&address.to_le_bytes());
    b.write_physical(0x10100, &bytes).unwrap();
    b.write_physical(0x12000, &[1, 0, 2, 0, 3, 0, 4, 0, 5, 0, 6, 0, 7, 0, 8, 0])
        .unwrap();
    b
}

#[test]
fn shifted_register_offset_loads_match_in_ram_and_on_fault() {
    for address in [0x12000, 0x17ffe] {
        let mut reference = register_offset_load_loop(false, address);
        let mut translated = register_offset_load_loop(true, address);
        let reference_result = reference.run_until(100, 8);
        let translated_result = translated.run_until(100, 8);
        assert_eq!(
            reference_result.is_err(),
            translated_result.is_err(),
            "results for base {address:#x}: {reference_result:?} / {translated_result:?}"
        );
        let expected = outcome(&reference);
        let actual = outcome(&translated);
        assert_eq!(actual.registers, expected.registers, "registers at {address:#x}");
        assert_eq!(actual.memory, expected.memory, "RAM at {address:#x}");
        assert_eq!(
            translated.state.borrow().instructions,
            reference.state.borrow().instructions,
            "instruction count at {address:#x}"
        );
        assert_eq!(
            translated.state.borrow().timeline.now(),
            reference.state.borrow().timeline.now(),
            "board clock at {address:#x}"
        );
        if address == 0x12000 {
            assert!(reference_result.is_ok());
            assert_eq!(actual.registers[1], 8);
            assert_eq!(actual.registers[2], 8);
            assert_eq!(actual.state, expected.state);
            assert!(
                translated.state.borrow().blocks[0].as_ref().unwrap().inline.entries > 0,
                "scaled register-offset load did not enter translated accounting"
            );
        } else {
            assert!(
                reference_result.as_ref().err().is_some_and(|error| error.to_ascii_lowercase().contains("unmapped")),
                "expected the reference load to fault outside RAM: {reference_result:?}"
            );
            assert!(
                translated_result.as_ref().err().is_some_and(|error| error.to_ascii_lowercase().contains("unmapped")),
                "expected the translated load to fault outside RAM: {translated_result:?}"
            );
            assert_eq!(actual.registers[1], 1);
            assert_eq!(actual.registers[2], 0);
        }
    }
}

fn indexed_transfer_pair(
    blocks: bool,
    pre_base: u32,
    post_base: u32,
    lr_value: u32,
    first: [u16; 2],
    second: [u16; 2],
) -> Board {
    let mut b = board(blocks);
    b.set_execution_policy(ExecutionPolicy::Interactive).unwrap();
    b.state.borrow_mut().settled.verify = 0;
    // Load both bases and LR from literals, then execute the two supplied
    // transfers in a loop.
    let code = [
        0x4b04u16,
        0x4805,
        0x4684,
        0x4805,
        0x4686,
        first[0],
        first[1],
        second[0],
        second[1],
        0xbf30,
    ];
    let bytes: Vec<u8> = code.iter().flat_map(|half| half.to_le_bytes()).collect();
    let mut bytes = bytes;
    bytes.extend_from_slice(&pre_base.to_le_bytes());
    bytes.extend_from_slice(&post_base.to_le_bytes());
    bytes.extend_from_slice(&lr_value.to_le_bytes());
    b.write_physical(0x10100, &bytes).unwrap();
    b.write_physical(
        0x12000,
        &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc],
    )
    .unwrap();
    b
}

#[test]
fn indexed_loads_match_for_ram_mmio_and_faults() {
    for (pre_base, post_base) in [(0x12004, 0x12008), (0xe000_e104, 0xe000_e100)] {
        let make = |blocks| {
            indexed_transfer_pair(
                blocks,
                pre_base,
                post_base,
                0x1357_9bdf,
                [0xf853, 0x3c04],
                [0xf85c, 0xeb04],
            )
        };
        let mut reference = make(false);
        let mut translated = make(true);
        let r = |n| Register::Arm32(fujimod_emulator_cpu::arm::Register::R(n));
        let a = reference.run_until(100, 64);
        let b = translated.run_until(100, 64);
        assert_eq!(a.is_err(), b.is_err(), "{pre_base:#x}/{post_base:#x}: {a:?} {b:?}");
        assert!(a.is_ok(), "unexpected indexed-load result: {a:?}");
        assert_eq!(outcome(&translated).registers, outcome(&reference).registers);
        assert_eq!(outcome(&translated).memory, outcome(&reference).memory);
        assert_eq!(translated.state(), reference.state());
        assert_eq!(
            translated.state.borrow().instructions,
            reference.state.borrow().instructions
        );
        assert_eq!(
            translated.state.borrow().timeline.now(),
            reference.state.borrow().timeline.now()
        );
        assert_eq!(
            translated.read_register(0, r(14)).unwrap(),
            reference.read_register(0, r(14)).unwrap()
        );
        assert_eq!(
            translated.read_register(0, r(12)).unwrap(),
            reference.read_register(0, r(12)).unwrap()
        );
        if pre_base == 0x12004 {
            assert!(a.is_ok(), "unexpected RAM load result {a:?}");
            assert_eq!(translated.read_register(0, r(3)).unwrap(), 0x4433_2211);
            assert_eq!(translated.read_register(0, r(14)).unwrap(), 0xccbb_aa99);
            assert_eq!(translated.read_register(0, r(12)).unwrap(), 0x1200c);
        } else {
            let blocks = translated.state.borrow();
            let blocks = blocks.blocks[0].as_ref().unwrap();
            assert!(blocks.restart_callbacks > 0, "device accesses were not restarted");
            assert!(blocks.mmio_callbacks > 0, "device callbacks did not run");
        }
    }

    for (pre_base, post_base, faulting_pc) in [
        (0x10000, 0x12008, 0x1010a),
        (0x12004, 0x18000, 0x1010e),
    ] {
        let make = |blocks| {
            indexed_transfer_pair(
                blocks,
                pre_base,
                post_base,
                0x1357_9bdf,
                [0xf853, 0x3c04],
                [0xf85c, 0xeb04],
            )
        };
        let mut reference = make(false);
        let mut translated = make(true);
        let a = reference.run_until(100, 64);
        let b = translated.run_until(100, 64);
        assert!(
            a.as_ref().err().is_some_and(|error| error.to_lowercase().contains("unmapped")),
            "expected reference indexed load fault: {a:?}"
        );
        assert!(
            b.as_ref().err().is_some_and(|error| error.to_lowercase().contains("unmapped")),
            "expected translated indexed load fault: {b:?}"
        );
        assert_eq!(outcome(&translated).registers, outcome(&reference).registers);
        assert_eq!(outcome(&translated).memory, outcome(&reference).memory);
        assert_eq!(translated.state(), reference.state());
        assert_eq!(
            translated.state.borrow().instructions,
            reference.state.borrow().instructions
        );
        assert_eq!(
            translated.state.borrow().timeline.now(),
            reference.state.borrow().timeline.now()
        );
        let r = |n| Register::Arm32(fujimod_emulator_cpu::arm::Register::R(n));
        if faulting_pc == 0x1010a {
            assert_eq!(translated.read_register(0, r(3)).unwrap(), 0x10000);
        } else {
            assert_eq!(translated.read_register(0, r(3)).unwrap(), 0x4433_2211);
            assert_eq!(translated.read_register(0, r(12)).unwrap(), 0x18000);
            assert_eq!(translated.read_register(0, r(14)).unwrap(), 0x1357_9bdf);
        }
    }
}

#[test]
fn indexed_stores_match_on_watched_code_pages() {
    let make = |blocks| {
        indexed_transfer_pair(
            blocks,
            0x10804,
            0x10804,
            0xdead_beef,
            [0xf843, 0x3c04],
            [0xf84c, 0xeb04],
        )
    };
    let mut reference = make(false);
    let mut translated = make(true);
    reference.run_until(100, 64).unwrap();
    translated.run_until(100, 64).unwrap();
    assert_eq!(outcome(&translated).registers, outcome(&reference).registers);
    assert_eq!(outcome(&translated).memory, outcome(&reference).memory);
    assert_eq!(translated.state(), reference.state());
    assert_eq!(
        translated.state.borrow().instructions,
        reference.state.borrow().instructions
    );
    assert_eq!(
        translated.state.borrow().timeline.now(),
        reference.state.borrow().timeline.now()
    );
    let mut actual = [0; 8];
    let mut expected = [0; 8];
    translated.read_physical(0x10800, &mut actual).unwrap();
    reference.read_physical(0x10800, &mut expected).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(&actual[..4], &0x10804u32.to_le_bytes());
    assert_eq!(&actual[4..], &0xdead_beefu32.to_le_bytes());
    let blocks = translated.state.borrow();
    let blocks = blocks.blocks[0].as_ref().unwrap();
    assert!(blocks.restart_callbacks > 0, "watched stores were not restarted");
    assert!(blocks.fast_stores);
}

/// Which hooks the execution core holds at an instant is a host choice. A
/// keyframe taken before block execution set its hooks up restores after it,
/// one taken in block execution restores with the instruction observer, and
/// the portable form is the same bytes either way.
#[test]
fn keyframes_do_not_bind_to_the_execution_core_s_own_hooks() {
    let reference = run(100, false, 2500, &[], ExecutionPolicy::Exact);
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
                released: true, parked_until_sgi: false,
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
                    released: true, parked_until_sgi: false,
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
                    released: true, parked_until_sgi: false,
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
/// Isolated PUSH at the end of a translated block. The watched case writes
/// into the code page; the fault case places the second word just beyond RAM.
fn push_program(blocks: bool, stack: u32, wide: bool) -> Board {
    let mut b = board(blocks);
    b.set_execution_policy(ExecutionPolicy::Interactive)
        .unwrap();
    b.state.borrow_mut().settled.verify = 0;
    b.write_physical(0x10000, &stack.to_le_bytes()).unwrap();

    // movs r4,#0x44; movs r5,#0x55; push {r4,r5}; adds r6,#1; b .
    let code: &[u16] = if wide {
        &[0x2444, 0x2555, 0xe92d, 0x0030, 0x3601, 0xe7fd]
    } else {
        &[0x2444, 0x2555, 0xb430, 0x3601, 0xe7fd]
    };
    let bytes: Vec<u8> = code.iter().flat_map(|half| half.to_le_bytes()).collect();
    b.write_physical(0x10100, &bytes).unwrap();
    b
}

fn stack_words(board: &Board, base: u32) -> [u8; 8] {
    let mut bytes = [0; 8];
    board.read_physical(base.into(), &mut bytes).unwrap();
    bytes
}

#[test]
fn terminal_pushes_match_observer_on_watched_ram_and_partial_fault() {
    for wide in [false, true] {
        // Both words are in the translated code page. This exercises the
        // watched-store terminal path without replaying an earlier push word.
        let mut reference = push_program(false, 0x10808, wide);
        let mut translated = push_program(true, 0x10808, wide);
        let a = reference.run_until(3, 3);
        let b = translated.run_until(3, 3);
        assert!(a.is_ok(), "watched reference PUSH: {a:?}");
        assert!(b.is_ok(), "watched translated PUSH: {b:?}");
        let expected = outcome(&reference);
        let actual = outcome(&translated);
        assert_eq!(actual.registers, expected.registers, "wide={wide}");
        assert_eq!(actual.state, expected.state, "wide={wide}");
        assert_eq!(actual.memory, expected.memory, "wide={wide}");
        assert_eq!(
            stack_words(&translated, 0x10800),
            stack_words(&reference, 0x10800)
        );
        let watched_words = stack_words(&translated, 0x10800);
        assert_eq!(&watched_words[..4], &0x44u32.to_le_bytes());
        assert_eq!(&watched_words[4..], &0x55u32.to_le_bytes());
        assert_eq!(actual.registers[4], 0x44);
        assert_eq!(actual.registers[5], 0x55);
        assert_eq!(
            scalar(translated.cpu(0).unwrap(), Register::Sp).unwrap(),
            0x10800
        );
        assert_eq!(
            translated.state.borrow().instructions,
            reference.state.borrow().instructions
        );
        assert_eq!(
            translated.state.borrow().timeline.now(),
            reference.state.borrow().timeline.now()
        );
        let blocks = translated.state.borrow();
        let stats = blocks.blocks[0].as_ref().unwrap();
        assert!(stats.watching, "wide={wide}");
        assert!(stats.watched.contains(&0x10000), "wide={wide}");
        assert!(
            stats.store_callbacks >= 2,
            "wide={wide}: {}",
            stats.store_callbacks
        );
        assert_eq!(stats.restart_callbacks, 0, "wide={wide}");

        // The first push word lands at the final mapped word. The second
        // faults at 0x18000; its prior successful write must remain visible.
        let mut reference = push_program(false, 0x18004, wide);
        let mut translated = push_program(true, 0x18004, wide);
        let a = reference.run_until(3, 3);
        let b = translated.run_until(3, 3);
        assert!(
            a.as_ref()
                .err()
                .is_some_and(|e| e.to_ascii_lowercase().contains("unmapped")),
            "expected observer PUSH fault, got {a:?}"
        );
        assert!(
            b.as_ref()
                .err()
                .is_some_and(|e| e.to_ascii_lowercase().contains("unmapped")),
            "expected translated PUSH fault, got {b:?}"
        );
        let expected = outcome(&reference);
        let actual = outcome(&translated);
        assert_eq!(actual.registers, expected.registers, "fault wide={wide}");
        assert_eq!(actual.state, expected.state, "fault wide={wide}");
        assert_eq!(actual.memory, expected.memory, "fault wide={wide}");
        assert_eq!(
            translated.state.borrow().instructions,
            reference.state.borrow().instructions,
            "fault wide={wide}"
        );
        assert_eq!(
            translated.state.borrow().timeline.now(),
            reference.state.borrow().timeline.now(),
            "fault wide={wide}"
        );
        let mut committed = [0; 4];
        translated.read_physical(0x17ffc, &mut committed).unwrap();
        assert_eq!(u32::from_le_bytes(committed), 0x44, "fault wide={wide}");
    }
}

