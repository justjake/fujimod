use super::*;
use crate::board::{
    Board,
    bus::{Bus, Mode},
    factory::{Graph, MemoryMap, Options, Processor, Ram},
};
use fujimod_catalog_configuration::{RunCpuBackend, RunEngine};
use fujimod_emulator_cpu::Model;
use fujimod_emulator_cpu::timeline::Ratio;
pub(super) fn board(backend: RunCpuBackend, code: &[u16], peer: &[u16]) -> Board {
    board_with_bus(backend, code, peer, Bus::new(Mode::Strict))
}
pub(super) fn board_with_bus(
    backend: RunCpuBackend,
    code: &[u16],
    peer: &[u16],
    bus: Bus,
) -> Board {
    board_with_permissions(backend, code, peer, bus, 7)
}
fn board_with_permissions(
    backend: RunCpuBackend,
    code: &[u16],
    peer: &[u16],
    bus: Bus,
    data_permissions: u32,
) -> Board {
    let graph = Graph {
        decoders: vec![],
        processors: (0..2)
            .map(|core| Processor {
                cpu_backend: backend,
                model: Model::CortexM4,
                released: true,
                parked_until_sgi: false,
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
                size: 0x1000,
                permissions: 7,
            },
            MemoryMap {
                core: 0,
                address: 0x11000,
                physical: 0x11000,
                size: 0x1000,
                permissions: data_permissions,
            },
            MemoryMap {
                core: 0,
                address: 0x12000,
                physical: 0x12000,
                size: 0x6000,
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
            Options::new(RunEngine::Unicorn).with_translation_cache_size_bytes(1024 * 1024),
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
    for backend in test_backends() {
        check_widths_clear_and_external_aba_keep_exact_bytes(backend);
    }
}

fn check_widths_clear_and_external_aba_keep_exact_bytes(backend: RunCpuBackend) {
    for quantum in [1, 64] {
        for (load, store, size) in [
            ([0xe850, 0x1f00], [0xe840, 0x3200], 4),
            ([0xe8d0, 0x1f4f], [0xe8c0, 0x3f42], 1),
            ([0xe8d0, 0x1f5f], [0xe8c0, 0x3f52], 2),
        ] {
            let code = [load[0], load[1], store[0], store[1], 0xbf30];
            let mut b = board(backend, &code, &[0xbf30]);
            b.run_until(20, quantum).unwrap();
            assert_eq!(get(b.cpu(0).unwrap(), reg(2)).unwrap(), 0);
            let mask = u32::MAX >> (32 - size * 8);
            assert_eq!(memory(&b), (0x11223344 & !mask) | (0xaabbccdd & mask));
            let mut b = board(backend, &code, &[0xbf30]);
            b.run_for_instructions(1, quantum).unwrap();
            b.write_memory(1, 0x5000, &0x87654321u32.to_le_bytes())
                .unwrap();
            b.write_memory(1, 0x5000, &0x11223344u32.to_le_bytes())
                .unwrap();
            b.run_until(20, quantum).unwrap();
            assert_eq!(get(b.cpu(0).unwrap(), reg(2)).unwrap(), 1);
            assert_eq!(memory(&b), 0x11223344);
            let mut b = board(
                backend,
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
    for backend in test_backends() {
        check_actual_peer_store_through_an_alias_invalidates_the_load(backend);
    }
}

fn check_actual_peer_store_through_an_alias_invalidates_the_load(backend: RunCpuBackend) {
    let mut b = board(
        backend,
        &[0xe850, 0x1f00, 0xe840, 0x3200, 0xbf30],
        &[0x602c, 0xbf30],
    ); // str r4,[r5]
    b.run_until(20, 1).unwrap();
    assert_eq!(get(b.cpu(0).unwrap(), reg(2)).unwrap(), 1);
    assert_eq!(memory(&b), 99);
}

#[test]
fn a_physical_bus_alias_invalidates_the_framework_monitor_by_canonical_ram() {
    for backend in test_backends() {
        check_a_physical_bus_alias_invalidates_the_framework_monitor_by_canonical_ram(backend);
    }
}

fn check_a_physical_bus_alias_invalidates_the_framework_monitor_by_canonical_ram(
    backend: RunCpuBackend,
) {
    let mut b = board(
        backend,
        &[0xe850, 0x1f00, 0xe840, 0x3200, 0xbf30],
        &[0xbf30],
    );
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
    for backend in test_backends() {
        check_unrelated_granules_preserve_the_monitor_and_invalid_accesses_refuse(backend);
    }
}

fn check_unrelated_granules_preserve_the_monitor_and_invalid_accesses_refuse(
    backend: RunCpuBackend,
) {
    let code = [0xe850, 0x1f00, 0xe840, 0x3200, 0xbf30];
    let mut b = board(backend, &code, &[0xbf30]);
    b.run_for_instructions(1, 1).unwrap();
    b.write_memory(1, 0x5040, &99u32.to_le_bytes()).unwrap();
    b.run_until(20, 1).unwrap();
    assert_eq!(get(b.cpu(0).unwrap(), reg(2)).unwrap(), 0);
    assert_eq!(memory(&b), 0xaabbccdd);
    for address in [0x11001, 0xe000e010] {
        let mut b = board(
            backend,
            &[0xbf00, 0xe850, 0x1f00, 0xe840, 0x3200, 0xbf30],
            &[0xbf30],
        );
        b.run_for_instructions(1, 1).unwrap();
        set(b.cpu(0).unwrap(), reg(0), address).unwrap();
        assert!(b.run_until(20, 64).is_err());
        assert_eq!(get(b.cpu(0).unwrap(), Register::Pc).unwrap(), 0x10102);
        assert_eq!(get(b.cpu(0).unwrap(), reg(1)).unwrap(), 0);
        assert_eq!(memory(&b), 0x11223344);
    }
    let mut b = board_with_permissions(backend, &code, &[0xbf30], Bus::new(Mode::Strict), 1);
    assert!(b.run_until(20, 64).is_err());
    assert_eq!(get(b.cpu(0).unwrap(), Register::Pc).unwrap(), 0x10104);
    assert_eq!(memory(&b), 0x11223344);
}
#[test]
fn thumb_it_conditions_and_monitor_clearing_on_exception() {
    for backend in test_backends() {
        check_thumb_it_conditions_and_monitor_clearing_on_exception(backend);
    }
}

fn check_thumb_it_conditions_and_monitor_clearing_on_exception(backend: RunCpuBackend) {
    // LDREX; CMP r1,#0; ITE EQ; STREXEQ; MOVNE r2,#7; WFI.
    for initial in [0u32, 1] {
        let mut b = board(
            backend,
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
    let mut b = board(
        backend,
        &[0xe850, 0x1f00, 0xdf00, 0xe840, 0x3200, 0xbf30],
        &[0xbf30],
    );
    b.write_memory(0, 0x10000 + 11 * 4, &0x10201u32.to_le_bytes())
        .unwrap();
    b.write_memory(0, 0x10200, &0x4770u16.to_le_bytes())
        .unwrap(); // BX LR, real exception return
    b.run_until(30, 64).unwrap();
    assert_eq!(get(b.cpu(0).unwrap(), reg(2)).unwrap(), 1);
    assert_eq!(memory(&b), 0x11223344);
}

#[cfg(all(
    feature = "dynarmic",
    any(target_arch = "aarch64", target_arch = "x86_64"),
    any(target_os = "linux", target_os = "macos")
))]
mod dynarmic {
    use super::*;

    #[test]
    fn controller_completes_dynarmic_exception_returns_from_pc_loads() {
        use fujimod_emulator_cpu::arm::Register::Msp;
        let backend = RunCpuBackend::Dynarmic;
        for code in [
            &[0x4770][..],             // bx lr
            &[0xb500, 0xbd00],         // push {lr}; pop {pc}
            &[0xb500, 0xf85d, 0xfb04], // push {lr}; ldr.w pc,[sp],#4
            &[0xb508, 0xe8bd, 0x8008], // push {r3,lr}; ldm.w sp!,{r3,pc}
        ] {
            let mut b = board(backend, &[0xdf00, 0x2609, 0xbf30], &[0xbf30]);
            b.write_memory(0, 0x10000 + 11 * 4, &0x10201u32.to_le_bytes())
                .unwrap();
            let handler = [0xf3efu16, 0x8405] // mrs r4,ipsr
                .into_iter()
                .chain(code.iter().copied())
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>();
            b.write_memory(0, 0x10200, &handler).unwrap();
            b.run_until(30, 64).unwrap();
            let cpu = b.cpu(0).unwrap();
            assert_eq!(get(cpu, reg(4)).unwrap(), 11, "{code:x?}");
            assert_eq!(get(cpu, reg(6)).unwrap(), 9, "{code:x?}");
            assert_eq!(get(cpu, special(Msp)).unwrap(), 0x13000, "{code:x?}");
            assert_eq!(b.state()["mprofile"][0]["nvic"]["active"], json!([]));
        }
    }

    #[test]
    fn cpacr_writes_bound_dynarmic_batches_and_disabled_vfp_faults() {
        use fujimod_emulator_cpu::arm::{
            Register::{Cpacr, S},
            thumb,
        };
        let backend = RunCpuBackend::Dynarmic;
        let mut b = board(
            backend,
            &[
                0xbf00, // nop
                0x6001, // str r1,[r0]; enable CP10/CP11
                0xee70, 0xfa20, // vadd.f32 s31,s0,s1
                0x6002, // str r2,[r0]; disable CP10/CP11
                0xee70, 0xfa20, // vadd.f32 s31,s0,s1; raises NOCP
                0x2609, 0xbf30,
            ],
            &[0xbf30],
        );
        b.write_memory(0, 0x10000 + 3 * 4, &0x10201u32.to_le_bytes())
            .unwrap();
        b.write_memory(0, 0x10200, &[0x30, 0xbf]).unwrap(); // WFI in HardFault.
        b.run_for_instructions(1, 64).unwrap();
        let cpu = b.cpu(0).unwrap();
        set(cpu, reg(0), 0xe000_ed88).unwrap();
        set(cpu, reg(1), 0x00f0_0000).unwrap();
        set(cpu, special(S(0)), 1.5_f32.to_bits()).unwrap();
        set(cpu, special(S(1)), 2.0_f32.to_bits()).unwrap();
        b.run_until(30, 64).unwrap();
        let cpu = b.cpu(0).unwrap();
        assert_eq!(get(cpu, special(Cpacr)).unwrap(), 0);
        assert_eq!(get(cpu, special(S(31))).unwrap(), 3.5_f32.to_bits());
        assert_eq!(get(cpu, reg(6)).unwrap(), 0);
        let state = b.state();
        let nvic = &state["mprofile"][0]["nvic"];
        assert_eq!(nvic["active"], json!([3]));
        assert_eq!(nvic["regs"][0xd28.to_string()], thumb::NOCP);
        let mut stacked_pc = [0; 4];
        b.read_memory(0, 0x13000 - 8, &mut stacked_pc).unwrap();
        assert_eq!(u32::from_le_bytes(stacked_pc), 0x1010a);
    }
}
