use super::*;
use decoder::{Choice, Decoder};
use factory::{Engine, Graph, MemoryMap, Options, Peripheral, Processor, Ram};
use fujimod_emulator_cpu::{Model, arm};
use fujimod_emulator_cpu_unicorn::rprofile;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_exr2_system::boot;
use memory::BusMapping;
use std::sync::{Arc, Mutex};
type Readings = Arc<Mutex<Vec<(u64, u32)>>>;

fn graph(private_peer: bool) -> Graph {
    let choice = |physical, permissions| Choice {
        processors: (0..2)
            .map(|core| MemoryMap {
                core,
                address: 0,
                physical,
                size: 0x1000,
                permissions,
            })
            .collect(),
        dma: vec![BusMapping {
            address: 0,
            physical,
            size: 0x1000,
            permissions: permissions & 3,
        }],
    };
    let initial = choice(0x80000000, 5);
    let mut mappings = initial.processors.clone();
    for core in 0..2 {
        mappings.extend([
            MemoryMap {
                core,
                address: 0x40000000,
                physical: 0x10000,
                size: 0x1000,
                permissions: 7,
            },
            MemoryMap {
                core,
                address: 0x80000000,
                physical: 0x80000000,
                size: 0x1000,
                permissions: 5,
            },
        ]);
    }
    Graph {
        processors: (0..2)
            .map(|core| Processor {
                model: Model::CortexR5F,
                mprofile: None,
                released: true,
                rprofile: Some(rprofile::core::Options {
                    tcm: if private_peer && core == 1 {
                        rprofile::tcm::Options {
                            atcm: rprofile::tcm::BankOptions {
                                bytes: 0x1000,
                                reset: 1,
                            },
                            ..Default::default()
                        }
                    } else {
                        Default::default()
                    },
                    ..Default::default()
                }),
                registers: vec![
                    (Register::Pc, if core == 0 { 0 } else { 0x100 }),
                    (Register::Pstate, 0xd3),
                    (arm::CORE[0], 1),
                    (arm::CORE[1], boot::BASE + 8),
                ],
            })
            .collect(),
        ram: vec![
            Ram {
                base: 0x10000,
                size: 0x1000,
            },
            Ram {
                base: 0x80000000,
                size: 0x1000,
            },
        ],
        dma_mappings: vec![
            initial.dma[0],
            BusMapping {
                address: 0x40000000,
                physical: 0x10000,
                size: 0x1000,
                permissions: 3,
            },
            BusMapping {
                address: 0x80000000,
                physical: 0x80000000,
                size: 0x1000,
                permissions: 1,
            },
        ],
        mappings,
        peripherals: (0..2)
            .map(|core| Peripheral {
                core,
                address: boot::BASE,
                physical: boot::BASE,
                size: 0x1000,
            })
            .collect(),
        decoders: vec![Decoder {
            initial: 0,
            choices: vec![initial, choice(0x10000, 7)],
        }],
        instructions_per_tick: Ratio::new(1, 1).unwrap(),
    }
}

// Exercise a same-tick DMA event scheduled by the selecting MMIO device. It
// must run after the selecting instruction and the stopped mapping commit.
struct Latch {
    control: boot::Block98000,
    reads: Readings,
    timed_selection: Option<(u64, usize)>,
}
impl Device for Latch {
    fn name(&self) -> &'static str {
        "decode-and-dma"
    }
    fn read(&mut self, cx: &mut Cx<'_>, a: u64, n: u8) -> u64 {
        self.control.read(cx, a, n)
    }
    fn write(&mut self, cx: &mut Cx<'_>, a: u64, n: u8, v: u64) {
        self.control.write(cx, a, n, v);
        if cx.refusal().is_none() {
            cx.schedule(cx.now(), 0);
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        let mut bytes = [0; 4];
        if let Err(e) = cx.ram_read(4, &mut bytes) {
            cx.fail(e);
            return;
        }
        self.reads
            .lock()
            .unwrap()
            .push((cx.now(), u32::from_le_bytes(bytes)));
        if let Err(e) = cx.ram_write(0x20, &0x12345678u32.to_le_bytes()) {
            cx.fail(e);
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.timed_selection.map(|s| s.0)
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.timed_selection.is_some_and(|s| s.0 <= cx.now()) {
            let (_, choice) = self.timed_selection.take().unwrap();
            if let Err(e) = cx.select_memory(0, choice) {
                cx.fail(e);
            }
        }
    }
}
fn board(private_peer: bool) -> (Board, Readings) {
    board_with_event(private_peer, None)
}
fn board_with_event(
    private_peer: bool,
    timed_selection: Option<(u64, usize)>,
) -> (Board, Readings) {
    board_from_graph(graph(private_peer), timed_selection)
}
fn board_from_graph(graph: Graph, timed_selection: Option<(u64, usize)>) -> (Board, Readings) {
    let reads = Arc::new(Mutex::new(vec![]));
    let control = boot::Block98000::new(
        boot::Options {
            boot_source: 9,
            selection_reset: 0,
        },
        boot::Wiring {
            decoder: 0,
            selections: vec![
                boot::Selection { word: 0, choice: 0 },
                boot::Selection { word: 1, choice: 1 },
            ],
        },
    )
    .unwrap();
    let mut bus = Bus::new(bus::Mode::Strict);
    bus.register(
        boot::BASE,
        0x1000,
        Box::new(Latch {
            control,
            reads: reads.clone(),
            timed_selection,
        }),
    )
    .unwrap();
    let board = graph
        .build(
            Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1024 * 1024),
            bus,
            None,
        )
        .unwrap();
    for (address, value) in [(0x80000000, 11), (0x40000000, 42)] {
        board
            .write_physical(
                address,
                &[0xe5810000u32, 0xe3a02000 | value, 0xe320f003]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        board
            .write_physical(
                address + 0x100,
                &[0xe3a03000u32 | value, 0xe320f003]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
    }
    (board, reads)
}
fn word(board: &Board, core: usize, address: u64) -> u32 {
    let mut bytes = [0; 4];
    board.read_memory(core, address, &mut bytes).unwrap();
    u32::from_le_bytes(bytes)
}
#[test]
fn selecting_mmio_commits_both_processors_and_dma_before_same_tick_events() {
    for pause in [1, 2, 3, 4] {
        let (mut b, reads) = board(false);
        b.run_for_instructions(pause, 1).unwrap();
        b.run_until(10, 1).unwrap();
        assert_eq!(b.read_register(0, arm::CORE[2]).unwrap(), 42);
        assert_eq!(b.read_register(1, arm::CORE[3]).unwrap(), 42);
        assert_eq!(*reads.lock().unwrap(), [(1, 0xe3a0202a)]);
        assert_eq!(b.instruction_counts(), [3, 2]);
        assert_eq!(b.state()["memory_decoders"]["choices"], json!([1]));
        for core in 0..2 {
            assert_eq!(word(&b, core, 0x20), 0x12345678);
            assert_eq!(word(&b, core, 0x80000004), 0xe3a0200b);
        }
    }
}
#[test]
fn selecting_external_decode_preserves_peer_tcm_and_updates_its_hidden_underlay() {
    let (mut b, _) = board(true);
    b.write_memory(1, 0x100, &0xaabbccddu32.to_le_bytes())
        .unwrap();
    b.run_for_instructions(1, 1).unwrap();
    assert_eq!(word(&b, 1, 0x100), 0xaabbccdd);
    let selector = arm::Coprocessor {
        cp: 15,
        opc1: 0,
        crn: 9,
        crm: 1,
        opc2: 1,
        secure: 0,
        wide: false,
    };
    b.cpu(1).unwrap().coprocessor(selector, Some(0)).unwrap();
    assert_eq!(word(&b, 1, 0x100), 0xe3a0302a);
    b.cpu(1).unwrap().coprocessor(selector, Some(1)).unwrap();
    assert_eq!(word(&b, 1, 0x100), 0xaabbccdd);
}
#[test]
fn unsupported_register_choice_refuses_before_changing_any_decode() {
    let (mut b, reads) = board(false);
    b.write_register(0, arm::CORE[0], 2).unwrap();
    assert!(
        b.run_for_instructions(1, 1)
            .unwrap_err()
            .contains("unsupported memory selection word 0x2")
    );
    assert!(reads.lock().unwrap().is_empty());
    assert_eq!(b.state()["memory_decoders"]["choices"], json!([0]));
    assert_eq!(word(&b, 0, 4), 0xe3a0200b);
    assert_eq!(word(&b, 1, 4), 0xe3a0200b);
}
#[test]
fn device_events_commit_while_stopped_and_refuse_a_change_inside_active_execution() {
    let (mut b, _) = board_with_event(false, Some((4, 0)));
    b.run_until(10, 1).unwrap();
    assert_eq!(b.state()["memory_decoders"]["choices"], json!([0]));
    assert_eq!(word(&b, 0, 4), 0xe3a0200b);
    assert_eq!(word(&b, 1, 4), 0xe3a0200b);
    let (mut b, _) = board_with_event(false, Some((1, 1)));
    assert!(
        b.run_until(10, 1)
            .unwrap_err()
            .contains("requires a stopped boundary")
    );
    assert_eq!(b.state()["memory_decoders"]["choices"], json!([0]));
    assert_eq!(b.state()["memory_decoders"]["pending"], json!([null]));
    assert_eq!(word(&b, 0, 4), 0xe3a0200b);
}
#[test]
fn a_backend_mapping_failure_keeps_the_partial_decode_unusable() {
    let (mut b, _) = board(false);
    b.cpu(1).unwrap().unmap(0, 0x1000).unwrap();
    let error = b.run_for_instructions(1, 1).unwrap_err();
    assert!(error.contains("memory decoding failed"), "{error}");
    let counts = b.instruction_counts();
    assert_eq!(b.run_until(10, 1).unwrap_err(), error);
    assert_eq!(b.instruction_counts(), counts);
    assert_eq!(b.state()["memory_decoders"]["failure"], error);
}
#[test]
fn decoder_choices_require_existing_unique_windows_and_valid_backing() {
    graph(false).validate().unwrap();
    for case in 0..9 {
        let mut g = graph(false);
        match case {
            0 => g.decoders[0].initial = 2,
            1 => g.decoders[0].choices[0].processors[0].permissions = 7,
            2 => g.decoders[0].choices[1].processors[0].physical = 0x20000,
            3 => g.decoders[0].choices[1].processors[0].size = 0x2000,
            4 => g.decoders[0].choices[1].dma[0].address = 0x1000,
            5 => g.decoders[0].choices[1].dma[0].physical = u64::MAX,
            6 => {
                let d = g.decoders.pop().unwrap();
                g.decoders.push(Decoder {
                    initial: 0,
                    choices: vec![Choice {
                        processors: d.choices[0].processors.clone(),
                        dma: d.choices[0].dma.clone(),
                    }],
                });
                g.decoders.push(d);
            }
            7 => {
                g.decoders[0].choices[1].processors[1] =
                    g.decoders[0].choices[1].processors[0].clone();
            }
            8 => g.decoders[0].choices[0].dma[0].permissions = 3,
            _ => unreachable!(),
        }
        assert!(g.validate().is_err(), "case {case}");
    }
}

#[test]
fn a7_remap_changes_both_cpu_aliases_and_dma_at_one_boundary() {
    for pause in [1, 2, 3] {
        let mut graph = graph(false);
        for cpu in &mut graph.processors {
            cpu.model = Model::CortexA7;
            cpu.rprofile = None;
        }
        let (mut b, reads) = board_from_graph(graph, None);
        for core in 0..2 {
            b.configure_armv7a(
                core,
                armv7a::Config {
                    mpidr: 0x80000000 | core as u32,
                    configuration_base: 0xffff0000,
                    cycles_per_tick: 1,
                },
            )
            .unwrap();
        }
        b.run_for_instructions(pause, 1).unwrap();
        b.run_until(10, 1).unwrap();
        assert_eq!(b.instruction_counts(), [3, 2]);
        assert_eq!(b.read_register(0, arm::CORE[2]).unwrap(), 42);
        assert_eq!(b.read_register(1, arm::CORE[3]).unwrap(), 42);
        assert_eq!(*reads.lock().unwrap(), [(1, 0xe3a0202a)]);
        for core in 0..2 {
            assert_eq!(word(&b, core, 0x20), 0x12345678);
            assert_eq!(word(&b, core, 0x80000004), 0xe3a0200b);
        }
    }
}

#[test]
fn a7_remap_keeps_unrelated_mmu_translation_until_guest_disables_the_mmu() {
    let mut graph = graph(false);
    for ram in &mut graph.ram {
        ram.size = 0x8000;
    }
    for map in &mut graph.mappings {
        map.size = 0x8000;
    }
    for map in &mut graph.dma_mappings {
        map.size = 0x8000;
    }
    for choice in &mut graph.decoders[0].choices {
        for map in &mut choice.processors {
            map.size = 0x8000;
        }
        for map in &mut choice.dma {
            map.size = 0x8000;
        }
    }
    for (core, cpu) in graph.processors.iter_mut().enumerate() {
        cpu.model = Model::CortexA7;
        cpu.rprofile = None;
        cpu.registers[0].1 = 0x80000000 + core as u64 * 0x100;
    }
    let (mut b, _) = board_from_graph(graph, None);
    for core in 0..2 {
        b.configure_armv7a(
            core,
            armv7a::Config {
                mpidr: 0x80000000 | core as u32,
                configuration_base: 0xffff0000,
                cycles_per_tick: 1,
            },
        )
        .unwrap();
    }
    let words = [
        0xe5810000u32,
        0xee110f10,
        0xe3c00001,
        0xee010f10,
        0xe3a0202a,
        0xe320f003,
    ];
    b.write_physical(
        0x80000000,
        &words
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    // Section entries: code and peripheral translation. The level-one table
    // itself is in the physical window that the selecting store replaces.
    for (index, entry) in [(0x800, 0x80000c02u32), (0xfff, 0xfff00c02)] {
        b.write_physical(0x80004000 + index * 4, &entry.to_le_bytes())
            .unwrap();
    }
    let cp = |crn, crm, opc2| arm::Coprocessor {
        cp: 15,
        opc1: 0,
        crn,
        crm,
        opc2,
        secure: 0,
        wide: false,
    };
    let cpu = b.cpu(0).unwrap();
    cpu.coprocessor(cp(2, 0, 0), Some(0x4000)).unwrap();
    cpu.coprocessor(cp(3, 0, 0), Some(3)).unwrap();
    let sctlr = cpu.coprocessor(cp(1, 0, 0), None).unwrap();
    cpu.coprocessor(cp(1, 0, 0), Some(sctlr | 1)).unwrap();
    b.run_for_instructions(100, 1).unwrap();
    assert_eq!(b.state()["memory_decoders"]["choices"], json!([1]));
    assert_eq!(b.read_register(0, arm::CORE[2]).unwrap(), 42);
    assert_eq!(
        b.cpu(0).unwrap().coprocessor(cp(1, 0, 0), None).unwrap() & 1,
        0
    );
    assert_eq!(word(&b, 0, 0x4000), 0);
}
