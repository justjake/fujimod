//! The board store hook of an A-profile peer is installed only while a
//! logical exclusive reservation is live. The peer's store must still reach
//! that reservation.
use super::*;
use fujimod_catalog_configuration::RunEngine;
use fujimod_emulator_cpu::Model;

/// A Cortex-M4 loads exclusively from shared RAM and later stores
/// exclusively. An AArch64 peer stores to the same word once.
/// Returns the M4's STREX status, the word, and the hook installs.
fn run(quantum: usize) -> (u64, u32, u64) {
    let (status, word, installs, _) = run_watching(quantum, &[]);
    (status, word, installs)
}

/// As `run`, with PC witnesses armed before the first instruction. Also
/// returns the witnessed (core, pc) pairs.
fn run_watching(quantum: usize, watched: &[u64]) -> (u64, u32, u64, Vec<(u64, u64)>) {
    use factory::*;
    let arm32 = |r| Register::Arm32(fujimod_emulator_cpu::arm::Register::R(r));
    let graph = Graph {
        decoders: vec![],
        processors: vec![
            Processor {
                model: Model::CortexM4,
                registers: vec![(arm32(0), 0x11000), (arm32(3), 0xaabbccdd)],
                released: true,
                rprofile: None,
                mprofile: Some(super::mprofile::Config {
                    clock_hz: 1_000_000_000,
                    board_ticks_per_second: 1_000_000_000,
                    vector_base: 0x10000,
                }),
            },
            Processor {
                model: Model::ArmV8A,
                registers: vec![
                    (Register::Pc, 0x14000),
                    (Register::Pstate, 0x3c5),
                    (Register::X(1), 0x11000),
                    (Register::X(2), 99),
                ],
                released: true,
                rprofile: None,
                mprofile: None,
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
    let words = |words: &[u32]| words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>();
    let halfwords =
        |words: &[u16]| words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>();
    board.write_physical(0x10000, &words(&[0x13000, 0x10101])).unwrap();
    // ldrex r1,[r0]; nop; nop; nop; strex r2,r3,[r0]; wfi
    board
        .write_physical(
            0x10100,
            &halfwords(&[0xe850, 0x1f00, 0xbf00, 0xbf00, 0xbf00, 0xe840, 0x3200, 0xbf30]),
        )
        .unwrap();
    // str w2,[x1]; b .
    board.write_physical(0x14000, &words(&[0xb9000022, 0x14000000])).unwrap();
    board.write_physical(0x11000, &words(&[0x11223344])).unwrap();
    board.watch_pcs(watched).unwrap();
    board.run_until(40, quantum).unwrap();
    let status = scalar(board.cpu(0).unwrap(), arm32(2)).unwrap();
    let mut word = [0; 4];
    board.read_physical(0x11000, &mut word).unwrap();
    let mut witnessed: Vec<_> = board
        .pc_entries()
        .iter()
        .map(|entry| (entry["core"].as_u64().unwrap(), entry["pc"].as_u64().unwrap()))
        .collect();
    witnessed.sort_unstable();
    (
        status,
        u32::from_le_bytes(word),
        board.store_hook_installs.get(),
        witnessed,
    )
}

#[test]
fn a_peer_store_reaches_a_live_reservation_through_the_on_demand_hook() {
    // The peer's slice begins while the reservation is live: its store hook
    // is installed, the store clears the reservation and STREX fails.
    for quantum in [1, 2, 3] {
        let (status, word, installs) = run(quantum);
        assert_eq!((status, word), (1, 99), "quantum {quantum}");
        assert!(installs > 0, "quantum {quantum}");
    }
    // The M4 finishes the pair inside one slice: no reservation is live when
    // the peer runs, so its stores keep the engine's fast path.
    let (status, word, installs) = run(64);
    assert_eq!((status, word, installs), (0, 99, 0));
}

#[test]
fn one_span_hook_witnesses_exactly_the_watched_entries_of_an_every_instruction_processor() {
    // The M4 runs three nops at 0x10104, 0x10106 and 0x10108 between its
    // exclusive pair; the peer runs 0x14000 and 0x14004. The unwatched nop
    // inside the watched span leaves no witness.
    let (status, word, _, witnessed) = run_watching(64, &[0x10104, 0x10108, 0x14004]);
    assert_eq!((status, word), (0, 99));
    assert_eq!(witnessed, [(0, 0x10104), (0, 0x10108), (1, 0x14004)]);
}
