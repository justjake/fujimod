//! Synthetic seqlock publication through a different CPU view of canonical RAM.
use super::super::{bus, factory};
use fujimod_emulator_cpu::{Model, Register, arm, timeline::Ratio};
use fujimod_emulator_cpu_unicorn::rprofile;

/// One core with cached and uncached views of RAM at 0x10000 and code RAM at
/// 0x20000.
fn two_view_board() -> (super::super::Board, Vec<factory::MemoryMap>) {
    use super::super::memory::BusMapping;
    use factory::{Engine, Graph, MemoryMap, Options, Processor, Ram};
    let maps = vec![
        MemoryMap {
            core: 0,
            address: 0x10000,
            physical: 0x10000,
            size: 0x1000,
            permissions: 7,
        },
        MemoryMap {
            core: 0,
            address: 0x40010000,
            physical: 0x10000,
            size: 0x1000,
            permissions: 7,
        },
        MemoryMap {
            core: 0,
            address: 0x20000,
            physical: 0x20000,
            size: 0x1000,
            permissions: 7,
        },
    ];
    let graph = Graph {
        processors: vec![Processor {
            model: Model::CortexR5F,
            released: true,
            mprofile: None,
            rprofile: Some(rprofile::core::Options {
                cp14_identity: 0,
                tcm: rprofile::tcm::Options {
                    atcm: rprofile::tcm::BankOptions {
                        bytes: 0x1000,
                        reset: 0xffff0001,
                    },
                    btcm: Default::default(),
                },
            }),
            registers: vec![(Register::Pc, 0x20000), (Register::Pstate, 0xd3)],
        }],
        ram: vec![
            Ram {
                base: 0x10000,
                size: 0x1000,
            },
            Ram {
                base: 0x20000,
                size: 0x1000,
            },
        ],
        mappings: maps.clone(),
        dma_mappings: vec![
            BusMapping::ram(0x10000, 0x1000),
            BusMapping::ram(0x20000, 0x1000),
            BusMapping {
                address: 0x40010000,
                physical: 0x10000,
                size: 0x1000,
                permissions: 3,
            },
        ],
        decoders: vec![],
        peripherals: vec![],
        instructions_per_tick: Ratio::new(1, 1).unwrap(),
    };
    let board = graph
        .build(
            Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1024 * 1024),
            bus::Bus::new(bus::Mode::Strict),
            None,
        )
        .unwrap();
    (board, maps)
}

#[test]
fn cached_read_watch_wakes_for_every_uncached_publication_store() {
    let (mut board, maps) = two_view_board();
    let code: [u32; 8] = [
        0xe3a00001, // mov r0,#1: publication busy
        0xe5810000, // str r0,[r1]: begin
        0xe5810008, // str r0,[r1,#8]: end
        0xe5810004, // str r0,[r1,#4]: publication number precedes completion
        0xe3a00002, // mov r0,#2: publication complete
        0xe5810008, // str r0,[r1,#8]: end
        0xe5810000, // str r0,[r1]: begin
        0xeafffffe,
    ];
    board
        .write_physical(
            0x20000,
            &code
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    board.write_register(0, arm::CORE[1], 0x40010000).unwrap();
    let mut observed = vec![];
    loop {
        board.read_set_begin().unwrap();
        let mut bytes = [0; 12];
        board.read_physical(0x10000, &mut bytes).unwrap();
        let reads = board.read_set_end().unwrap();
        assert_eq!(reads.stores, vec![(0x10000, 0x1000c)]);
        let words: Vec<_> = bytes
            .chunks_exact(4)
            .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
            .collect();
        observed.push(words.clone());
        if words[0] != 0 && words[0] == words[2] && words[0] & 1 == 0 {
            break;
        }
        assert!(observed.len() <= 5, "completed snapshot was never observed");
        board
            .continuation_begin(reads.stores, reads.devices, maps.clone())
            .unwrap();
        board.run_for_instructions(100, 7).unwrap();
        assert!(
            board.continuation_end().unwrap(),
            "alias store did not wake the predicate"
        );
    }
    assert_eq!(
        observed,
        vec![
            vec![0, 0, 0],
            vec![1, 0, 0],
            vec![1, 0, 1],
            vec![1, 1, 1],
            vec![1, 1, 2],
            vec![2, 1, 2]
        ]
    );
    assert_eq!(board.instruction_counts(), vec![7]);
}

#[test]
fn reads_in_order_record_one_span_and_others_union_at_the_end() {
    let mut spans = vec![];
    for word in 0..1000u64 {
        super::add_span(&mut spans, (0x1000 + word * 4, 0x1004 + word * 4));
    }
    assert_eq!(spans, vec![(0x1000, 0x1000 + 4000)]);
    super::add_span(&mut spans, (0x100, 0x104));
    super::add_span(&mut spans, (0x1008, 0x100c));
    super::add_span(&mut spans, (0x104, 0x108));
    assert_eq!(
        super::union(spans),
        vec![(0x100, 0x108), (0x1000, 0x1000 + 4000)]
    );
}

#[test]
fn stores_that_keep_watched_bytes_leave_the_wait_running() {
    let mut stops = super::Stops {
        stores: vec![(0x100, 0x108), (0x200, 0x30_0000)],
        values: vec![vec![1, 2, 3, 4, 5, 6, 7, 8], vec![]],
        ..Default::default()
    };
    stops.store(0x102, &[3, 4]);
    stops.store(0xfe, &[9, 9, 1]);
    assert!(!stops.fired);
    stops.store(0x107, &[0]);
    assert!(stops.fired);
    assert_eq!(
        stops.first_stop,
        Some(super::ContinuationStop::Store {
            range_index: 0,
            range: (0x100, 0x108),
            address: 0x107,
            change_confirmed: true,
        })
    );
    stops.store(0x100, &[9]);
    assert_eq!(
        stops.first_stop,
        Some(super::ContinuationStop::Store {
            range_index: 0,
            range: (0x100, 0x108),
            address: 0x107,
            change_confirmed: true,
        }),
        "later stores must not replace the first stop"
    );
    let mut large = super::Stops {
        stores: vec![(0x200, 0x30_0000)],
        values: vec![vec![]],
        ..Default::default()
    };
    large.store(0x200, &[0]);
    assert!(large.fired);
    assert_eq!(
        large.first_stop,
        Some(super::ContinuationStop::Store {
            range_index: 0,
            range: (0x200, 0x30_0000),
            address: 0x200,
            change_confirmed: false,
        })
    );
    let mut many = super::Stops {
        stores: (0..512u64).map(|n| (n * 0x38, n * 0x38 + 4)).collect(),
        values: vec![vec![0; 4]; 512],
        ..Default::default()
    };
    many.store(0x38 * 300 + 4, &[1; 0x30]);
    many.store(0x38 * 511, &[0; 4]);
    assert!(!many.fired);
    many.store(0x38 * 300 - 1, &[0, 0, 1]);
    assert!(many.fired);
}

#[test]
fn continuation_stop_keeps_the_first_device_or_store_cause() {
    let mut device = super::Stops::default();
    device.device(7);
    device.device(11);
    assert!(device.fired);
    assert_eq!(
        device.first_stop,
        Some(super::ContinuationStop::Device { id: 7 })
    );

    let mut store_then_device = super::Stops {
        stores: vec![(0x100, 0x104)],
        values: vec![vec![0; 4]],
        ..Default::default()
    };
    store_then_device.store(0x102, &[1]);
    store_then_device.device(7);
    assert_eq!(
        store_then_device.first_stop,
        Some(super::ContinuationStop::Store {
            range_index: 0,
            range: (0x100, 0x104),
            address: 0x102,
            change_confirmed: true,
        })
    );
}

#[test]
fn board_exposes_the_stop_until_continuation_end() {
    let (mut board, maps) = two_view_board();
    board
        .write_physical(0x20000, &0xe5810000u32.to_le_bytes()) // str r0,[r1]
        .unwrap();
    board.write_register(0, Register::Pc, 0x20000).unwrap();
    board.write_register(0, arm::CORE[0], 0x5a).unwrap();
    board.write_register(0, arm::CORE[1], 0x10000).unwrap();
    board
        .continuation_begin(vec![(0x10000, 0x10004)], Default::default(), maps)
        .unwrap();

    board.run_for_instructions(10, 1).unwrap();
    assert_eq!(
        board.continuation_stop(),
        Some(super::ContinuationStop::Store {
            range_index: 0,
            range: (0x10000, 0x10004),
            address: 0x10000,
            change_confirmed: true,
        })
    );
    assert!(board.continuation_end().unwrap());
    assert_eq!(board.continuation_stop(), None);
    assert_eq!(board.instruction_counts(), vec![1]);
}

#[test]
fn a_new_watch_set_replaces_the_retained_hooks() {
    let (mut board, maps) = two_view_board();
    assert_eq!(board.mappings(), maps);
    let code: [u32; 3] = [
        0xe5810000, // str r0,[r1]
        0xe5810010, // str r0,[r1,#16]
        0xeafffffe,
    ];
    board
        .write_physical(
            0x20000,
            &code
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    board.write_register(0, arm::CORE[1], 0x40010000).unwrap();
    // Runs from the first store with a new value; returns the instructions
    // entered and whether the watch fired.
    let mut run = |board: &mut super::super::Board, value: u64, watched: Option<(u64, u64)>| {
        board.write_register(0, Register::Pc, 0x20000).unwrap();
        board.write_register(0, arm::CORE[0], value).unwrap();
        if let Some(range) = watched {
            board
                .continuation_begin(vec![range], Default::default(), maps.clone())
                .unwrap();
        }
        let before = board.instruction_counts()[0];
        board.run_for_instructions(20, 7).unwrap();
        let fired = watched.is_some() && board.continuation_end().unwrap();
        (board.instruction_counts()[0] - before, fired)
    };
    let second = Some((0x10010, 0x10014));
    assert_eq!(run(&mut board, 1, second), (2, true));
    // The same set reuses its hooks and still stops at the changed word.
    assert_eq!(run(&mut board, 2, second), (2, true));
    // Another set watches only its own range.
    assert_eq!(run(&mut board, 3, Some((0x10000, 0x10004))), (1, true));
    // Retained hooks leave an unwatched run alone.
    assert_eq!(run(&mut board, 4, None), (20, false));
    assert_eq!(run(&mut board, 5, second), (2, true));
}
