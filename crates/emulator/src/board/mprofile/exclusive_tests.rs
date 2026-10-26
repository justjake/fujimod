use super::*;
use crate::board::{
    Board,
    bus::{Bus, Mode},
    factory::{Engine, Graph, MemoryMap, Options, Processor, Ram},
};
use fujimod_emulator_cpu::Model;
use fujimod_emulator_cpu::timeline::Ratio;
pub(super) fn board(code: &[u16], peer: &[u16]) -> Board {
    board_with_bus(code, peer, Bus::new(Mode::Strict))
}
pub(super) fn board_with_bus(code: &[u16], peer: &[u16], bus: Bus) -> Board {
    let graph = Graph {
        decoders: vec![],
        processors: (0..2)
            .map(|core| Processor {
                model: Model::CortexM4,
                released: true,
                registers: vec![
                    (reg(0), 0x11000),
                    (reg(3), 0xaabbccdd),
                    (reg(4), 99),
                    (reg(5), 0x5000),
                ],
                rprofile: None,
                mprofile: Some(Config {
                    clock_hz: 600_000_000,
                    board_ticks_per_second: 1_000_000_000,
                    vector_base: if core == 0 { 0x10000 } else { 0 },
                }),
            })
            .collect(),
        dma_mappings: vec![
            crate::board::memory::BusMapping::ram(0x10000, 0x8000),
            crate::board::memory::BusMapping {
                address: 0x40010000,
                physical: 0x10000,
                size: 0x8000,
                permissions: 3,
            },
        ],
        ram: vec![Ram {
            base: 0x10000,
            size: 0x8000,
        }],
        mappings: vec![
            MemoryMap {
                core: 0,
                address: 0x10000,
                physical: 0x10000,
                size: 0x8000,
                permissions: 7,
            },
            MemoryMap {
                core: 0,
                address: 0x20000,
                physical: 0x10000,
                size: 0x8000,
                permissions: 7,
            },
            MemoryMap {
                core: 1,
                address: 0,
                physical: 0x14000,
                size: 0x4000,
                permissions: 7,
            },
            MemoryMap {
                core: 1,
                address: 0x4000,
                physical: 0x10000,
                size: 0x4000,
                permissions: 7,
            },
        ],
        peripherals: vec![],
        instructions_per_tick: Ratio::new(1, 1).unwrap(),
    };
    let b = graph
        .build(
            Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1024 * 1024),
            bus,
            None,
        )
        .unwrap();
    for (at, words) in [(0x10000, [0x13000u32, 0x10101]), (0x14000, [0x3000, 0x101])] {
        b.write_memory(
            0,
            at,
            &words
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    }
    for (at, words) in [(0x10100, code), (0x14100, peer)] {
        b.write_memory(
            0,
            at,
            &words
                .iter()
                .flat_map(|n| n.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
    }
    b.write_memory(0, 0x11000, &0x11223344u32.to_le_bytes())
        .unwrap();
    b
}
fn memory(b: &Board) -> u32 {
    let mut v = [0; 4];
    b.read_memory(0, 0x11000, &mut v).unwrap();
    u32::from_le_bytes(v)
}
#[test]
fn widths_clear_and_external_aba_keep_exact_bytes() {
    for quantum in [1, 64] {
        for (load, store, size) in [
            ([0xe850, 0x1f00], [0xe840, 0x3200], 4),
            ([0xe8d0, 0x1f4f], [0xe8c0, 0x3f42], 1),
            ([0xe8d0, 0x1f5f], [0xe8c0, 0x3f52], 2),
        ] {
            let code = [load[0], load[1], store[0], store[1], 0xbf30];
            let mut b = board(&code, &[0xbf30]);
            b.run_until(20, quantum).unwrap();
            assert_eq!(get(b.cpu(0).unwrap(), reg(2)).unwrap(), 0);
            let mask = u32::MAX >> (32 - size * 8);
            assert_eq!(memory(&b), (0x11223344 & !mask) | (0xaabbccdd & mask));
            let mut b = board(&code, &[0xbf30]);
            b.run_for_instructions(1, quantum).unwrap();
            b.write_memory(1, 0x5000, &0x87654321u32.to_le_bytes())
                .unwrap();
            b.write_memory(1, 0x5000, &0x11223344u32.to_le_bytes())
                .unwrap();
            b.run_until(20, quantum).unwrap();
            assert_eq!(get(b.cpu(0).unwrap(), reg(2)).unwrap(), 1);
            assert_eq!(memory(&b), 0x11223344);
            let mut b = board(
                &[load[0], load[1], 0xf3bf, 0x8f2f, store[0], store[1], 0xbf30],
                &[0xbf30],
            );
            b.run_until(20, quantum).unwrap();
            assert_eq!(get(b.cpu(0).unwrap(), reg(2)).unwrap(), 1);
            assert_eq!(memory(&b), 0x11223344);
        }
    }
}
#[test]
fn actual_peer_store_through_an_alias_invalidates_the_load() {
    let mut b = board(&[0xe850, 0x1f00, 0xe840, 0x3200, 0xbf30], &[0x602c, 0xbf30]); // str r4,[r5]
    b.run_until(20, 1).unwrap();
    assert_eq!(get(b.cpu(0).unwrap(), reg(2)).unwrap(), 1);
    assert_eq!(memory(&b), 99);
}

#[test]
fn a_physical_bus_alias_invalidates_the_framework_monitor_by_canonical_ram() {
    let mut b = board(&[0xe850, 0x1f00, 0xe840, 0x3200, 0xbf30], &[0xbf30]);
    b.run_for_instructions(1, 1).unwrap();
    b.write_physical(0x40011000, &0x99887766u32.to_le_bytes())
        .unwrap();
    b.write_physical(0x40011000, &0x11223344u32.to_le_bytes())
        .unwrap();
    b.run_until(20, 1).unwrap();
    assert_eq!(get(b.cpu(0).unwrap(), reg(2)).unwrap(), 1);
    assert_eq!(memory(&b), 0x11223344);
}
#[test]
fn unrelated_granules_preserve_the_monitor_and_invalid_accesses_refuse() {
    let code = [0xe850, 0x1f00, 0xe840, 0x3200, 0xbf30];
    let mut b = board(&code, &[0xbf30]);
    b.run_for_instructions(1, 1).unwrap();
    b.write_memory(1, 0x5040, &99u32.to_le_bytes()).unwrap();
    b.run_until(20, 1).unwrap();
    assert_eq!(get(b.cpu(0).unwrap(), reg(2)).unwrap(), 0);
    assert_eq!(memory(&b), 0xaabbccdd);
    for address in [0x11001, 0xe000e010] {
        let mut b = board(&[0xbf00, 0xe850, 0x1f00, 0xe840, 0x3200, 0xbf30], &[0xbf30]);
        b.run_for_instructions(1, 1).unwrap();
        set(b.cpu(0).unwrap(), reg(0), address).unwrap();
        assert!(b.run_until(20, 64).is_err());
        assert_eq!(get(b.cpu(0).unwrap(), Register::Pc).unwrap(), 0x10102);
        assert_eq!(get(b.cpu(0).unwrap(), reg(1)).unwrap(), 0);
        assert_eq!(memory(&b), 0x11223344);
    }
    let mut b = board(&code, &[0xbf30]);
    b.cpu(0).unwrap().protect(0x11000, 0x1000, 1).unwrap();
    assert!(b.run_until(20, 64).is_err());
    assert_eq!(get(b.cpu(0).unwrap(), Register::Pc).unwrap(), 0x10104);
    assert_eq!(memory(&b), 0x11223344);
}
#[test]
fn thumb_it_conditions_and_monitor_clearing_on_exception() {
    // LDREX; CMP r1,#0; ITE EQ; STREXEQ; MOVNE r2,#7; WFI.
    for initial in [0u32, 1] {
        let mut b = board(
            &[
                0xe850, 0x1f00, 0x2900, 0xbf0c, 0xe840, 0x3200, 0x2207, 0xbf30,
            ],
            &[0xbf30],
        );
        b.write_memory(0, 0x11000, &initial.to_le_bytes()).unwrap();
        b.run_until(20, 64).unwrap();
        assert_eq!(
            get(b.cpu(0).unwrap(), reg(2)).unwrap(),
            if initial == 0 { 0 } else { 7 }
        );
        assert_eq!(memory(&b), if initial == 0 { 0xaabbccdd } else { 1 });
    }
    let mut b = board(&[0xe850, 0x1f00, 0xdf00, 0xe840, 0x3200, 0xbf30], &[0xbf30]);
    b.write_memory(0, 0x10000 + 11 * 4, &0x10201u32.to_le_bytes())
        .unwrap();
    b.write_memory(0, 0x10200, &0x4770u16.to_le_bytes())
        .unwrap(); // BX LR, real exception return
    b.run_until(30, 64).unwrap();
    assert_eq!(get(b.cpu(0).unwrap(), reg(2)).unwrap(), 1);
    assert_eq!(memory(&b), 0x11223344);
}
