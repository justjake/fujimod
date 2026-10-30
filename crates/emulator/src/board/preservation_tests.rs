use super::bus::{Bus, Mode};
use super::factory::{Engine, Graph, MemoryMap, Options, Processor, Ram};
use super::memory::BusMapping;
use super::preservation::{
    PhysicalRange, PreservationBusSource, PreservationFault, PreservationState,
    PreservationViolation, ProtectedRanges,
};
use super::*;
use fujimod_emulator_cpu::{Model, Register, arm, timeline::Ratio};

fn board(alias: bool) -> Board {
    let mut mappings = vec![MemoryMap {
        core: 0,
        address: 0x10000,
        physical: 0x10000,
        size: 0x4000,
        permissions: 7,
    }];
    if alias {
        mappings.push(MemoryMap {
            core: 0,
            address: 0x20000,
            physical: 0x10000,
            size: 0x4000,
            permissions: 3,
        });
    }
    mapped_board(mappings, vec![])
}

fn mapped_board(mappings: Vec<MemoryMap>, decoders: Vec<decoder::Decoder>) -> Board {
    Graph {
        processors: vec![Processor {
            model: Model::CortexR5F,
            registers: vec![(Register::Pc, 0x10000)],
            released: true,
            mprofile: None,
            rprofile: Some(Default::default()),
        }],
        ram: vec![Ram {
            base: 0x10000,
            size: 0x4000,
        }],
        dma_mappings: vec![BusMapping::ram(0x10000, 0x4000)],
        decoders,
        mappings,
        peripherals: vec![],
        instructions_per_tick: Ratio::new(1, 1).unwrap(),
    }
    .build(
        Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1 << 20),
        Bus::new(Mode::Strict),
        None,
    )
    .unwrap()
}

pub(super) fn two_core_board() -> Board {
    Graph {
        processors: vec![
            Processor {
                model: Model::CortexR5F,
                registers: vec![(Register::Pc, 0x10000)],
                released: true,
                mprofile: None,
                rprofile: Some(Default::default()),
            },
            Processor {
                model: Model::CortexR5F,
                registers: vec![(Register::Pc, 0x10000)],
                released: true,
                mprofile: None,
                rprofile: Some(Default::default()),
            },
        ],
        ram: vec![Ram {
            base: 0x10000,
            size: 0x4000,
        }],
        dma_mappings: vec![BusMapping::ram(0x10000, 0x4000)],
        decoders: vec![],
        mappings: vec![
            MemoryMap {
                core: 0,
                address: 0x10000,
                physical: 0x10000,
                size: 0x1000,
                permissions: 7,
            },
            MemoryMap {
                core: 1,
                address: 0x10000,
                physical: 0x11000,
                size: 0x1000,
                permissions: 7,
            },
        ],
        peripherals: vec![],
        instructions_per_tick: Ratio::new(1, 1).unwrap(),
    }
    .build(
        Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1 << 20),
        Bus::new(Mode::Strict),
        None,
    )
    .unwrap()
}

fn protected(ranges: &[(u64, u64)]) -> ProtectedRanges {
    ProtectedRanges::new(
        ranges
            .iter()
            .map(|&(lo, hi)| PhysicalRange::new(lo, hi).unwrap())
            .collect(),
    )
    .unwrap()
}

fn observed_ranges(state: &PreservationState) -> Vec<(u64, u64)> {
    state
        .ranges()
        .map(|ranges| {
            ranges
                .ranges()
                .iter()
                .map(|range| (range.start(), range.end()))
                .collect()
        })
        .unwrap_or_default()
}

fn code(board: &Board, words: &[u32]) {
    board
        .cpu(0)
        .unwrap()
        .write_memory(
            0x10000,
            &words
                .iter()
                .flat_map(|word| word.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
}

fn data(board: &Board, address: u64, bytes: &[u8]) {
    board.cpu(0).unwrap().write_memory(address, bytes).unwrap();
}

fn address(board: &Board, value: u32) {
    board
        .write_register(0, Register::Arm32(arm::Register::R(1)), u64::from(value))
        .unwrap();
}

fn run(board: &mut Board, instructions: u64) {
    board.run_for_instructions(instructions, 1).unwrap();
}

fn bytes(board: &Board, address: u64, count: usize) -> Vec<u8> {
    let mut bytes = vec![0; count];
    board.read_memory(0, address, &mut bytes).unwrap();
    bytes
}

#[test]
fn identity_store_counts_without_a_violation() {
    let mut board = board(false);
    code(&board, &[0xe5910000, 0xe5810000, 0xeafffffe]);
    data(&board, 0x10080, &[1, 2, 3, 4]);
    address(&board, 0x10080);
    let preservation = board
        .preservation_begin(protected(&[(0x10080, 0x10090)]))
        .unwrap();
    run(&mut board, 3);
    let state = preservation.state();
    assert_eq!(observed_ranges(&state), vec![(0x10080, 0x10090)]);
    assert_eq!(
        (
            state.identity_cpu_writes().get(),
            state.changed_cpu_writes().get()
        ),
        (1, 0)
    );
    assert_eq!(
        (state.host_writes().get(), state.model_bus_writes().get()),
        (0, 0)
    );
    assert_eq!(state.violation(), None);
    assert_eq!(bytes(&board, 0x10080, 4), vec![1, 2, 3, 4]);
}

#[test]
fn transient_change_and_restore_reports_the_first_store() {
    let mut board = board(false);
    code(
        &board,
        &[0xe3a00001, 0xe5810000, 0xe3a00000, 0xe5810000, 0xeafffffe],
    );
    data(&board, 0x10080, &[0; 4]);
    address(&board, 0x10080);
    let preservation = board
        .preservation_begin(protected(&[(0x10080, 0x10090)]))
        .unwrap();
    run(&mut board, 5);
    assert_eq!(bytes(&board, 0x10080, 4), vec![0; 4]);
    let state = preservation.state();
    assert_eq!(
        (
            state.identity_cpu_writes().get(),
            state.changed_cpu_writes().get()
        ),
        (0, 2)
    );
    let PreservationViolation::CpuChange {
        core,
        pc,
        address,
        physical,
        size,
        before,
        after,
        ..
    } = state.violation().cloned().unwrap()
    else {
        panic!("expected a changed CPU store");
    };
    assert_eq!(
        (core.get(), pc.get(), address.get(), physical.get()),
        (0, 0x10004, 0x10080, 0x10080)
    );
    assert_eq!(size.get(), 4);
    assert_eq!(before.as_slice(), &[0u8, 0, 0, 0][..]);
    assert_eq!(after.as_slice(), &[1u8, 0, 0, 0][..]);
}

#[test]
fn setend_big_endian_word_store_uses_the_callback_value_order() {
    for (initial, changed) in [
        ([0x11u8, 0x22, 0x33, 0x44], false),
        ([0x44u8, 0x33, 0x22, 0x11], true),
    ] {
        let mut board = board(false);
        code(
            &board,
            &[
                0xf1010200, // setend be
                0xe3030344, // movw r0,#0x3344
                0xe3410122, // movt r0,#0x1122
                0xe5810000, // str r0,[r1]
                0xeafffffe,
            ],
        );
        data(&board, 0x10080, &initial);
        address(&board, 0x10080);
        let preservation = board
            .preservation_begin(protected(&[(0x10080, 0x10090)]))
            .unwrap();
        run(&mut board, 5);
        assert_eq!(bytes(&board, 0x10080, 4), vec![0x11, 0x22, 0x33, 0x44]);
        let state = preservation.state();
        if changed {
            assert_eq!(
                (
                    state.identity_cpu_writes().get(),
                    state.changed_cpu_writes().get()
                ),
                (0, 1)
            );
            let PreservationViolation::CpuChange { before, after, .. } =
                state.violation().cloned().unwrap()
            else {
                panic!("expected a changed CPU store");
            };
            assert_eq!(before.as_slice(), &[0x44u8, 0x33, 0x22, 0x11][..]);
            assert_eq!(after.as_slice(), &[0x11u8, 0x22, 0x33, 0x44][..]);
        } else {
            assert_eq!(
                (
                    state.identity_cpu_writes().get(),
                    state.changed_cpu_writes().get()
                ),
                (1, 0)
            );
            assert_eq!(state.violation(), None);
        }
    }
}

#[test]
fn partial_overlap_compares_only_the_protected_intersection() {
    let mut unchanged = board(false);
    code(
        &unchanged,
        &[0xe30c0cdd, 0xe34b0baa, 0xe5810000, 0xeafffffe],
    );
    data(&unchanged, 0x10080, &[0x11, 0x22, 0xaa, 0xbb]);
    address(&unchanged, 0x10080);
    let preservation = unchanged
        .preservation_begin(protected(&[(0x10082, 0x10090)]))
        .unwrap();
    run(&mut unchanged, 4);
    assert_eq!(bytes(&unchanged, 0x10080, 4), vec![0xdd, 0xcc, 0xaa, 0xbb]);
    let state = preservation.state();
    assert_eq!(
        (
            state.identity_cpu_writes().get(),
            state.changed_cpu_writes().get()
        ),
        (1, 0)
    );
    assert_eq!(state.violation(), None);

    let mut changed = board(false);
    code(&changed, &[0xe30c0cdd, 0xe34b0bab, 0xe5810000, 0xeafffffe]);
    data(&changed, 0x10080, &[0x11, 0x22, 0xaa, 0xbb]);
    address(&changed, 0x10080);
    let preservation = changed
        .preservation_begin(protected(&[(0x10082, 0x10090)]))
        .unwrap();
    run(&mut changed, 4);
    let state = preservation.state();
    assert_eq!(
        (
            state.identity_cpu_writes().get(),
            state.changed_cpu_writes().get()
        ),
        (0, 1)
    );
    let PreservationViolation::CpuChange {
        address,
        physical,
        size,
        before,
        after,
        ..
    } = state.violation().cloned().unwrap()
    else {
        panic!("expected a changed CPU store");
    };
    assert_eq!(
        (address.get(), physical.get(), size.get()),
        (0x10082, 0x10082, 2)
    );
    assert_eq!(before.as_slice(), &[0xaau8, 0xbb][..]);
    assert_eq!(after.as_slice(), &[0xabu8, 0xbb][..]);
}

#[test]
fn alias_view_store_is_observed_as_the_protected_physical_bytes() {
    let mut board = board(true);
    code(&board, &[0xe3a00001, 0xe5810000, 0xeafffffe]);
    data(&board, 0x10080, &[0; 4]);
    address(&board, 0x20080);
    let preservation = board
        .preservation_begin(protected(&[(0x10080, 0x10090)]))
        .unwrap();
    run(&mut board, 3);
    let state = preservation.state();
    assert_eq!(
        (
            state.identity_cpu_writes().get(),
            state.changed_cpu_writes().get()
        ),
        (0, 1)
    );
    let PreservationViolation::CpuChange {
        address, physical, ..
    } = state.violation().cloned().unwrap()
    else {
        panic!("expected a changed CPU store");
    };
    assert_eq!((address.get(), physical.get()), (0x20080, 0x10080));
}

#[test]
fn host_and_model_bus_writes_are_unverifiable() {
    let board = board(false);
    let preservation = board
        .preservation_begin(protected(&[(0x10080, 0x10090)]))
        .unwrap();
    board.write_physical(0x10080, &[9]).unwrap();
    board.write_memory(0, 0x10084, &[8]).unwrap();
    {
        let state = board.state.borrow();
        board
            .physical_memory
            .write_dma(&board.cpus, &state.exclusive, 0x10088, &[7])
            .unwrap();
    }
    let state = preservation.state();
    assert_eq!(
        (state.host_writes().get(), state.model_bus_writes().get()),
        (2, 1)
    );
    let PreservationViolation::UnverifiableWrite {
        source,
        address,
        physical,
        size,
    } = state.violation().cloned().unwrap()
    else {
        panic!("expected an unverifiable write");
    };
    assert_eq!(source, PreservationBusSource::HostWrite);
    assert_eq!(
        (address.get(), physical.get(), size.get()),
        (0x10080, 0x10080, 1)
    );
}

#[test]
fn integrity_fault_dominates_an_earlier_cpu_change() {
    let mut board = board(false);
    code(&board, &[0xe3a00001, 0xe5810000, 0xeafffffe]);
    data(&board, 0x10080, &[0; 4]);
    address(&board, 0x10080);
    let preservation = board
        .preservation_begin(protected(&[(0x10080, 0x10090)]))
        .unwrap();
    run(&mut board, 2);
    let state = preservation.state();
    assert_eq!(
        (
            state.identity_cpu_writes().get(),
            state.changed_cpu_writes().get()
        ),
        (0, 1)
    );
    assert!(matches!(
        state.violation(),
        Some(PreservationViolation::CpuChange { .. })
    ));
    assert!(board.write_physical(0x20000, &[1]).is_err());
    let integrity = Some(PreservationViolation::IntegrityFault {
        reason: PreservationFault::UnresolvedMapping,
    });
    assert_eq!(preservation.state().violation().cloned(), integrity);
    board.write_physical(0x10084, &[9]).unwrap();
    board.write_memory(0, 0x10084, &[8]).unwrap();
    let state = preservation.state();
    assert_eq!(state.host_writes().get(), 2);
    assert_eq!(state.violation().cloned(), integrity);
}

#[test]
fn dropping_preservation_removes_hooks_and_rearms() {
    let mut board = board(false);
    code(&board, &[0xe5910000, 0xe5810000, 0xeafffffe]);
    data(&board, 0x10080, &[1, 2, 3, 4]);
    address(&board, 0x10080);
    {
        let preservation = board
            .preservation_begin(protected(&[(0x10080, 0x10090)]))
            .unwrap();
        run(&mut board, 3);
        assert_eq!(preservation.state().identity_cpu_writes().get(), 1);
    }
    let preservation = board
        .preservation_begin(protected(&[(0x10080, 0x10090)]))
        .unwrap();
    board.write_register(0, Register::Pc, 0x10000).unwrap();
    run(&mut board, 3);
    let state = preservation.state();
    assert_eq!(
        (
            state.identity_cpu_writes().get(),
            state.changed_cpu_writes().get()
        ),
        (1, 0)
    );
    assert_eq!(state.violation(), None);
    drop(preservation);
    let mut preservation = board
        .preservation_begin(protected(&[(0x10080, 0x10090)]))
        .unwrap();
    board.preservation_end(&mut preservation).unwrap();
    drop(preservation);
    let preservation = board
        .preservation_begin(protected(&[(0x10080, 0x10090)]))
        .unwrap();
    board.write_register(0, Register::Pc, 0x10000).unwrap();
    run(&mut board, 3);
    assert_eq!(preservation.state().identity_cpu_writes().get(), 1);
}

#[test]
fn host_write_uses_the_target_core_mapping() {
    let board = two_core_board();
    let preservation = board
        .preservation_begin(protected(&[(0x11080, 0x11090)]))
        .unwrap();
    board.write_memory(0, 0x10080, &[1]).unwrap();
    assert_eq!(preservation.state().host_writes().get(), 0);
    board.write_memory(1, 0x10080, &[2]).unwrap();
    let state = preservation.state();
    assert_eq!(state.host_writes().get(), 1);
    let PreservationViolation::UnverifiableWrite {
        address, physical, ..
    } = state.violation().cloned().unwrap()
    else {
        panic!("expected an unverifiable write");
    };
    assert_eq!((address.get(), physical.get()), (0x10080, 0x11080));
}

#[test]
fn empty_host_writes_stay_clean() {
    let board = board(false);
    let preservation = board
        .preservation_begin(protected(&[(0x10080, 0x10090)]))
        .unwrap();
    board.write_physical(0x10084, &[]).unwrap();
    let _ = board.write_memory(0, 0x10084, &[]);
    {
        let state = board.state.borrow();
        board
            .physical_memory
            .write_dma(&board.cpus, &state.exclusive, 0x10084, &[])
            .unwrap();
    }
    let state = preservation.state();
    assert_eq!(
        (state.host_writes().get(), state.model_bus_writes().get()),
        (0, 0)
    );
    assert_eq!(state.violation(), None);
}

#[test]
fn failed_host_mapping_marks_an_integrity_fault() {
    let board = board(false);
    let preservation = board
        .preservation_begin(protected(&[(0x10080, 0x10090)]))
        .unwrap();
    assert!(board.write_physical(0x20000, &[1]).is_err());
    assert_eq!(
        preservation.state().violation().cloned(),
        Some(PreservationViolation::IntegrityFault {
            reason: PreservationFault::UnresolvedMapping
        })
    );
}

#[test]
fn direct_cpu_write_memory_records_one_host_write() {
    let board = board(false);
    data(&board, 0x10080, &[1, 2, 3, 4]);
    let preservation = board
        .preservation_begin(protected(&[(0x10080, 0x10090)]))
        .unwrap();
    board
        .cpu(0)
        .unwrap()
        .write_memory(0x10080, &[1, 2, 3, 4])
        .unwrap();
    assert_eq!(bytes(&board, 0x10080, 4), vec![1, 2, 3, 4]);
    let state = preservation.state();
    assert_eq!(state.host_writes().get(), 1);
    assert!(matches!(
        state.violation(),
        Some(PreservationViolation::UnverifiableWrite {
            source: PreservationBusSource::HostWrite,
            ..
        })
    ));
}

#[test]
fn successful_end_disables_state_and_is_idempotent() {
    let mut board = board(false);
    code(&board, &[0xe5910000, 0xe5810000, 0xeafffffe]);
    data(&board, 0x10080, &[1, 2, 3, 4]);
    address(&board, 0x10080);
    let mut preservation = board
        .preservation_begin(protected(&[(0x10080, 0x10090)]))
        .unwrap();
    run(&mut board, 3);
    assert_eq!(preservation.state().identity_cpu_writes().get(), 1);
    board.preservation_end(&mut preservation).unwrap();
    board.preservation_end(&mut preservation).unwrap();
    board.write_register(0, Register::Pc, 0x10000).unwrap();
    run(&mut board, 3);
    assert_eq!(
        serde_json::to_value(preservation.state()).unwrap(),
        serde_json::json!({
            "enabled": false,
            "ranges": [],
            "identity_cpu_writes": 0,
            "changed_cpu_writes": 0,
            "host_writes": 0,
            "model_bus_writes": 0,
            "violation": null,
        })
    );
}

#[test]
fn begin_refuses_unsupported_ranges_and_conflicting_observers() {
    let board = board(false);
    assert!(ProtectedRanges::new(Vec::new()).is_err());
    assert!(
        board
            .preservation_begin(protected(&[(0x14000, 0x14010)]))
            .is_err()
    );
    let mut preservation = board
        .preservation_begin(protected(&[(0x10080, 0x10090), (0x10090, 0x100a0)]))
        .unwrap();
    assert!(
        board
            .memory_watch_begin(vec![(0x10080, 0x10090)], 8, board.mappings().to_vec())
            .is_err()
    );
    assert!(
        board
            .preservation_begin(protected(&[(0x10080, 0x10090)]))
            .is_err()
    );
    board.preservation_end(&mut preservation).unwrap();
    let mut watch = board
        .memory_watch_begin(vec![(0x10080, 0x10090)], 8, board.mappings().to_vec())
        .unwrap();
    assert!(
        board
            .preservation_begin(protected(&[(0x10080, 0x10090)]))
            .is_err()
    );
    board.memory_watch_end(&mut watch).unwrap();
}

// Adjacent CPU windows need not have adjacent physical destinations.
fn split_mapping_board(decoded: bool) -> Board {
    let mappings = vec![
        MemoryMap {
            core: 0,
            address: 0x10000,
            physical: 0x10000,
            size: 0x1000,
            permissions: 7,
        },
        MemoryMap {
            core: 0,
            address: 0x11000,
            physical: 0x12000,
            size: 0x1000,
            permissions: 7,
        },
    ];
    let decoders = if decoded {
        vec![decoder::Decoder {
            initial: 1,
            choices: vec![
                decoder::Choice {
                    processors: mappings
                        .iter()
                        .cloned()
                        .map(|mut map| {
                            map.physical = 0x13000;
                            map
                        })
                        .collect(),
                    dma: vec![],
                },
                decoder::Choice {
                    processors: mappings.clone(),
                    dma: vec![],
                },
            ],
        }]
    } else {
        vec![]
    };
    mapped_board(mappings, decoders)
}

#[test]
fn host_write_spanning_mappings_counts_once_at_the_protected_chunk() {
    for decoded in [false, true] {
        for (ranges, expected) in [
            (vec![(0x10ffe, 0x11000)], Some((0x10ffe, 0x10ffe, 2))),
            (vec![(0x12000, 0x12002)], Some((0x11000, 0x12000, 2))),
            (
                vec![(0x10ffe, 0x11000), (0x12000, 0x12002)],
                Some((0x10ffe, 0x10ffe, 2)),
            ),
            (vec![(0x13000, 0x13002)], None),
        ] {
            let board = split_mapping_board(decoded);
            let preservation = board.preservation_begin(protected(&ranges)).unwrap();
            board
                .cpu(0)
                .unwrap()
                .write_memory(0x10ffe, &[1, 2, 3, 4])
                .unwrap();
            assert_eq!(bytes(&board, 0x10ffe, 4), vec![1, 2, 3, 4]);
            let state = preservation.state();
            assert_eq!(state.host_writes().get(), u64::from(expected.is_some()));
            if let Some(expected) = expected {
                let Some(PreservationViolation::UnverifiableWrite {
                    source,
                    address,
                    physical,
                    size,
                }) = state.violation()
                else {
                    panic!(
                        "expected a resolved host write, got {:?}",
                        state.violation()
                    );
                };
                assert_eq!(*source, PreservationBusSource::HostWrite);
                assert_eq!((address.get(), physical.get(), size.get()), expected);
            } else {
                assert_eq!(state.violation(), None);
            }
        }
    }
}

#[test]
fn host_write_spanning_mappings_preserves_an_earlier_violation() {
    for decoded in [false, true] {
        let mut board = split_mapping_board(decoded);
        code(&board, &[0xe3a00001, 0xe5810000, 0xeafffffe]);
        address(&board, 0x10080);
        let preservation = board
            .preservation_begin(protected(&[(0x10080, 0x10084), (0x12000, 0x12002)]))
            .unwrap();
        run(&mut board, 2);
        let first = preservation.state().violation().cloned().unwrap();
        assert!(matches!(first, PreservationViolation::CpuChange { .. }));
        board.write_memory(0, 0x10ffe, &[1, 2, 3, 4]).unwrap();
        let state = preservation.state();
        assert_eq!(state.violation(), Some(&first));
        assert_eq!(state.host_writes().get(), 1);
        assert_eq!(state.changed_cpu_writes().get(), 1);
    }
}

#[test]
fn coherence_store_through_an_alias_is_counted_once() {
    for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
        for thumb in [false, true] {
            for changed in [false, true] {
                let mut board = board(true);
                board.set_execution_policy(policy).unwrap();
                let before = 0xe3a04001u32; // mov r4,#1
                let after = if changed { 0xe3a04002u32 } else { before };
                data(&board, 0x10080, &before.to_le_bytes());
                let writer = if thumb {
                    vec![0x08, 0x60, 0x01, 0x32, 0xfe, 0xe7] // str r0,[r1]; adds r2,#1; b .
                } else {
                    [0xe5810000u32, 0xe2822001, 0xeafffffe]
                        .into_iter()
                        .flat_map(u32::to_le_bytes)
                        .collect()
                };
                data(&board, 0x11000, &writer);
                // Translate the destination before overwriting it through its alias.
                board.write_register(0, Register::Pc, 0x10080).unwrap();
                board.run_for_instructions(1, 64).unwrap();
                assert_eq!(scalar(&*board.cpus[0], arm::CORE[4]).unwrap(), 1);
                board
                    .write_register(0, arm::CORE[0], u64::from(after))
                    .unwrap();
                address(&board, 0x20080);
                board.write_register(0, Register::Pc, 0x11000).unwrap();
                board
                    .write_register(0, Register::Pstate, if thumb { 0xf3 } else { 0xd3 })
                    .unwrap();
                let preservation = board
                    .preservation_begin(protected(&[(0x10080, 0x10084)]))
                    .unwrap();
                let counters = board.cpu(0).unwrap().exit_counters().unwrap();
                // A full block lets the coherence path request a store retry.
                board.run_for_instructions(8, 64).unwrap();
                let exits = board.cpu(0).unwrap().exit_counters().unwrap();
                assert!(
                    exits["restart_requests"] > counters["restart_requests"],
                    "{policy:?}, thumb={thumb}"
                );
                assert_eq!(bytes(&board, 0x10080, 4), after.to_le_bytes());
                assert_eq!(scalar(&*board.cpus[0], arm::CORE[2]).unwrap(), 1);
                let state = preservation.state();
                assert_eq!(
                    (
                        state.identity_cpu_writes().get(),
                        state.changed_cpu_writes().get()
                    ),
                    if changed { (0, 1) } else { (1, 0) },
                    "{policy:?}, thumb={thumb}"
                );
                assert_eq!(
                    (state.host_writes().get(), state.model_bus_writes().get()),
                    (0, 0)
                );
                if changed {
                    let Some(PreservationViolation::CpuChange {
                        pc,
                        address,
                        physical,
                        size,
                        before: observed_before,
                        after: observed_after,
                        ..
                    }) = state.violation()
                    else {
                        panic!("expected the changed store");
                    };
                    assert_eq!(
                        (pc.get(), address.get(), physical.get(), size.get()),
                        (0x11000, 0x20080, 0x10080, 4)
                    );
                    assert_eq!(observed_before.as_slice(), before.to_le_bytes());
                    assert_eq!(observed_after.as_slice(), after.to_le_bytes());
                } else {
                    assert_eq!(state.violation(), None);
                }
            }
        }
    }
}
