//! Keeping an M-profile processor's translations across a peer's slice must
//! run exactly the instructions that discarding them on every switch runs.
use super::*;
use fujimod_catalog_configuration::RunEngine;
use fujimod_emulator_cpu::Model;

/// A Cortex-M4 loop in RAM that a Cortex-R5F peer later patches through its
/// own view of the same RAM. The patch replaces `adds r0,#1` with
/// `adds r1,#1`, so R0 freezes at the M4 instruction where the store landed.
fn run(quantum: usize, retain: bool, patch: u32) -> (Value, [u64; 2], u64) {
    run_with(quantum, retain, patch, None)
}

/// As `run`, with an optional host write of `adds r2,#1` over the M4 loop
/// between two run windows, the way a timed patch or a poke lands.
fn run_with(
    quantum: usize,
    retain: bool,
    patch: u32,
    host_write_at: Option<u64>,
) -> (Value, [u64; 2], u64) {
    use factory::*;
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
                    vector_base: 0,
                }),
            },
            Processor {
                rprofile: Some(Default::default()),
                mprofile: None,
                model: Model::CortexR5F,
                registers: vec![(Register::Pc, 0x10000)],
                released: true,
            },
        ],
        dma_mappings: vec![
            crate::board::memory::BusMapping::ram(0, 0x4000),
            crate::board::memory::BusMapping::ram(0x10000, 0x4000),
        ],
        ram: vec![
            Ram {
                base: 0,
                size: 0x4000,
            },
            Ram {
                base: 0x10000,
                size: 0x4000,
            },
        ],
        mappings: vec![
            MemoryMap {
                core: 0,
                address: 0,
                physical: 0,
                size: 0x4000,
                permissions: 7,
            },
            MemoryMap {
                core: 1,
                address: 0x10000,
                physical: 0x10000,
                size: 0x4000,
                permissions: 7,
            },
            MemoryMap {
                core: 1,
                address: 0x20000,
                physical: 0,
                size: 0x4000,
                permissions: 3,
            },
        ],
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
    let mut board = board;
    if !retain {
        board.state.borrow_mut().code_shadow[0] = None;
    }
    let m4 = board.cpu(0).unwrap();
    m4.write_memory(
        0,
        &[0x3000u32, 0x1001]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    // adds r0,#1; b .-2
    m4.write_memory(0x1000, &[0x01, 0x30, 0xfd, 0xe7]).unwrap();
    // mov r3,#40; subs r3,r3,#1; bne .-4; mov r4,#0x21000; mov r5,#PATCH_HIGH;
    // orr r5,r5,#PATCH_LOW; strh r5,[r4]; b .
    let peer = [
        0xe3a03028u32,
        0xe2533001,
        0x1afffffd,
        0xe3a04a21,
        0xe3a05c00 | (patch >> 8),
        0xe3855000 | (patch & 0xff),
        0xe1c450b0,
        0xeafffffe,
    ];
    board
        .cpu(1)
        .unwrap()
        .write_memory(
            0x10000,
            &peer.into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>(),
        )
        .unwrap();
    if let Some(at) = host_write_at {
        board.run_until(at, quantum).unwrap();
        board.write_physical(0x1000, &[0x01, 0x32]).unwrap();
    }
    board.run_until(400, quantum).unwrap();
    let m4 = board.cpu(0).unwrap();
    let registers = [0, if host_write_at.is_some() { 2 } else { 1 }].map(|r| {
        scalar(m4, Register::Arm32(fujimod_emulator_cpu::arm::Register::R(r))).unwrap()
    });
    let retained = board.state.borrow().code_shadow[0]
        .as_ref()
        .map_or(0, |shadow| shadow.retained());
    (board.state(), registers, retained)
}

#[test]
fn retained_translations_run_the_instructions_that_discarded_translations_run() {
    for quantum in [1, 2, 3, 5, 8, 64] {
        // 0x3101 is adds r1,#1; 0x3001 rewrites the instruction unchanged.
        for patch in [0x3101, 0x3001] {
            let (discarded, discarded_registers, _) = run(quantum, false, patch);
            let (kept, kept_registers, retained) = run(quantum, true, patch);
            assert_eq!(kept_registers, discarded_registers, "quantum {quantum}");
            assert_eq!(kept, discarded, "quantum {quantum}");
            assert!(retained > 0, "quantum {quantum} never kept a translation");
            if patch == 0x3101 {
                assert!(kept_registers[1] > 0, "the peer's patch never ran");
                assert!(kept_registers[0] > 0, "the original loop never ran");
            } else {
                assert_eq!(kept_registers[1], 0);
            }
        }
    }
}

#[test]
fn a_host_write_between_run_windows_reaches_retained_translations() {
    for quantum in [1, 3, 8, 64] {
        // The peer rewrites the instruction unchanged; only the host patches it.
        let (discarded, discarded_registers, _) = run_with(quantum, false, 0x3001, Some(150));
        let (kept, kept_registers, retained) = run_with(quantum, true, 0x3001, Some(150));
        assert_eq!(kept_registers, discarded_registers, "quantum {quantum}");
        assert_eq!(kept, discarded, "quantum {quantum}");
        assert!(retained > 0, "quantum {quantum} never kept a translation");
        assert!(kept_registers[0] > 0 && kept_registers[1] > 0, "quantum {quantum}");
    }
}
